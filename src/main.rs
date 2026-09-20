#![cfg_attr(all(windows, feature = "install"), windows_subsystem = "windows")]
#![allow(
    clippy::similar_names,
    clippy::ptr_as_ptr,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::cast_sign_loss,
    clippy::struct_excessive_bools,
    clippy::type_complexity
)]

mod assets;
mod config;
mod pty;
mod renderer;
mod terminal;
mod ui;
mod util;

use assets::{GlobalAssets, PyonjiAsset, PyonjiAssetsSource};
use pty::Event;
use terminal::{Tab as TerminalTab, *};
use ui::Overlay;
use ui::Terminal;

use std::{array, path::PathBuf, sync::Arc};

use async_channel::{Receiver, Sender};
use clap::Parser;
use gpui::{prelude::*, *};
use gpui_base::*;
use gpui_component::{
    Icon, Root, ThemeMode, WindowExt,
    notification::{Notification, NotificationType},
};
use mlua::prelude::*;
use smallvec::{SmallVec, smallvec};
use tracing_subscriber::prelude::*;

use crate::{
    pty::SshConnection,
    ui::{OverlayScreen, StatusBar, StatusBarEvent, StatusBarMode},
};

actions!([
    EnterRename,
    EnterLuaRepl,
    OpenPalette,
    OpenReleases,
    OpenSessions
]);

#[derive(clap::Parser)]
struct Cli {
    path: Option<PathBuf>,
}

fn main() {
    cfg_select! {
        feature = "install" => {
            logging::init();
        }
        _ => {
            tracing_subscriber::registry()
                .with(tracing_subscriber::fmt::layer())
                .with(tracing_subscriber::filter::LevelFilter::WARN)
                .init();
        }
    };

    gpui_platform::application()
        .with_assets(GlobalAssets::new([
            Box::new(gpui_component_assets::Assets),
            Box::new(PyonjiAssetsSource),
        ]))
        .run(|cx| {
            gpui_component::init(cx);
            Theme::init(cx);

            let window_options = Pyonji::window_options(cx);
            cx.open_window(window_options, |window, cx| {
                gpui_component::Theme::change(ThemeMode::Dark, Some(window), cx);

                let py = cx.new(|cx| Pyonji::new(window, cx));
                cx.new(|cx| Root::new(py, window, cx))
            })
            .expect("open window");
        });
}

struct Pyonji {
    cli: Cli,
    lua: Lua,
    tx: Sender<Event>,
    focus_handle: FocusHandle,

    //workspace
    session_manager: SessionManager,
    tabs: [Option<TerminalTab>; 9],
    current_tab: Option<usize>,
    detached_sessions: Vec<SessionId>,
    wheel_remainder: f32,

    //views
    terminal: Entity<Terminal>,
    status_bar: Entity<StatusBar>,
    overlay: Entity<Overlay>,

    //jobs
    _event_loop_task: Task<()>,
    _subscriptions: SmallVec<[Subscription; 4]>,

    //config
    font_family: Option<String>,
    font_size: f32,
    line_height: f32,
    default_cwd: Option<PathBuf>,
    ssh_sessions: Vec<SshConnection>,
}

impl Pyonji {
    const TITLE: &str = cfg_select! {
        feature = "install" => "Pyonji",
        _ => {
            const_format::formatcp!("Pyonji {}", git_version::git_version!())
        },
    };
    const ICON: &[u8] = include_bytes!("../resources/icon.ico");
    const INITIAL_SIZE: (f32, f32) = (1280.0, 720.0);

    const TERMINAL_CONTEXT: &str = "terminal";

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cli = Cli::parse();
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();
        {
            let tx = tx.clone();
            if let Ok(print) = lua.create_function(move |lua, args: mlua::MultiValue| {
                let tostring: mlua::Function = lua.globals().get("tostring")?;
                let mut parts = Vec::new();
                for value in args {
                    let text: String = tostring.call(value)?;
                    parts.push(text);
                }
                _ = tx.try_send(Event::LuaPrint(parts.join("\t")));
                Ok(())
            }) {
                _ = lua.globals().set("print", print);
            }
        }

        config::watch(tx.clone());

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        cx.bind_keys([
            KeyBinding::new("ctrl-b r", EnterRename, Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-b l", EnterLuaRepl, Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-shift-f", OpenPalette, Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-shift-r", OpenReleases, Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-shift-s", OpenSessions, Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-b 1", SwitchTab(0), Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-b 2", SwitchTab(1), Some(Self::TERMINAL_CONTEXT)),
            KeyBinding::new("ctrl-b 3", SwitchTab(2), Some(Self::TERMINAL_CONTEXT)),
        ]);

        let py = cx.weak_entity();
        Self::spawn_inital_session(window, cx);
        Self {
            cli,
            lua,
            tx: tx.clone(),
            focus_handle,

            session_manager: SessionManager::new(tx.clone()),
            tabs: array::from_fn(|_| None),
            current_tab: Some(0),
            detached_sessions: vec![],
            wheel_remainder: 0.0,

            terminal: cx.new(|cx| Terminal::new(py.clone(), cx)),
            status_bar: Self::setup_status_bar(window, cx),
            overlay: cx.new(|cx| Overlay::new(py.clone(), window, cx)),

            _event_loop_task: Self::spawn_event_loop(rx, window, cx),
            _subscriptions: smallvec![],

            font_family: None,
            font_size: 38.0,
            line_height: 1.1,
            default_cwd: None,
            ssh_sessions: vec![],
        }
    }

    fn setup_status_bar(window: &mut Window, cx: &mut Context<Self>) -> Entity<StatusBar> {
        let py = cx.weak_entity();
        let bar = cx.new(|cx| StatusBar::new(py, cx));
        cx.subscribe_in(
            &bar,
            window,
            |this, status_bar, ev: &StatusBarEvent, window, cx| match ev {
                StatusBarEvent::Dismiss => {
                    status_bar.update(cx, |this, cx| {
                        this.set_mode(StatusBarMode::Sessions, cx);
                        this.reset_history();
                    });
                    window.focus(&this.focus_handle, cx);
                }
                StatusBarEvent::Renamed(id, title) => {
                    if let Some(session) = this.session_manager.session_mut(*id) {
                        session.rename(title.clone());
                    }
                }
                StatusBarEvent::ExecLua(code) => {
                    let lua = this.lua.clone();
                    let res = config::with_env(this, cx, |_| lua.load(code).exec());
                    this.status_bar.update(cx, |this, cx| {
                        this.push_lua_history(code.clone());
                        this.reset_history();
                        cx.notify();
                    });
                    if let Err(e) = res {
                        window.dispatch_action(Box::new(PushError::new(e)), cx);
                    }
                }
            },
        )
        .detach();
        bar
    }

    fn spawn_event_loop(
        rx: Receiver<Event>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                if let Ok(event) = rx.recv().await {
                    _ = this
                        .update_in(cx, |this, window, cx| match event {
                            Event::Closed(id) => {
                                if this.close_session(id, cx) && this.session_manager.is_empty() {
                                    cx.quit();
                                }
                                cx.notify();
                            }
                            Event::Data(id, data) => {
                                this.session_manager.update_session(id, &data);
                                cx.notify();
                            }
                            Event::ProgramChanged((id, title)) => {
                                if let Some(session) = this.session_manager.session_mut(id) {
                                    session.set_title(title);
                                    cx.notify();
                                }
                            }
                            Event::ConfigChanged => {
                                config::load(this, cx);
                                cx.notify();
                            }
                            Event::LuaPrint(text) => {
                                let text = text.replace(['\r'], " ");
                                if !text.is_empty() {
                                    window.dispatch_action(Box::new(PushLuaPrint::new(text)), cx);
                                }
                            }
                            Event::Exit => {
                                cx.quit();
                            }
                        })
                        .unwrap();
                }
            }
        })
    }

    fn spawn_inital_session(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            _ = this
                .update_in(cx, |this, window, cx| {
                    config::load(this, cx);
                    let res = this
                        .session_manager
                        .create_session(20, 80, this.cli.path.as_deref());
                    match res {
                        Ok(id) => {
                            this.tabs[0] = Some(TerminalTab::new(id));
                            this.current_tab = Some(0);
                            cx.notify();
                        }
                        Err(e) => {
                            window.dispatch_action(Box::new(PushError::new(e.to_string())), cx);
                        }
                    }
                })
                .unwrap()
        })
        .detach()
    }

    fn on_enter_rename(&mut self, _: &EnterRename, window: &mut Window, cx: &mut Context<Self>) {
        let Some(session) = self.active_session() else {
            return;
        };
        let Some(inital) = self.session_manager.session(session) else {
            return;
        };
        let inital = inital.title();
        let mode = StatusBarMode::Rename {
            inital: inital.to_string(),
            session,
        };
        self.status_bar.update(cx, |this, cx| {
            this.set_mode(mode, cx);
        });
    }

    fn on_enter_lua(&mut self, _: &EnterLuaRepl, _: &mut Window, cx: &mut Context<Self>) {
        self.status_bar.update(cx, |this, cx| {
            this.set_mode(StatusBarMode::Lua, cx);
        });
    }

    fn on_switch_tab(&mut self, tab: &SwitchTab, _: &mut Window, cx: &mut Context<Self>) {
        self.switch_tab(tab.0, cx);
    }

    fn on_error(&mut self, error: &PushError, window: &mut Window, cx: &mut Context<Self>) {
        Self::push_note(
            NotificationType::Error,
            "Error",
            error.string.clone(),
            error.autohide,
            window,
            cx,
        );
    }

    fn on_warning(&mut self, warning: &PushWarning, window: &mut Window, cx: &mut Context<Self>) {
        Self::push_note(
            NotificationType::Warning,
            "Warning",
            warning.string.clone(),
            warning.autohide,
            window,
            cx,
        );
    }

    fn on_info(&mut self, info: &PushInfo, window: &mut Window, cx: &mut Context<Self>) {
        Self::push_note(
            NotificationType::Info,
            "Info",
            info.string.clone(),
            true,
            window,
            cx,
        );
    }

    fn on_lua_print(&mut self, print: &PushLuaPrint, window: &mut Window, cx: &mut Context<Self>) {
        fn lua_icon(cx: &App) -> Icon {
            Icon::new(PyonjiAsset::Lua).text_color(gpui_component::ActiveTheme::theme(cx).info)
        }

        let note = Notification::new()
            .icon(lua_icon(cx))
            .bg(cx.theme().surface)
            .title("Lua")
            .autohide(true)
            .placement(Anchor::TopRight)
            .message(print.string.clone());

        window.push_notification(note, cx);
    }

    fn push_note(
        kind: NotificationType,
        title: impl Into<SharedString>,
        message: impl Into<SharedString>,
        autohide: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        fn notification_icon(kind: NotificationType, cx: &App) -> Icon {
            let theme = gpui_component::ActiveTheme::theme(cx);
            match kind {
                NotificationType::Error => {
                    Icon::new(PyonjiAsset::NotificationError).text_color(theme.danger)
                }
                NotificationType::Warning => {
                    Icon::new(PyonjiAsset::NotificationWarning).text_color(theme.warning)
                }
                NotificationType::Success => {
                    Icon::new(PyonjiAsset::NotificationSuccess).text_color(theme.success)
                }
                NotificationType::Info => {
                    Icon::new(PyonjiAsset::NotificationInfo).text_color(theme.info)
                }
            }
        }

        let note = Notification::new()
            .icon(notification_icon(kind, cx))
            .bg(cx.theme().surface)
            .title(title)
            .autohide(autohide)
            .placement(Anchor::TopRight)
            .message(message);

        window.push_notification(note, cx);
    }

    fn window_options(cx: &mut App) -> WindowOptions {
        let icon: Option<Arc<image::RgbaImage>> = image::load_from_memory(Self::ICON)
            .map(|image| Arc::new(image.to_rgba8()))
            .inspect_err(|error| tracing::error!(%error, "failed to decode window icon"))
            .ok();
        let (initial_width, initial_height) = Self::INITIAL_SIZE;
        let initial_size = size(px(initial_width), px(initial_height));
        let initial_origin = cx
            .primary_display()
            .map(|display| {
                let bounds = display.bounds();
                let x = (f32::from(bounds.size.width) - initial_width).max(0.0) / 2.0
                    + f32::from(bounds.origin.x);
                let y = (f32::from(bounds.size.height) - initial_height).max(0.0) / 2.0
                    + f32::from(bounds.origin.y);
                point(px(x), px(y))
            })
            .unwrap_or_default();
        WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::new(
                initial_origin,
                initial_size,
            ))),
            titlebar: Some(TitlebarOptions {
                title: Some(Self::TITLE.into()),
                appears_transparent: false,
                traffic_light_position: None,
            }),
            app_id: Some("pyonji".to_string()),
            icon,
            ..Default::default()
        }
    }

    pub fn close_session(&mut self, session: SessionId, cx: &mut Context<Self>) -> bool {
        if self.session_manager.session(session).is_none() {
            return false;
        }
        self.session_manager.remove_session(session);
        self.detached_sessions
            .retain(|detached| *detached != session);

        let mut removed_current_tab = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Some(tab_state) = tab.as_mut() else {
                continue;
            };
            if !tab_state.remove_session(session) {
                continue;
            }
            if tab_state.is_empty() {
                *tab = None;
                removed_current_tab |= Some(index) == self.current_tab;
            }
        }

        let Some(current_tab) = self.current_tab else {
            return true;
        };

        if removed_current_tab {
            self.switch_to_previous_live_tab_or_stay(current_tab, cx);
        } else {
            self.resize_tab(current_tab, cx);
        }
        true
    }

    pub fn switch_tab(&mut self, tab: usize, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        if tab >= self.tabs.len() {
            return;
        }
        if self.tabs[tab].is_none() {
            let id = match self.session_manager.create_session(
                terminal.rows.max(1),
                terminal.cols.max(1),
                self.default_cwd.as_deref(),
            ) {
                Ok(id) => id,
                Err(error) => {
                    tracing::error!(error = ?error, "failed to create tab session");
                    return;
                }
            };
            self.tabs[tab] = Some(TerminalTab::new(id));
        }

        self.current_tab = Some(tab);
        self.wheel_remainder = 0.0;
        self.resize_tab(tab, cx);
        //self.update_ime_cursor_area();
        cx.notify();
    }

    fn switch_to_previous_live_tab_or_stay(&mut self, closed_tab: usize, cx: &mut Context<Self>) {
        if let Some(tab) = (0..self.tabs.len())
            .map(|offset| (closed_tab + self.tabs.len() - 1 - offset) % self.tabs.len())
            .find(|&tab| self.tabs[tab].is_some())
        {
            self.current_tab = Some(tab);
            self.wheel_remainder = 0.0;
            self.resize_tab(tab, cx);
            return;
        }

        self.current_tab = Some(closed_tab.min(self.tabs.len().saturating_sub(1)));
    }

    pub fn resize_tab(&mut self, index: usize, cx: &mut Context<Self>) {
        let terminal = self.terminal.read(cx);
        let Some(tab) = self.tabs[index].as_ref() else {
            return;
        };
        for (session_id, geometry) in tab.layout(PaneGeometry {
            x: 0,
            y: 0,
            cols: terminal.cols,
            rows: terminal.rows,
        }) {
            self.session_manager.resize_session(
                session_id,
                geometry.rows.max(1),
                geometry.cols.max(1),
            );
        }
    }

    fn tab_layouts(&self, rows: u16, cols: u16) -> Vec<(SessionId, PaneGeometry)> {
        let Some(current_tab) = self.current_tab else {
            return vec![];
        };
        self.tabs[current_tab]
            .as_ref()
            .map(|tab| {
                tab.layout(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols,
                    rows,
                })
            })
            .unwrap_or_default()
    }

    pub fn tab_dividers(&self, rows: u16, cols: u16) -> Vec<Divider> {
        let Some(current_tab) = self.current_tab else {
            return vec![];
        };
        self.tabs[current_tab]
            .as_ref()
            .map(|tab| {
                tab.dividers(PaneGeometry {
                    x: 0,
                    y: 0,
                    cols,
                    rows,
                })
            })
            .unwrap_or_default()
    }

    pub fn active_session(&self) -> Option<SessionId> {
        self.tabs[self.current_tab?]
            .as_ref()
            .and_then(TerminalTab::active_session)
    }
}

impl Render for Pyonji {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        //window.request_animation_frame();
        v_flex()
            .id("main-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .on_action(cx.listener(Self::on_error))
            .on_action(cx.listener(Self::on_warning))
            .on_action(cx.listener(Self::on_info))
            .on_action(cx.listener(Self::on_lua_print))
            .child(
                v_flex()
                    .size_full()
                    .p_2()
                    .child(
                        div()
                            .size_full()
                            .track_focus(&self.focus_handle)
                            .key_context(Self::TERMINAL_CONTEXT)
                            .on_action(cx.listener(Self::on_enter_rename))
                            .on_action(cx.listener(Self::on_enter_lua))
                            .on_action(cx.listener(Self::on_switch_tab))
                            .on_action(cx.listener(|this, _: &OpenPalette, window, cx| {
                                this.overlay.update(cx, |this, cx| {
                                    this.open(OverlayScreen::Palette, window, cx);
                                });
                            }))
                            .on_action(cx.listener(|this, _: &OpenReleases, window, cx| {
                                this.overlay.update(cx, |this, cx| {
                                    this.open(OverlayScreen::Releases, window, cx);
                                });
                            }))
                            .on_action(cx.listener(|this, _: &OpenSessions, window, cx| {
                                this.overlay.update(cx, |this, cx| {
                                    this.open(OverlayScreen::Sessions, window, cx);
                                });
                            }))
                            .child(self.terminal.clone())
                            .children(Root::render_dialog_layer(window, cx)),
                    )
                    .child(self.status_bar.clone()),
            )
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct SwitchTab(usize);

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct PushError {
    string: SharedString,
    autohide: bool,
}

impl PushError {
    fn new(v: impl ToString) -> Self {
        Self {
            string: v.to_string().into(),
            autohide: false,
        }
    }

    #[allow(dead_code)]
    fn autohide(mut self, autohide: bool) -> Self {
        self.autohide = autohide;
        self
    }
}

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct PushWarning {
    string: SharedString,
    autohide: bool,
}

impl PushWarning {
    #[allow(dead_code)]
    fn new(v: impl ToString) -> Self {
        Self {
            string: v.to_string().into(),
            autohide: true,
        }
    }

    #[allow(dead_code)]
    fn autohide(mut self, autohide: bool) -> Self {
        self.autohide = autohide;
        self
    }
}

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct PushInfo {
    string: SharedString,
}

impl PushInfo {
    #[allow(dead_code)]
    fn new(v: impl ToString) -> Self {
        Self {
            string: v.to_string().into(),
        }
    }
}

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct PushLuaPrint {
    string: SharedString,
}

impl PushLuaPrint {
    fn new(v: impl ToString) -> Self {
        Self {
            string: v.to_string().into(),
        }
    }
}

pub trait PyTheme {
    fn theme(&self) -> &Theme;
}

impl PyTheme for App {
    #[inline(always)]
    fn theme(&self) -> &Theme {
        Theme::global(self)
    }
}

impl Global for Theme {}

pub struct Theme {
    background: Rgba,
    surface: Rgba,
    text: Rgba,
    selected: Rgba,
    unselected: Rgba,
    selected_border: Rgba,
    unselected_border: Rgba,
}

impl Theme {
    pub fn init(cx: &mut App) {
        cx.set_global(Theme::new());
    }

    fn new() -> Self {
        Self {
            background: Rgba::new(24.0 / 255.0, 24.0 / 255.0, 24.0 / 255.0, 1.0),
            surface: Rgba::new(30.0 / 255.0, 30.0 / 255.0, 46.0 / 255.0, 1.0),
            text: Rgba::new(0.9, 0.9, 0.9, 1.0),
            selected: Rgba::new(58.0 / 255.0, 58.0 / 255.0, 92.0 / 255.0, 1.0),
            unselected: Rgba::new(40.0 / 255.0, 40.0 / 255.0, 40.0 / 255.0, 1.0),
            selected_border: Rgba::new(0.4, 0.4, 0.6, 1.0),
            unselected_border: Rgba::new(0.2, 0.2, 0.2, 1.0),
        }
    }
}
