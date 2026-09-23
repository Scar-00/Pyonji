use crate::Pyonji;
use crate::renderer::{Pane as RendererPane, *};
use crate::terminal::{Divider, PaneGeometry, PanePathStep, SessionId, SplitDirection};

use gpui::*;
use gpui_base::{ElementExt as _};
use gpui_wgpu::{WgpuContextHandle, WgpuRenderTarget};

pub struct Terminal {
    pyonji: WeakEntity<Pyonji>,

    pub context: Option<WgpuContextHandle>,
    target: Option<WgpuRenderTarget>,
    pub renderer: Option<Renderer>,

    //metrics
    pub scale: f32,
    bounds: Bounds<Pixels>,
    pub rows: u16,
    pub cols: u16,

    ime_preedit: Option<String>,
    pub resize_mode_held: bool,
    pub divider_drag: Option<DividerDrag>,
}

impl Terminal {
    pub const CONTEXT: &str = "terminal";

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
            resize_mode_held: false,
            divider_drag: None,
        }
    }

    pub fn pt_to_term(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.bounds;
        (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        )
    }

    pub fn cell_metrics(&self, font_size: f32, line_height: f32) -> Option<(f32, f32)> {
        let scale = self.scale.max(f32::EPSILON);
        let cell_width = (font_size / 2.0) / scale;
        let line_height = line_height / scale;
        (cell_width > 0.0 && line_height > 0.0).then_some((cell_width, line_height))
    }

    pub fn mouse_to_cell(&self, position: Point<Pixels>, pyonji: &Pyonji) -> Option<(u16, u16)> {
        let font_size = pyonji.font_size;
        let line_height = pyonji.line_height * font_size;

        let (cell_width, line_height) = self.cell_metrics(font_size, line_height)?;
        let (dx, dy) = self.pt_to_term(position);
        let col = ((dx.max(0.0) / cell_width).floor() as i32 + 1)
            .clamp(1, i32::from(self.cols.max(1))) as u16;
        let row = ((dy.max(0.0) / line_height).floor() as i32 + 1)
            .clamp(1, i32::from(self.rows.max(1))) as u16;
        Some((col, row))
    }

    pub fn divider_hit_test(&self, x: f32, y: f32, pyonji: &Pyonji) -> Option<Divider> {
        let font_size = pyonji.font_size;
        let line_height = pyonji.line_height * font_size;

        let (cell_width, line_height) = self.cell_metrics(font_size, line_height)?;
        hit_divider(
            &pyonji.tab_dividers(self.rows, self.cols),
            x,
            y,
            cell_width,
            line_height,
        )
    }

    pub fn cursor_to_grid_position(&self, x: f32, y: f32, cx: &mut App) -> Option<(f32, f32)> {
        let pyonji = self.pyonji.upgrade()?;

        let font_size = pyonji.read(cx).font_size;
        let line_height = pyonji.read(cx).line_height * font_size;
        let (cell_width, line_height) = self.cell_metrics(font_size, line_height)?;
        let col = (x.max(0.0) / cell_width).clamp(0.0, f32::from(self.cols));
        let row = (y.max(0.0) / line_height).clamp(0.0, f32::from(self.rows));
        Some((col, row))
    }

    pub fn resize_dragged_divider(&mut self, drag: &DividerDrag, x: f32, y: f32, cx: &mut App) {
        let Some((col, row)) = self.cursor_to_grid_position(x, y, cx) else {
            return;
        };
        let position = match drag.direction {
            SplitDirection::Vertical => col,
            SplitDirection::Horizontal => row,
        };
        let Some(pyonji) = self.pyonji.upgrade() else {
            return;
        };
        let Some(tab) = pyonji.read(cx).current_tab else {
            return;
        };
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols: self.cols,
            rows: self.rows,
        };
        pyonji.update(cx, |this, _| {
            let Some(tab) = this.tabs[tab].as_mut() else {
                return;
            };
            tab.resize_split_by_position(area, &drag.path, drag.direction, position);
        });
    }

    pub fn pane_at(&self, col: u16, row: u16, py: &Pyonji) -> Option<(SessionId, u16, u16)> {
        for (session_id, geometry) in py.tab_layouts(self.rows, self.cols) {
            if !geometry.contains_global_cell(col, row) {
                continue;
            }
            let (col, row) = geometry.local_cell(col, row);
            return Some((session_id, col, row));
        }
        None
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



pub fn hit_divider(
    dividers: &[Divider],
    x: f32,
    y: f32,
    cell_width: f32,
    line_height: f32,
) -> Option<Divider> {
    const HIT_SLOP: f32 = 6.0;
    for divider in dividers {
        match divider.direction {
            SplitDirection::Vertical => {
                let line_x = cell_width * f32::from(divider.x);
                let min_y = line_height * f32::from(divider.y);
                let max_y = line_height * f32::from(divider.y + divider.rows);
                if (x - line_x).abs() <= HIT_SLOP
                    && y >= min_y - HIT_SLOP
                    && y <= max_y + HIT_SLOP
                {
                    return Some(divider.clone());
                }
            }
            SplitDirection::Horizontal => {
                let line_y = line_height * f32::from(divider.y);
                let min_x = cell_width * f32::from(divider.x);
                let max_x = cell_width * f32::from(divider.x + divider.cols);
                if (y - line_y).abs() <= HIT_SLOP
                    && x >= min_x - HIT_SLOP
                    && x <= max_x + HIT_SLOP
                {
                    return Some(divider.clone());
                }
            }
        }
    }
    None
}

#[derive(Debug, Clone)]
pub struct DividerDrag {
    pub path: Vec<PanePathStep>,
    pub direction: SplitDirection,
}
