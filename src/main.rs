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

mod pty;
mod terminal;
mod renderer;

use pty::Event;
use renderer::{*, Pane as RendererPane};
use terminal::{*, Tab as TerminalTab};

use std::{array, path::PathBuf, sync::Arc};

use async_channel::{Receiver, Sender};
use clap::Parser;
use gpui::{prelude::*, *};
use gpui_base::*;
use gpui_component::{Root, ThemeMode, WindowExt, notification::Notification};
use gpui_wgpu::WgpuContextHandle;
use smallvec::{SmallVec, smallvec};
use tracing_subscriber::prelude::*;
use mlua::prelude::*;

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

    gpui_platform::application().with_assets(gpui_component_assets::Assets).run(|cx| {
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

    //jobs
    _event_loop_task: Task<()>,
    _subscriptions: SmallVec<[Subscription; 4]>,

    //config
    font_family: Option<String>,
    font_size: f32,
    line_height: f32,
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

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let cli = Cli::parse();
        let lua = Lua::new();
        let (tx, rx) = async_channel::unbounded();

        let focus_handle = cx.focus_handle();
        window.focus(&focus_handle, cx);

        cx.bind_keys([
            KeyBinding::new("ctrl-a", PushError::new("test"), None),
        ]);

        //_ = lua.globals().set("py", LuaProxy(cx.weak_entity()));
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

            _event_loop_task: Self::spawn_event_loop(rx, window, cx),
            _subscriptions: smallvec![],

            font_family: None,
            font_size: 38.0,
            line_height: 1.1,
        }
    }

    fn spawn_event_loop(rx: Receiver<Event>, window: &mut Window, cx: &mut Context<Self>) -> Task<()> {
        cx.spawn_in(window, async move |this, cx| {
            loop {
                if let Ok(event) = rx.recv().await {
                    _ = this.update_in(cx, |this, _, cx| {
                        match event {
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
                                //config::load(self);
                                cx.notify();
                            }
                            Event::LuaPrint(text) => {}
                            Event::Exit => {
                                cx.quit();
                            }
                        }
                    }).unwrap();
                }
            }
        })
    }

    fn spawn_inital_session(window: &mut Window, cx: &mut Context<Self>) {
        cx.spawn_in(window, async move |this, cx| {
            _ = this.update_in(cx, |this, window, cx| {
                let res = this.session_manager.create_session(20, 80, this.cli.path.as_deref());
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
            }).unwrap()
        })
        .detach()
    }

    fn on_error(&mut self, error: &PushError, window: &mut Window, cx: &mut Context<Self>) {
        let note = Notification::new()
            .bg(cx.theme().background)
            .title("Error")
            .autohide(false)
            .placement(Anchor::TopRight)
            .message(error.string.clone());

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
        self.detached_sessions.retain(|detached| *detached != session);

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
        window.request_animation_frame();
        v_flex()
            .id("main-view")
            .track_focus(&self.focus_handle)
            .size_full()
            .bg(theme.background)
            .text_color(cx.theme().text)
            .on_action(cx.listener(Self::on_error))
            .child(
                v_flex()
                    .size_full()
                    .p_2()
                    .child(self.terminal.clone())
            )
            .children(Root::render_sheet_layer(window, cx))
            .children(Root::render_dialog_layer(window, cx))
            .children(Root::render_notification_layer(window, cx))
    }
}

/*struct LuaProxy(WeakEntity<Pyonji>);

impl LuaUserData for LuaProxy {
    fn add_methods<M: LuaUserDataMethods<Self>>(methods: &mut M) {
        methods.add_async_method("test", |_, this, _: ()| async move {
            println!("called test");
            Ok(())
        })
    }
}*/

struct Terminal {
    pyonji: WeakEntity<Pyonji>,

    context: Option<gpui_wgpu::WgpuContextHandle>,
    target: Option<gpui_wgpu::WgpuRenderTarget>,
    renderer: Option<Renderer>,

    //metrics
    scale: f32,
    bounds: Bounds<Pixels>,
    rows: u16,
    cols: u16,

    ime_preedit: Option<String>,
}

impl Terminal {
    pub fn new(pyonji: WeakEntity<Pyonji>, _cx: &mut Context<Self>) -> Self {
        Self {
            pyonji,

            context: None,
            target: None,
            renderer: None,

            scale: 0.0,
            bounds: Bounds::default(),
            rows: 20,
            cols: 80,

            ime_preedit: None,
        }
    }

    fn on_prepaint(&mut self, bounds: Bounds<Pixels>, _: &mut Window, cx: &mut Context<Self>) {
        if self.bounds != bounds {
            self.bounds = bounds;
            cx.notify();
        }
    }

    fn clear_gpu_resources(&mut self) {
        self.context = None;
        self.target = None;
        self.renderer = None;
    }

    fn ensure_context(&mut self, window: &mut Window) -> Option<WgpuContextHandle> {
        let context = WgpuContextHandle::from_window(window)?;
        if self
            .context
            .as_ref()
            .is_some_and(|previous| !context.is_same_device(previous))
        {
            self.clear_gpu_resources();
        }
        if context.device_lost() {
            self.clear_gpu_resources();
            return None;
        }
        self.context = Some(context.clone());
        Some(context)
    }

    fn sync_surface(&mut self, content_size: Size<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(context) = self.ensure_context(window) else {
            return;
        };
        let scale_factor = window.scale_factor();
        self.scale = scale_factor;
        let target_size = size(
            gpui::DevicePixels((f32::from(content_size.width) * scale_factor).round() as i32),
            gpui::DevicePixels((f32::from(content_size.height) * scale_factor).round() as i32),
        );
        let target_size = size(
            target_size.width.max(gpui::DevicePixels(1)),
            target_size.height.max(gpui::DevicePixels(1)),
        );

        match self.target.as_mut() {
            Some(target) => target.resize(&context, target_size),
            None => {
                self.target = Some(gpui_wgpu::WgpuRenderTarget::new(&context, target_size));
            }
        }

        let Some(target) = self.target.as_ref() else {
            return;
        };

        let Some(pyonji) = self.pyonji.upgrade() else {
            return;
        };

        if self.renderer.is_none() {
            let pyonji = pyonji.read(cx);
            let queue = context.queue().clone();
            let device = context.device().clone();
            self.renderer = Renderer::new(
                queue,
                device,
                target.format(),
                pyonji.font_family.as_deref(),
                pyonji.font_size,
                pyonji.font_size * pyonji.line_height,
            )
            .ok();
        }

        let font_size = pyonji.read(cx).font_size;
        let cols = ((target_size.width.0 as f32 / (font_size / 2.0)) as u16).max(1);
        let rows = ((target_size.height.0 as f32 / font_size * pyonji.read(cx).line_height) as u16).max(1);
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            pyonji.update(cx, |this, _| {
                for (id, geometry) in this.tab_layouts(cols, rows) {
                    this
                        .session_manager
                        .resize_session(id, geometry.rows, geometry.cols);
                }
            });
        }
    }

    fn paint(&mut self, cx: &mut Context<Self>) {
        let Some(pyonji) = self.pyonji.upgrade() else {
            println!("failed to upgrade py");
            return;
        };
        let pyonji = pyonji.read(cx);
        let Some(target) = self.target.as_ref() else {
            println!("target is none");
            return;
        };
        let panes = pyonji.tab_layouts(self.rows, self.cols);
        let active = pyonji.active_session();
        let dividers = pyonji.tab_dividers(self.rows, self.cols);
        let ime_preedit = self.ime_preedit(pyonji);

        let mut pane_data = Vec::with_capacity(panes.len());
        for (session_id, geometry) in panes {
            let Some(session) = pyonji.session_manager.session(session_id) else {
                continue;
            };
            pane_data.push(RendererPane {
                screen: session.vt.screen(),
                cursor_style: &session.cursor_style,
                geometry,
                is_active: Some(session_id) == active,
            });
        }
        let Some(renderer) = self.renderer.as_mut() else {
            return;
        };
        let target_size = target.size();
        let size = [target_size.width.0 as u32, target_size.height.0 as u32];
        _ = renderer.render(
            target.view(),
            &pane_data,
            &dividers,
            ime_preedit.as_ref(),
            size,
        );
    }

    fn ime_preedit(&self, pyonji: &Pyonji) -> Option<ImePreedit> {
        let text = self.ime_preedit.clone()?;
        let active_session = pyonji.active_session()?;
        let session = pyonji.session_manager.session(active_session)?;
        let (_, geometry) = pyonji
            .tab_layouts(self.rows, self.cols)
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;
        let (row, col) = session.vt.screen().cursor_position();

        Some(ImePreedit {
            text,
            geometry,
            row,
            col,
        })
    }
}

impl Render for Terminal {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        container_query(cx.processor(|this, size: Size<Pixels>, window, cx| {
            this.sync_surface(size, window, cx);
            this.paint(cx);
            div()
                .size_full()
                .on_prepaint(cx.processor(Self::on_prepaint))
                .children(this.target.as_ref().map(|target| {
                    target.surface().object_fit(gpui::ObjectFit::Fill).size_full()
                }))
        }))
        .size_full()
    }
}

#[derive(Action, Clone, PartialEq)]
#[action(no_json)]
struct PushError {
    string: SharedString,
}

impl PushError {
    fn new(v: impl ToString) -> Self {
        Self{ string: v.to_string().into() }
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
    text: Rgba,
}

impl Theme {
    pub fn init(cx: &mut App) {
        cx.set_global(Theme::new());
    }

    fn new() -> Self {
        Self {
            background: Rgba::new(24.0 / 255.0, 24.0 / 255.0, 24.0 / 255.0, 1.0),
            text: Rgba::new(0.9, 0.9, 0.9, 1.0),
        }
    }
}


