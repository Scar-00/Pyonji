//! Terminal UI component: viewport, GPU resources and pointer gestures.
//!
//! Owns *how* the terminal looks and where it is — grid size, font
//! metrics, wgpu context/target/renderer, layout bounds, scale, divider
//! drags and IME preedit — without owning *what* is open (see
//! `workspace::Workspace`). `Surface` composes this component and passes
//! the workspace in for layout, painting and hit-testing.

use gpui::{Bounds, Pixels, Point, Window, point, px, size};

use crate::{
    renderer::{ImePreedit, Pane, Renderer},
    terminal::{Divider, PaneGeometry, PanePathStep, SessionId, SplitDirection},
    workspace::Workspace,
};

pub const STATUS_BAR_ROWS: u16 = 0;

#[derive(Debug, Clone)]
pub struct DividerDrag {
    pub path: Vec<PanePathStep>,
    pub direction: SplitDirection,
}

pub struct TerminalState {
    pub rows: u16,
    pub cols: u16,
    pub font_size: f32,
    pub line_height: f32,
    pub font_family: Option<String>,
    /// Status-bar height as a multiple of `line_height` (1.0 = one row).
    /// Owned here alongside the other viewport metrics so Lua config and
    /// layout math stay synchronous; `render` syncs a copy into the bar.
    pub status_height: f32,
    /// Whether the status bar is hidden. Owned here because `terminal_rows`
    /// needs it on every layout path without a GPUI context in scope.
    pub status_hidden: bool,

    pub context: Option<gpui_wgpu::WgpuContextHandle>,
    pub target: Option<gpui_wgpu::WgpuRenderTarget>,
    pub renderer: Option<Renderer>,
    pub terminal_bounds: Option<Bounds<Pixels>>,
    pub terminal_scale: f32,

    pub divider_drag: Option<DividerDrag>,
    pub ime_preedit: Option<String>,
}

impl TerminalState {
    pub fn new() -> Self {
        Self {
            rows: 20,
            cols: 80,
            font_size: 24.0,
            line_height: 28.0,
            font_family: None,
            status_height: 1.0,
            status_hidden: false,
            context: None,
            target: None,
            renderer: None,
            terminal_bounds: None,
            terminal_scale: 1.0,
            divider_drag: None,
            ime_preedit: None,
        }
    }

    pub fn terminal_rows(&self) -> u16 {
        if self.rows > STATUS_BAR_ROWS && !self.status_hidden {
            self.rows - STATUS_BAR_ROWS
        } else {
            self.rows
        }
    }

    pub fn apply_font_metrics(&mut self) {
        if let Some(renderer) = self.renderer.as_mut() {
            renderer.set_font_metrics(self.font_size, self.line_height);
            if let Some(font_family) = self.font_family.as_deref() {
                renderer.set_font_family(font_family);
            }
            renderer.evict_glyphs();
        }
    }

    fn clear_gpu_resources(&mut self) {
        self.context = None;
        self.target = None;
        self.renderer = None;
    }

    fn ensure_context(&mut self, window: &mut Window) -> Option<gpui_wgpu::WgpuContextHandle> {
        let context = gpui_wgpu::WgpuContextHandle::from_window(window)?;
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

    /// Size the offscreen target from the surface element's layout bounds
    /// (content box, logical pixels) instead of the full window viewport,
    /// so the texture maps 1:1 to where GPUI composites it.
    pub fn sync_surface(
        &mut self,
        window: &mut Window,
        content_size: gpui::Size<Pixels>,
        workspace: &mut Workspace,
    ) {
        let Some(context) = self.ensure_context(window) else {
            return;
        };
        let scale_factor = window.scale_factor();
        self.terminal_scale = scale_factor;
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

        if self.renderer.is_none() {
            let queue = context.queue().clone();
            let device = context.device().clone();
            self.renderer = Renderer::new(
                queue,
                device,
                target.format(),
                self.font_family.as_deref(),
                self.font_size,
                self.line_height,
            )
            .ok();
        }

        let cols = ((target_size.width.0 as f32 / (self.font_size / 2.0)) as u16).max(1);
        let rows = ((target_size.height.0 as f32 / self.line_height) as u16).max(1);
        if cols != self.cols || rows != self.rows {
            self.cols = cols;
            self.rows = rows;
            let term_rows = self.terminal_rows();
            for (id, geometry) in workspace.tab_layouts(cols, term_rows) {
                workspace
                    .session_manager
                    .resize_session(id, geometry.rows, geometry.cols);
            }
        }
    }

    pub fn paint_terminal(&mut self, workspace: &Workspace) {
        let Some(target) = self.target.as_ref() else {
            return;
        };
        let term_rows = self.terminal_rows();
        let panes = workspace.tab_layouts(self.cols, term_rows);
        let active = workspace.active_session();
        let dividers = workspace.tab_dividers(self.cols, term_rows);
        let ime_preedit = self.ime_preedit(workspace);

        let mut pane_data = Vec::with_capacity(panes.len());
        for (session_id, geometry) in panes {
            let Some(session) = workspace.session_manager.session(session_id) else {
                continue;
            };
            pane_data.push(Pane {
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

    /// Current IME preedit positioned at the active cursor, for the wgpu
    /// preedit renderer.
    pub fn ime_preedit(&self, workspace: &Workspace) -> Option<ImePreedit> {
        let text = self.ime_preedit.clone()?;
        let active_session = workspace.active_session()?;
        let session = workspace.session_manager.session(active_session)?;
        let term_rows = self.terminal_rows();
        let (_, geometry) = workspace
            .tab_layouts(self.cols, term_rows)
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

    pub fn clear_ime(&mut self) {
        self.ime_preedit = None;
    }

    pub fn set_ime(&mut self, text: Option<String>) {
        self.ime_preedit = text;
    }

    /// Window-relative mouse position → logical pixels relative to the
    /// terminal texture origin.
    pub fn mouse_to_terminal(&self, position: Point<Pixels>) -> Option<(f32, f32)> {
        let bounds = self.terminal_bounds?;
        Some((
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        ))
    }

    pub fn cell_metrics(&self) -> Option<(f32, f32)> {
        let scale = self.terminal_scale.max(f32::EPSILON);
        let cell_width = (self.font_size / 2.0) / scale;
        let line_height = self.line_height / scale;
        (cell_width > 0.0 && line_height > 0.0).then_some((cell_width, line_height))
    }

    /// Window-relative mouse position → 1-based terminal cell.
    pub fn mouse_to_cell(&self, position: Point<Pixels>) -> Option<(u16, u16)> {
        let (cell_width, line_height) = self.cell_metrics()?;
        let (dx, dy) = self.mouse_to_terminal(position)?;
        let col = ((dx.max(0.0) / cell_width).floor() as i32 + 1)
            .clamp(1, i32::from(self.cols.max(1))) as u16;
        let rows = self.terminal_rows().max(1);
        let row = ((dy.max(0.0) / line_height).floor() as i32 + 1)
            .clamp(1, i32::from(rows)) as u16;
        Some((col, row))
    }

    /// Logical-pixel position → fractional grid position, for divider drags.
    pub fn cursor_to_grid_position(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let (cell_width, line_height) = self.cell_metrics()?;
        Some(grid_position(
            x,
            y,
            cell_width,
            line_height,
            self.cols,
            self.terminal_rows(),
        ))
    }

    /// Divider under a terminal-relative logical-pixel position, with the
    /// same 6px slop as the previous implementation so thin dividers stay
    /// grabbable.
    pub fn divider_hit_test(
        &self,
        x: f32,
        y: f32,
        workspace: &Workspace,
    ) -> Option<Divider> {
        let (cell_width, line_height) = self.cell_metrics()?;
        let term_rows = self.terminal_rows();
        hit_divider(
            &workspace.tab_dividers(self.cols, term_rows),
            x,
            y,
            cell_width,
            line_height,
        )
    }

    pub fn resize_dragged_divider(
        &mut self,
        drag: &DividerDrag,
        x: f32,
        y: f32,
        workspace: &mut Workspace,
    ) {
        let Some((col, row)) = self.cursor_to_grid_position(x, y) else {
            return;
        };
        let position = match drag.direction {
            SplitDirection::Vertical => col,
            SplitDirection::Horizontal => row,
        };
        if workspace.resize_split_by_position(
            &drag.path,
            drag.direction,
            position,
            self.cols,
            self.terminal_rows(),
        ) {
            // wheel state already reset inside workspace
        }
    }

    /// Candidate-window placement for IME: tracks the cursor cell.
    pub fn ime_bounds(
        &self,
        element_bounds: Bounds<Pixels>,
        workspace: &Workspace,
    ) -> Option<Bounds<Pixels>> {
        let (cell_width, line_height) = self.cell_metrics()?;
        let active_session = workspace.active_session()?;
        let session = workspace.session_manager.session(active_session)?;
        let term_rows = self.terminal_rows();
        let (_, geometry) = workspace
            .tab_layouts(self.cols, term_rows)
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;
        let (row, col) = session.vt.screen().cursor_position();
        let x = element_bounds.origin.x
            + px(cell_width * (f32::from(geometry.x) + f32::from(col)));
        let y = element_bounds.origin.y
            + px(line_height * (f32::from(geometry.y) + f32::from(row)));
        Some(Bounds::new(
            point(x, y),
            size(px(cell_width), px(line_height)),
        ))
    }

    pub fn take_drag(&mut self) -> Option<DividerDrag> {
        self.divider_drag.take()
    }
}

impl Default for TerminalState {
    fn default() -> Self {
        Self::new()
    }
}

/// Fractional grid position of a logical-pixel point, clamped to the grid.
pub fn grid_position(
    x: f32,
    y: f32,
    cell_width: f32,
    line_height: f32,
    cols: u16,
    rows: u16,
) -> (f32, f32) {
    let col = (x.max(0.0) / cell_width).clamp(0.0, f32::from(cols));
    let row = (y.max(0.0) / line_height).clamp(0.0, f32::from(rows));
    (col, row)
}

/// Divider under a terminal-relative point, with grab slop around the line.
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

/// Lookup helper for tests and mouse handling.
pub fn pane_session_at(
    workspace: &Workspace,
    cols: u16,
    rows: u16,
    col: u16,
    row: u16,
) -> Option<(SessionId, u16, u16)> {
    workspace.pane_at(col, row, cols, rows)
}

#[allow(dead_code)]
pub fn active_geometry(
    workspace: &Workspace,
    cols: u16,
    rows: u16,
    session: SessionId,
) -> Option<PaneGeometry> {
    workspace
        .tab_layouts(cols, rows)
        .into_iter()
        .find(|(id, _)| *id == session)
        .map(|(_, geometry)| geometry)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::{PanePathStep, SplitDirection};

    fn vertical_divider(x: u16, rows: u16) -> Divider {
        Divider {
            path: vec![PanePathStep::First],
            direction: SplitDirection::Vertical,
            x,
            y: 0,
            cols: 0,
            rows,
        }
    }

    fn horizontal_divider(y: u16, cols: u16) -> Divider {
        Divider {
            path: vec![PanePathStep::Second],
            direction: SplitDirection::Horizontal,
            x: 0,
            y,
            cols,
            rows: 0,
        }
    }

    #[test]
    fn divider_hit_test_finds_lines_within_slop() {
        let dividers = vec![vertical_divider(40, 24)];
        assert!(hit_divider(&dividers, 400.0, 100.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 406.0, 100.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 394.0, 100.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 420.0, 100.0, 10.0, 20.0).is_none());
        assert!(hit_divider(&dividers, 400.0, 24.0 * 20.0 + 7.0, 10.0, 20.0).is_none());

        let dividers = vec![horizontal_divider(12, 80)];
        assert!(hit_divider(&dividers, 100.0, 240.0, 10.0, 20.0).is_some());
        assert!(hit_divider(&dividers, 100.0, 200.0, 10.0, 20.0).is_none());
        assert!(hit_divider(&dividers, 80.0 * 10.0 + 7.0, 240.0, 10.0, 20.0).is_none());

        assert!(hit_divider(&[], 0.0, 0.0, 10.0, 20.0).is_none());
    }

    #[test]
    fn grid_position_clamps_to_grid() {
        assert_eq!(grid_position(25.0, 30.0, 10.0, 20.0, 80, 24), (2.5, 1.5));
        assert_eq!(grid_position(-5.0, -5.0, 10.0, 20.0, 80, 24), (0.0, 0.0));
        assert_eq!(
            grid_position(10000.0, 10000.0, 10.0, 20.0, 80, 24),
            (80.0, 24.0)
        );
    }
}
