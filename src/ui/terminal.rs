use crate::Pyonji;
use crate::renderer::{Pane as RendererPane, *};

use gpui::*;
use gpui_base::{ElementExt as _, StyledExt};
use gpui_wgpu::{WgpuContextHandle, WgpuRenderTarget};

pub struct Terminal {
    pyonji: WeakEntity<Pyonji>,

    context: Option<WgpuContextHandle>,
    target: Option<WgpuRenderTarget>,
    renderer: Option<Renderer>,

    //metrics
    scale: f32,
    bounds: Bounds<Pixels>,
    pub rows: u16,
    pub cols: u16,

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

    fn sync_surface(
        &mut self,
        content_size: Size<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(context) = self.ensure_context(window) else {
            return;
        };
        let scale_factor = window.scale_factor();
        self.scale = scale_factor;
        let target_size = size(
            DevicePixels((f32::from(content_size.width) * scale_factor).round() as i32),
            DevicePixels((f32::from(content_size.height) * scale_factor).round() as i32),
        );
        let target_size = size(
            target_size.width.max(gpui::DevicePixels(1)),
            target_size.height.max(gpui::DevicePixels(1)),
        );

        match self.target.as_mut() {
            Some(target) => target.resize(&context, target_size),
            None => {
                self.target = Some(WgpuRenderTarget::new(&context, target_size));
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
        let rows = ((target_size.height.0 as f32 / (font_size * pyonji.read(cx).line_height))
            as u16)
            .max(1);
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            pyonji.update(cx, |this, _| {
                for (id, geometry) in this.tab_layouts(rows, cols) {
                    this.session_manager
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
                    target
                        .surface()
                        .object_fit(gpui::ObjectFit::Fill)
                        .size_full()
                }))
            //.debug_red()
        }))
        .size_full()
    }
}
