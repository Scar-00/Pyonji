#![cfg_attr(all(windows, feature = "install"), windows_subsystem = "windows")]
#![allow(
    clippy::similar_names,
    clippy::ptr_as_ptr,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::struct_field_names,
    clippy::too_many_lines,
    clippy::cast_sign_loss,
    clippy::type_complexity
)]

mod assets;
mod commands;
mod config;
mod logging;
mod lua_complete;
mod pty;
mod renderer;
mod terminal;
mod ui;
mod util;

use assets::{GlobalAssets, PyonjiAsset, PyonjiAssetsSource};
use pty::Event;
use terminal::{Tab as TerminalTab, *};
use tracing::Level;
use tracing_subscriber::fmt::time;
use ui::Overlay;
use ui::Terminal;

use std::path::Path;
use std::{array, path::PathBuf, sync::Arc};

use anyhow::anyhow;
use async_channel::{Receiver, Sender};
use clap::Parser;
use gpui::{prelude::*, *};
use gpui_base::*;
use gpui_component::{
    Icon, Root, ThemeMode, WindowExt,
    notification::{Notification, NotificationType},
};
use gpui_component_assets as gassets;
use mlua::prelude::*;
use smallvec::{SmallVec, smallvec};
use tracing_subscriber::prelude::*;

use crate::config::LuaAction;
use crate::logging::LogEmitter;
use crate::logging::ResultLogExt as _;
use crate::logging::TracingLogSubscriber;
use crate::ui::DividerDrag;
use crate::{
    pty::SshConnection,
    ui::{OverlayScreen, StatusBar, StatusBarEvent, StatusBarMode},
};

actions!([
    EnterRename,
    EnterLuaRepl,
    OpenPalette,
    OpenReleases,
    OpenSessions,
    OpenDetached,
    Submit,
    Next,
    Prev,
    ClipboardPaste,
    ClipboardCopy,
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
        _ => {}
    };

    let log_emitter = LogEmitter::new();
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_file(false)
                .with_level(false)
                .with_timer(time::ChronoLocal::new("%H:%M:%S".into()))
                .with_writer(TracingLogSubscriber::new(&log_emitter)),
        )
        .with(tracing_subscriber::filter::LevelFilter::WARN)
        .init();

    gpui_platform::application()
        .with_assets(GlobalAssets::new([
            Box::new(gassets::Assets),
            Box::new(PyonjiAssetsSource),
        ]))
        .run(|cx| {
            gpui_component::init(cx);
            gpui_tokio::init(cx);
            Theme::init(cx);

            let window_options = Pyonji::window_options(cx);
            cx.open_window(window_options, |window, cx| {
                gpui_component::Theme::change(ThemeMode::Dark, Some(window), cx);

                window
                    .spawn(cx, async move |cx| {
                        while let Some(ev) = log_emitter.recv().await {
                            _ = cx.update(|window, cx| {
                                let action = match ev.level {
                                    Level::ERROR => PushError::new(ev.line).boxed_clone(),
                                    Level::WARN => PushWarning::new(ev.line).boxed_clone(),
                                    _ => PushInfo::new(ev.line).boxed_clone(),
                                };
                                window.dispatch_action(action, cx);
                            })
                        }
                    })
                    .detach();

                let py = cx.new(|cx| Pyonji::new(window, cx));
                cx.new(|cx| Root::new(py, window, cx))
            })
            .expect("open window");
        });
}

struct Pyonji {
    cli: Cli,
    lua: Lua,
    _tx: Sender<Event>,
    focus_handle: FocusHandle,

    //workspace
    session_manager: SessionManager,
    tabs: [Option<TerminalTab>; 9],
    current_tab: Option<usize>,
    detached_sessions: Vec<SessionId>,
    wheel_remainder: f32,
    selection_drag: Option<SessionId>,

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
    registered_callbacks: Vec<LuaAction>,
    editor: Option<String>,
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

    fn init(cx: &mut App) {
        use ui::overlay;

        cx.bind_keys([
            KeyBinding::new("ctrl-shift-v", ClipboardPaste, Some(Terminal::CONTEXT)),
            KeyBinding::new("ctrl-shift-c", ClipboardCopy, Some(Terminal::CONTEXT)),
            KeyBinding::new("ctrl-shift-p", OpenPalette, Some(Terminal::CONTEXT)),
        ]);

        ui::status_bar::init(cx);
        overlay::palette::init(cx);
        overlay::sessions::init(cx);
        overlay::releases::init(cx);
        overlay::opener::init(cx);
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cli = Cli::parse();
        let lua = unsafe { Lua::unsafe_new() };
        _ = config::install_inspect(&lua);
        for (name, source) in config::LUA_MODULES {
            let function = lua.load(*source).into_function().unwrap();
            let package: LuaTable = lua.globals().get("package").unwrap();
            let preload: LuaTable = package.get("preload").unwrap();
            preload.set(*name, function.clone()).unwrap();
            function.call::<()>(()).unwrap();
        }
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

        Self::init(cx);

        config::watch(tx.clone());

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);
        cx.on_blur(&focus_handle, window, |this, _, cx| {
            this.selection_drag = None;
            this.terminal
                .update(cx, |terminal, _| terminal.divider_drag = None);
        })
        .detach();
        cx.observe_window_activation(window, |this, window, cx| {
            if !window.is_window_active() {
                this.selection_drag = None;
                this.terminal
                    .update(cx, |terminal, _| terminal.divider_drag = None);
            }
        })
        .detach();

        let py = cx.weak_entity();
        Self::spawn_inital_session(window, cx);
        Self {
            cli,
            lua,
            _tx: tx.clone(),
            focus_handle,

            session_manager: SessionManager::new(tx.clone()),
            tabs: array::from_fn(|_| None),
            current_tab: Some(0),
            detached_sessions: vec![],
            wheel_remainder: 0.0,
            selection_drag: None,

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
            registered_callbacks: vec![],
            editor: None,
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
                    config::with_env(this, window, cx, |_| lua.load(code).exec()).log();
                    this.status_bar.update(cx, |this, cx| {
                        this.push_lua_history(code.clone());
                        this.reset_history();
                        cx.notify();
                    });
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
                    _ = this.update_in(cx, |this, window, cx| match event {
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
                            if let Err(e) = config::load(this, window, cx) {
                                window.dispatch_action(Box::new(PushError::new(e)), cx);
                            }
                            cx.notify();
                        }
                        Event::LuaPrint(text) => {
                            let text = text.replace(['\r'], " ");
                            if !text.is_empty() {
                                window.dispatch_action(Box::new(PushLuaPrint::new(text)), cx);
                            }
                        }
                    });
                }
            }
        })
    }

    fn spawn_inital_session(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in_with_priority(Priority::High, window, async move |this, cx| {
            _ = this.update_in(cx, |this, window, cx| {
                if let Err(e) = config::load(this, window, cx) {
                    window.dispatch_action(Box::new(PushError::new(e)), cx);
                }
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
            });
        })
        .detach()
    }

    fn on_enter_rename(&mut self, _: &EnterRename, _: &mut Window, cx: &mut Context<Self>) {
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

    fn on_exec(&mut self, func: &ExecKeybind, window: &mut Window, cx: &mut Context<Self>) {
        let func = func.0.clone();
        if let Err(e) = config::with_env(self, window, cx, |this| func.call::<()>(this)) {
            window.dispatch_action(Box::new(PushError::new(e)), cx);
        }
    }

    fn on_open_sessions(&mut self, _: &OpenSessions, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.update(cx, |this, cx| {
            this.open(OverlayScreen::Sessions, window, cx);
        });
    }

    fn on_open_detached(&mut self, _: &OpenDetached, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.update(cx, |this, cx| {
            this.open(OverlayScreen::Detached, window, cx);
        });
    }

    fn on_open_palette(&mut self, _: &OpenPalette, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.update(cx, |this, cx| {
            this.open(OverlayScreen::Palette, window, cx);
        });
    }

    fn on_open_releases(&mut self, _: &OpenReleases, window: &mut Window, cx: &mut Context<Self>) {
        self.overlay.update(cx, |this, cx| {
            this.open(OverlayScreen::Releases, window, cx);
        });
    }

    fn on_copy(&mut self, _: &ClipboardCopy, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self
            .active_session()
            .and_then(|id| self.session_manager.session(id))
            && let Some(selection) = &session.selection
        {
            let text = selection.text(session.vt.screen());
            if !text.is_empty() {
                cx.write_to_clipboard(ClipboardItem::new_string(text));
            }
        }
    }

    fn extend_selection(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let Some(id) = self.selection_drag else {
            return;
        };
        let terminal = self.terminal.read(cx);
        let Some((col, row)) = terminal.mouse_to_cell(position, self) else {
            return;
        };
        let Some((_, geometry)) = self
            .tab_layouts(terminal.rows, terminal.cols)
            .into_iter()
            .find(|(session_id, _)| *session_id == id)
        else {
            return;
        };
        // Keep a drag in its original pane, even when the pointer crosses a divider.
        let col = col
            .saturating_sub(1)
            .saturating_sub(geometry.x)
            .min(geometry.cols.saturating_sub(1));
        let row = row
            .saturating_sub(1)
            .saturating_sub(geometry.y)
            .min(geometry.rows.saturating_sub(1));
        if let Some(session) = self.session_manager.session_mut(id)
            && let Some(selection) = &mut session.selection
        {
            selection.extend(session.vt.screen(), row, col);
            cx.notify();
        }
    }

    fn on_paste(&mut self, _: &ClipboardPaste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(session) = self.active_session()
            && let Some(item) = cx.read_from_clipboard()
            && let Some(text) = item.text()
        {
            self.session_manager.send_text(session, &text);
        }
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
            .bg(cx.theme().surface.opacity(0.15))
            .shadow(crate::ui::surface_shadow())
            .title(title)
            .autohide(autohide)
            .placement(Anchor::TopRight)
            .backdrop_blur(px(24.0))
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
}

impl Pyonji {
    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .size_full()
            .p_2()
            .child(
                div()
                    .size_full()
                    .track_focus(&self.focus_handle)
                    .key_context(Terminal::CONTEXT)
                    .on_action(cx.listener(Self::on_enter_rename))
                    .on_action(cx.listener(Self::on_enter_lua))
                    .on_action(cx.listener(Self::on_switch_tab))
                    .on_key_down(cx.listener(Self::handle_key_down))
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::handle_mouse_down))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::handle_mouse_up))
                    .on_mouse_up_out(
                        MouseButton::Left,
                        cx.listener(|this, event, window, cx| {
                            if this.selection_drag.is_some()
                                || this.terminal.read(cx).divider_drag.is_some()
                            {
                                this.handle_mouse_up(event, window, cx);
                            }
                        }),
                    )
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::handle_mouse_down))
                    .on_mouse_up(MouseButton::Right, cx.listener(Self::handle_mouse_up))
                    .on_mouse_down(MouseButton::Middle, cx.listener(Self::handle_mouse_down))
                    .on_mouse_up(MouseButton::Middle, cx.listener(Self::handle_mouse_up))
                    .on_mouse_move(cx.listener(Self::handle_mouse_move))
                    .on_scroll_wheel(cx.listener(Self::handle_scroll))
                    .on_action(cx.listener(Self::on_open_sessions))
                    .on_action(cx.listener(Self::on_open_detached))
                    .on_action(cx.listener(Self::on_open_releases))
                    .on_action(cx.listener(Self::on_paste))
                    .on_action(cx.listener(Self::on_copy))
                    .child(self.terminal.clone())
                    .children(Root::render_dialog_layer(window, cx)),
            )
            .child(self.status_bar.clone())
    }
}

impl Render for Pyonji {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        v_flex()
            .id("main-view")
            .on_action(cx.listener(Self::on_open_palette))
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .text_color(theme.text)
            .on_action(cx.listener(Self::on_error))
            .on_action(cx.listener(Self::on_warning))
            .on_action(cx.listener(Self::on_info))
            .on_action(cx.listener(Self::on_lua_print))
            .on_action(cx.listener(Self::on_exec))
            .child(Self::render_main(self, window, cx))
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

impl Pyonji {
    fn handle_key_down(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            return;
        }
        if event.is_held {
            return self.handle_key_repeat(event, window, cx);
        }

        if self.terminal.read(cx).resize_mode_held {
            let delta = match event.keystroke.key.as_str() {
                "left" => Some((SplitDirection::Vertical, -1)),
                "right" => Some((SplitDirection::Vertical, 1)),
                "up" => Some((SplitDirection::Horizontal, -1)),
                "down" => Some((SplitDirection::Horizontal, 1)),
                _ => None,
            };
            if let Some((direction, delta)) = delta {
                self.terminal.update(cx, |this, _| {
                    this.resize_mode_held = true;
                });
                self.resize_active_pane(direction, delta, cx);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }

        let Some(active_session) = self.active_session() else {
            return;
        };
        let reset_scrollback = self
            .session_manager
            .session_mut(active_session)
            .is_some_and(TerminalSession::reset_scrollback);
        let consumed = self
            .session_manager
            .session_mut(active_session)
            .is_some_and(|session| session.handle_key_down(event));
        if consumed {
            window.prevent_default();
            cx.stop_propagation();
        }
        if reset_scrollback {
            cx.notify();
        }
    }

    fn handle_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.focus_handle.is_focused(window) {
            window.focus(&self.focus_handle, cx);
        }
        if event.button == MouseButton::Left {
            self.selection_drag = None;
        }
        let terminal = self.terminal.clone();
        // Divider drags start on left press, like before — they take over the
        // gesture so the press never reaches the pane underneath.
        let (x, y) = terminal.read(cx).pt_to_term(event.position);
        if event.button == MouseButton::Left
            && let Some(divider) = terminal.read(cx).divider_hit_test(x, y, self)
        {
            let drag = DividerDrag {
                path: divider.path,
                direction: divider.direction,
            };
            self.resize_dragged_divider(&drag, x, y, cx);
            terminal.update(cx, |this, cx| {
                this.divider_drag = Some(drag);
                cx.notify();
            });
            return;
        }
        let Some((col, row)) = terminal.read(cx).mouse_to_cell(event.position, self) else {
            return;
        };
        let Some((session_id, col, row)) = terminal.read(cx).pane_at(col, row, self) else {
            return;
        };
        self.set_active_session(session_id);
        let local = event.modifiers.shift
            || self
                .session_manager
                .session(session_id)
                .is_some_and(|session| {
                    session.vt.screen().mouse_protocol_mode() == vt100::MouseProtocolMode::None
                });
        if local && event.button == MouseButton::Left {
            if let Some(session) = self.session_manager.session_mut(session_id) {
                session.selection = Some(Selection::new(session.vt.screen(), row - 1, col - 1));
                self.selection_drag = Some(session_id);
                cx.notify();
            }
            return;
        }
        if let Some(session) = self.session_manager.session_mut(session_id) {
            if !session.uses_local_scrollback() {
                session.reset_scrollback();
            }
            session.handle_mouse_down(event.button, &event.modifiers, col, row);
        }
        cx.notify();
    }

    fn handle_mouse_up(&mut self, event: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        if event.button == MouseButton::Left && self.selection_drag.is_some() {
            self.extend_selection(event.position, cx);
            self.selection_drag = None;
            return;
        }
        if self
            .terminal
            .update(cx, |this, _| this.divider_drag.take().is_some())
        {
            return;
        }
        let Some((col, row)) = self.terminal.read(cx).mouse_to_cell(event.position, self) else {
            return;
        };
        let Some((session_id, col, row)) = self.terminal.read(cx).pane_at(col, row, self) else {
            return;
        };
        if let Some(session) = self.session_manager.session_mut(session_id) {
            session.handle_mouse_up(event.button, &event.modifiers, col, row);
        }
    }

    fn handle_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.pressed_button.is_none() {
            return;
        }
        if let Some(drag) = self.terminal.read(cx).divider_drag.clone() {
            let (x, y) = self.terminal.read(cx).pt_to_term(event.position);
            self.resize_dragged_divider(&drag, x, y, cx);
            return;
        }
        if self.selection_drag.is_some() {
            self.extend_selection(event.position, cx);
            return;
        }
        let Some((col, row)) = self.terminal.read(cx).mouse_to_cell(event.position, self) else {
            return;
        };
        let Some((session_id, col, row)) = self.terminal.read(cx).pane_at(col, row, self) else {
            return;
        };
        if let Some(session) = self.session_manager.session_mut(session_id) {
            session.handle_mouse_move(&event.modifiers, col, row);
        }
    }

    fn take_wheel_steps(&mut self, delta_lines: f32) -> i32 {
        let total = self.wheel_remainder + delta_lines;
        let whole = if total > 0.0 {
            total.floor() as i32
        } else if total < 0.0 {
            total.ceil() as i32
        } else {
            0
        };
        self.wheel_remainder = total - whole as f32;
        whole
    }

    fn resize_dragged_divider(
        &mut self,
        drag: &DividerDrag,
        x: f32,
        y: f32,
        cx: &mut Context<Self>,
    ) {
        if self
            .terminal
            .read(cx)
            .resize_dragged_divider(drag, x, y, self)
        {
            if let Some(tab) = self.current_tab {
                self.resize_tab(tab, cx);
            }
            self.wheel_remainder = 0.0;
            cx.notify();
        }
    }

    fn handle_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some((col, row)) = self.terminal.read(cx).mouse_to_cell(event.position, self) else {
            return;
        };
        let Some((session_id, col, row)) = self.terminal.read(cx).pane_at(col, row, self) else {
            return;
        };
        let line_height = self.font_size * self.line_height;
        let lines = match event.delta {
            ScrollDelta::Lines(lines) => lines.y,
            ScrollDelta::Pixels(pixels) => {
                let scale = self.terminal.read(cx).scale.max(f32::EPSILON);
                f32::from(pixels.y) / (line_height / scale)
            }
        };
        let uses_local_scrollback = self
            .session_manager
            .session(session_id)
            .is_some_and(TerminalSession::uses_local_scrollback);
        let whole_lines = if uses_local_scrollback {
            self.take_wheel_steps(lines)
        } else {
            self.wheel_remainder = 0.0;
            0
        };
        if let Some(session) = self.session_manager.session_mut(session_id) {
            if uses_local_scrollback {
                if whole_lines != 0 && session.scroll_scrollback(whole_lines) {
                    cx.notify();
                }
            } else {
                let reset_scrollback = session.reset_scrollback();
                session.handle_mouse_wheel(lines, &event.modifiers, col, row);
                if reset_scrollback {
                    cx.notify();
                }
            }
        }
        self.extend_selection(event.position, cx);
    }

    /*fn handle_key_up(&mut self, key: &str) -> bool {
        if key.to_lowercase() == self.action.key {
            self.resize_mode_held = false;
            if self.resize_mode_used {
                self.action_mode = false;
            }
            self.resize_mode_used = false;
            true
        } else {
            false
        }
    }*/

    fn handle_key_repeat(
        &mut self,
        event: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.terminal.read(cx).resize_mode_held {
            let delta = match event.keystroke.key.as_str() {
                "left" => Some((SplitDirection::Vertical, -1)),
                "right" => Some((SplitDirection::Vertical, 1)),
                "up" => Some((SplitDirection::Horizontal, -1)),
                "down" => Some((SplitDirection::Horizontal, 1)),
                _ => None,
            };
            if let Some((direction, delta)) = delta {
                self.terminal.update(cx, |this, _| {
                    this.resize_mode_held = true;
                });
                self.resize_active_pane(direction, delta, cx);
                window.prevent_default();
                cx.stop_propagation();
                cx.notify();
                return;
            }
        }
        if let Some(active_session) = self.active_session() {
            let consumed = self
                .session_manager
                .session_mut(active_session)
                .is_some_and(|session| session.handle_key_down(event));
            if consumed {
                window.prevent_default();
                cx.stop_propagation();
            }
        }
    }

    pub fn create_session(
        &mut self,
        dir: Option<&Path>,
        tab: Option<usize>,
        parent: Option<SessionId>,
        cx: &mut Context<Self>,
    ) -> Result<SessionId> {
        let slot = match tab.or(self.current_tab) {
            Some(slot) if slot < self.tabs.len() => slot,
            _ => return Err(anyhow!("tab index out of range")),
        };
        let cwd = self.default_cwd.clone();
        let owned_dir = dir.map(PathBuf::from);
        let path = owned_dir.as_deref().or(cwd.as_deref());
        let id = self
            .session_manager
            .create_session(20, 80, path)
            .map_err(mlua::Error::external)?;
        if self.tabs[slot].is_none() {
            self.tabs[slot] = Some(TerminalTab::new(id));
        } else {
            let placed = self.tabs[slot]
                .as_mut()
                .map(
                    |tab| match parent.filter(|id| tab.sessions().contains(id)) {
                        Some(at) => tab.split_on(at, SplitDirection::Vertical, id),
                        None => tab.split_active(SplitDirection::Vertical, id),
                    },
                )
                .unwrap_or(false);
            if !placed {
                self.session_manager.remove_session(id);
                return Err(anyhow!("failed to place session"));
            }
        }
        self.resize_tab(slot, cx);
        Ok(id)
    }

    pub fn create_remote_session(
        &mut self,
        connection: &SshConnection,
        tab: Option<usize>,
        parent: Option<SessionId>,
        cx: &mut Context<Self>,
    ) -> Result<SessionId> {
        let slot = match tab.or(self.current_tab) {
            Some(slot) if slot < self.tabs.len() => slot,
            _ => return Err(anyhow!("tab index out of range")),
        };
        let id = self
            .session_manager
            .create_remote_session(20, 80, connection)
            .map_err(mlua::Error::external)?;
        if self.tabs[slot].is_none() {
            self.tabs[slot] = Some(TerminalTab::new(id));
        } else {
            let placed = self.tabs[slot]
                .as_mut()
                .map(
                    |tab| match parent.filter(|id| tab.sessions().contains(id)) {
                        Some(at) => tab.split_on(at, SplitDirection::Vertical, id),
                        None => tab.split_active(SplitDirection::Vertical, id),
                    },
                )
                .unwrap_or(false);
            if !placed {
                self.session_manager.remove_session(id);
                return Err(anyhow!("failed to place session"));
            }
        }
        self.resize_tab(slot, cx);
        Ok(id)
    }

    pub fn split_active(
        &mut self,
        direction: SplitDirection,
        cx: &mut Context<Self>,
    ) -> Result<SessionId> {
        let current = self.current_tab.ok_or_else(|| anyhow!("no active tab"))?;
        if self.active_session().is_none() {
            return Err(anyhow!("no active session to split"));
        }
        let id = self
            .session_manager
            .create_session(20, 80, self.default_cwd.as_deref())?;
        let placed = self.tabs[current]
            .as_mut()
            .is_some_and(|tab| tab.split_active(direction, id));
        if !placed {
            self.session_manager.remove_session(id);
            return Err(anyhow!("failed to split the active pane"));
        }
        self.resize_tab(current, cx);
        cx.notify();
        Ok(id)
    }

    pub fn attach_session(
        &mut self,
        session: SessionId,
        target: Option<usize>,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.session_manager.session(session).is_none() {
            return false;
        }
        let Some(target) = target.or(self.current_tab) else {
            return false;
        };
        let source_tabs = self
            .tabs
            .iter()
            .enumerate()
            .filter_map(|(index, tab)| {
                (index != target
                    && tab
                        .as_ref()
                        .is_some_and(|tab| tab.sessions().contains(&session)))
                .then_some(index)
            })
            .collect::<Vec<_>>();
        if !attach_session_to_tabs(&mut self.tabs, &mut self.detached_sessions, session, target) {
            return false;
        }
        self.current_tab = Some(target);
        self.wheel_remainder = 0.0;
        for source in source_tabs {
            self.resize_tab(source, cx);
        }
        self.resize_tab(target, cx);
        cx.notify();
        true
    }

    pub fn detach_session(&mut self, session: SessionId, cx: &mut Context<Self>) -> bool {
        if self.session_manager.session(session).is_none() {
            return false;
        }
        if !self.detached_sessions.contains(&session) {
            self.detached_sessions.push(session);
        }
        let mut emptied_current = false;
        for (index, tab) in self.tabs.iter_mut().enumerate() {
            let Some(state) = tab.as_mut() else { continue };
            if state.remove_session(session) && state.is_empty() {
                *tab = None;
                emptied_current |= Some(index) == self.current_tab;
            }
        }
        if let Some(current) = self.current_tab {
            if emptied_current {
                self.switch_to_previous_live_tab_or_stay(current, cx);
            } else {
                self.resize_tab(current, cx);
            }
        }
        self.wheel_remainder = 0.0;
        cx.notify();
        true
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

    pub fn switch_tab(&mut self, tab: usize, cx: &mut Context<Self>) -> bool {
        let terminal = self.terminal.read(cx);
        if tab >= self.tabs.len() {
            return false;
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
                    return false;
                }
            };
            self.tabs[tab] = Some(TerminalTab::new(id));
        }

        self.current_tab = Some(tab);
        self.wheel_remainder = 0.0;
        self.resize_tab(tab, cx);
        //self.update_ime_cursor_area();
        cx.notify();
        true
    }

    pub fn next_tab(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(curr) = self.current_tab else {
            return false;
        };
        self.switch_tab(curr + 1, cx)
    }

    pub fn prev_tab(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(curr) = self.current_tab else {
            return false;
        };
        self.switch_tab(curr.saturating_sub(1), cx)
    }

    pub fn move_session(
        &mut self,
        from: Option<usize>,
        to: usize,
        session: SessionId,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(from) = from.or(self.current_tab) else {
            return false;
        };
        if !move_session_between_tabs(&mut self.tabs, from, to, session) {
            return false;
        }
        self.current_tab = Some(to);
        self.wheel_remainder = 0.0;
        self.resize_tab(from, cx);
        self.resize_tab(to, cx);
        cx.notify();
        true
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

    pub fn resize_active_pane(
        &mut self,
        direction: SplitDirection,
        delta_first: i16,
        cx: &mut Context<Self>,
    ) {
        let (rows, cols) = {
            let terminal = self.terminal.read(cx);
            (terminal.rows, terminal.cols)
        };
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols,
            rows,
        };
        let Some(current_tab) = self.current_tab else {
            return;
        };
        let Some(tab) = self.tabs[current_tab].as_mut() else {
            return;
        };
        if !tab.resize_active_split(area, direction, delta_first) {
            return;
        }
        self.wheel_remainder = 0.0;
        self.resize_tab(current_tab, cx);
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

    pub fn set_active_session(&mut self, id: SessionId) {
        let Some(tab) = self.current_tab else {
            return;
        };
        let Some(tab) = self.tabs[tab].as_mut() else {
            return;
        };
        if tab.set_active_session(id) {
            self.wheel_remainder = 0.0;
        }
    }
}

/// Keep the existing owner until the destination has accepted the session.
fn attach_session_to_tabs(
    tabs: &mut [Option<TerminalTab>],
    detached_sessions: &mut Vec<SessionId>,
    session: SessionId,
    target: usize,
) -> bool {
    let Some(destination) = tabs.get_mut(target) else {
        return false;
    };
    if let Some(tab) = destination {
        let placed = if tab.sessions().contains(&session) {
            tab.set_active_session(session)
        } else {
            tab.split_active(SplitDirection::Vertical, session)
        };
        if !placed {
            return false;
        }
    } else {
        *destination = Some(TerminalTab::new(session));
    }
    for (index, tab) in tabs.iter_mut().enumerate() {
        if index == target {
            continue;
        }
        let Some(state) = tab.as_mut() else {
            continue;
        };
        if state.remove_session(session) && state.is_empty() {
            *tab = None;
        }
    }
    detached_sessions.retain(|detached| *detached != session);
    true
}

#[cfg(test)]
mod session_attachment_tests {
    use super::{SessionId, SplitDirection, TerminalTab, attach_session_to_tabs};

    fn session(id: u64) -> SessionId {
        id.to_string().parse().unwrap()
    }

    #[test]
    fn attaching_a_detached_session_to_an_empty_tab_reuses_its_id() {
        let id = session(4);
        let mut tabs = [None, None];
        let mut detached = vec![id];

        assert!(attach_session_to_tabs(&mut tabs, &mut detached, id, 1));
        assert!(tabs[0].is_none());
        assert_eq!(tabs[1].as_ref().unwrap().sessions(), [id]);
        assert_eq!(tabs[1].as_ref().unwrap().active_session(), Some(id));
        assert!(detached.is_empty());
    }

    #[test]
    fn attaching_to_an_occupied_tab_preserves_panes_and_focuses_the_session() {
        let first = session(1);
        let second = session(2);
        let detached_id = session(3);
        let mut destination = TerminalTab::new(first);
        destination.split_active(SplitDirection::Horizontal, second);
        let mut tabs = [Some(destination)];
        let mut detached = vec![detached_id];

        assert!(attach_session_to_tabs(
            &mut tabs,
            &mut detached,
            detached_id,
            0
        ));
        assert_eq!(
            tabs[0].as_ref().unwrap().sessions(),
            [first, second, detached_id]
        );
        assert_eq!(
            tabs[0].as_ref().unwrap().active_session(),
            Some(detached_id)
        );
        assert!(detached.is_empty());
    }

    #[test]
    fn attaching_an_existing_session_moves_it_once_and_preserves_other_panes() {
        let id = session(1);
        let remaining = session(2);
        let destination_id = session(3);
        let mut source = TerminalTab::new(id);
        source.split_active(SplitDirection::Vertical, remaining);
        let mut tabs = [Some(source), Some(TerminalTab::new(destination_id))];
        let mut detached = vec![];

        assert!(attach_session_to_tabs(&mut tabs, &mut detached, id, 1));
        assert_eq!(tabs[0].as_ref().unwrap().sessions(), [remaining]);
        assert_eq!(tabs[1].as_ref().unwrap().sessions(), [destination_id, id]);
        assert!(attach_session_to_tabs(&mut tabs, &mut detached, id, 1));
        assert_eq!(tabs[1].as_ref().unwrap().sessions(), [destination_id, id]);
        assert_eq!(tabs[1].as_ref().unwrap().active_session(), Some(id));
        assert!(attach_session_to_tabs(
            &mut tabs,
            &mut detached,
            remaining,
            1
        ));
        assert!(tabs[0].is_none());
        assert_eq!(
            tabs[1].as_ref().unwrap().sessions(),
            [destination_id, id, remaining]
        );
    }

    #[test]
    fn an_invalid_target_or_failed_placement_preserves_session_ownership() {
        let attached = session(1);
        let detached_id = session(2);
        let mut unusable_destination = TerminalTab::new(session(3));
        unusable_destination.remove_session(session(3));
        let mut tabs = [Some(TerminalTab::new(attached)), Some(unusable_destination)];
        let mut detached = vec![detached_id];

        assert!(!attach_session_to_tabs(
            &mut tabs,
            &mut detached,
            detached_id,
            2
        ));
        assert!(!attach_session_to_tabs(
            &mut tabs,
            &mut detached,
            detached_id,
            1
        ));
        assert!(!attach_session_to_tabs(
            &mut tabs,
            &mut detached,
            attached,
            1
        ));
        assert_eq!(tabs[0].as_ref().unwrap().sessions(), [attached]);
        assert_eq!(tabs[0].as_ref().unwrap().active_session(), Some(attached));
        assert!(tabs[1].as_ref().unwrap().is_empty());
        assert_eq!(detached, [detached_id]);
    }
}

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct SwitchTab(usize);

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct ExecKeybind(LuaFunction);

unsafe impl Send for ExecKeybind {}

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

#[derive(serde::Serialize)]
pub struct Theme {
    // Base layers
    pub background: Rgba,
    pub surface: Rgba,
    pub surface_elevated: Rgba,

    // Text
    pub text: Rgba,
    pub text_muted: Rgba,
    pub text_disabled: Rgba,

    // Tabs / list items
    pub selected: Rgba,
    pub unselected: Rgba,
    pub hovered: Rgba,
    pub selected_border: Rgba,
    pub unselected_border: Rgba,

    // Accent
    pub accent: Rgba,
    pub accent_muted: Rgba,

    // Terminal-specific
    pub cursor: Rgba,
    pub cursor_text: Rgba,
    pub selection: Rgba,
    pub search_match: Rgba,
    pub search_match_active: Rgba,
    pub scrollbar: Rgba,
    pub scrollbar_hover: Rgba,
    pub split_divider: Rgba,
    pub split_divider_active: Rgba,

    // Semantic status
    pub success: Rgba,
    pub warning: Rgba,
    pub error: Rgba,
    pub info: Rgba,

    // Overlays / popups
    pub overlay_backdrop: Rgba,
    pub tooltip_background: Rgba,
    pub border: Rgba,
    pub focus_ring: Rgba,
}

impl Theme {
    pub fn init(cx: &mut App) {
        cx.set_global(Theme::new());
    }

    #[allow(clippy::eq_op)]
    fn new() -> Self {
        Self {
            // Base layers (kept your background)
            background: Rgba::new(24.0 / 255.0, 24.0 / 255.0, 24.0 / 255.0, 1.0),
            surface: Rgba::new(32.0 / 255.0, 32.0 / 255.0, 32.0 / 255.0, 1.0),
            surface_elevated: Rgba::new(42.0 / 255.0, 42.0 / 255.0, 42.0 / 255.0, 1.0),

            // Text
            text: Rgba::new(220.0 / 255.0, 220.0 / 255.0, 220.0 / 255.0, 1.0),
            text_muted: Rgba::new(160.0 / 255.0, 160.0 / 255.0, 160.0 / 255.0, 1.0),
            text_disabled: Rgba::new(90.0 / 255.0, 90.0 / 255.0, 90.0 / 255.0, 1.0),

            // Tabs / list items
            selected: Rgba::new(44.0 / 255.0, 44.0 / 255.0, 44.0 / 255.0, 1.0),
            unselected: Rgba::new(28.0 / 255.0, 28.0 / 255.0, 28.0 / 255.0, 1.0),
            hovered: Rgba::new(36.0 / 255.0, 36.0 / 255.0, 36.0 / 255.0, 1.0),
            selected_border: Rgba::new(201.0 / 255.0, 167.0 / 255.0, 232.0 / 255.0, 1.0),
            unselected_border: Rgba::new(48.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 1.0),

            // Accent (one hue, used everywhere emphasis is needed)
            accent: Rgba::new(201.0 / 255.0, 167.0 / 255.0, 232.0 / 255.0, 1.0),
            accent_muted: Rgba::new(201.0 / 255.0, 167.0 / 255.0, 232.0 / 255.0, 0.25),

            // Terminal-specific
            cursor: Rgba::new(220.0 / 255.0, 220.0 / 255.0, 220.0 / 255.0, 1.0),
            cursor_text: Rgba::new(24.0 / 255.0, 24.0 / 255.0, 24.0 / 255.0, 1.0),
            selection: Rgba::new(201.0 / 255.0, 167.0 / 255.0, 232.0 / 255.0, 0.3),
            search_match: Rgba::new(224.0 / 255.0, 175.0 / 255.0, 104.0 / 255.0, 0.4),
            search_match_active: Rgba::new(224.0 / 255.0, 175.0 / 255.0, 104.0 / 255.0, 0.8),
            scrollbar: Rgba::new(255.0 / 255.0, 255.0 / 255.0, 255.0 / 255.0, 0.12),
            scrollbar_hover: Rgba::new(255.0 / 255.0, 255.0 / 255.0, 255.0 / 255.0, 0.25),
            split_divider: Rgba::new(48.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 1.0),
            split_divider_active: Rgba::new(201.0 / 255.0, 167.0 / 255.0, 232.0 / 255.0, 1.0),

            // Semantic status (bell, exit codes, warnings, etc.)
            success: Rgba::new(158.0 / 255.0, 206.0 / 255.0, 106.0 / 255.0, 1.0),
            warning: Rgba::new(224.0 / 255.0, 175.0 / 255.0, 104.0 / 255.0, 1.0),
            error: Rgba::new(247.0 / 255.0, 118.0 / 255.0, 142.0 / 255.0, 1.0),
            info: Rgba::new(125.0 / 255.0, 207.0 / 255.0, 255.0 / 255.0, 1.0),

            // Overlays / popups
            overlay_backdrop: Rgba::new(0.0, 0.0, 0.0, 0.5),
            tooltip_background: Rgba::new(48.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 1.0),
            border: Rgba::new(48.0 / 255.0, 48.0 / 255.0, 48.0 / 255.0, 1.0),
            focus_ring: Rgba::new(201.0 / 255.0, 167.0 / 255.0, 232.0 / 255.0, 1.0),
        }
    }
}

#[cfg(test)]
mod theme_readability_tests {
    use super::Theme;
    use gpui::Rgba;

    fn luminance(color: Rgba) -> f32 {
        let channel = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * channel(color.red) + 0.7152 * channel(color.green) + 0.0722 * channel(color.blue)
    }

    #[test]
    fn supporting_text_and_search_matches_remain_readable_on_ui_surfaces() {
        let theme = Theme::new();
        for background in [
            theme.background,
            theme.surface,
            theme.surface_elevated,
            theme.selected,
            theme.hovered,
        ] {
            for foreground in [
                theme.text,
                theme.text_muted,
                theme.accent,
                theme.search_match,
                theme.search_match_active,
            ] {
                // Search match tokens are rendered as opaque text, with an underline.
                let contrast = (luminance(foreground) + 0.05) / (luminance(background) + 0.05);
                assert!(contrast >= 4.5, "text contrast {contrast} is below 4.5:1");
            }
        }
    }
}
