use crate::Pyonji;
use crate::renderer::{Pane as RendererPane, *};
use crate::terminal::{Divider, PaneGeometry, PanePathStep, SessionId, SplitDirection};

use gpui::*;
use gpui_base::ElementExt as _;
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
    ime_session: Option<SessionId>,
    ime_selection: std::ops::Range<usize>,
    pub mouse_capture: Option<(SessionId, MouseButton)>,
    cell_width: f32,
    applied_metrics: Option<(Option<String>, f32, f32)>,
    blink_visible: bool,
    focused: bool,
    _blink_task: Task<()>,
    pub resize_mode_held: bool,
    pub divider_drag: Option<DividerDrag>,
}

impl Terminal {
    pub const CONTEXT: &str = "terminal";

    pub fn new(pyonji: WeakEntity<Pyonji>, cx: &mut Context<Self>) -> Self {
        let blink_task = cx.spawn(async |this, cx| {
            loop {
                smol::Timer::after(std::time::Duration::from_millis(500)).await;
                if this
                    .update(cx, |this, cx| {
                        this.blink_visible = !this.blink_visible;
                        let blinking = this.pyonji.upgrade().is_some_and(|py| {
                            let py = py.read(cx);
                            py.active_session()
                                .and_then(|id| py.session_manager.session(id))
                                .is_some_and(|session| session.cursor_blink)
                        });
                        if this.focused && blinking {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
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
            ime_session: None,
            ime_selection: 0..0,
            mouse_capture: None,
            cell_width: 19.0,
            applied_metrics: None,
            blink_visible: true,
            focused: false,
            _blink_task: blink_task,
            resize_mode_held: false,
            divider_drag: None,
        }
    }

    pub fn accepts_platform_key(&self, event: &KeyDownEvent) -> bool {
        if self.ime_preedit.is_some() {
            return true;
        }
        let mods = event.keystroke.modifiers;
        !mods.control
            && !mods.alt
            && !mods.platform
            && !mods.function
            && event
                .keystroke
                .key_char
                .as_ref()
                .is_some_and(|text| !text.chars().any(char::is_control))
    }

    pub fn cancel_input(&mut self, cx: &mut Context<Self>) -> Option<(SessionId, MouseButton)> {
        self.ime_preedit = None;
        self.ime_session = None;
        self.ime_selection = 0..0;
        self.divider_drag = None;
        self.resize_mode_held = false;
        cx.notify();
        self.mouse_capture.take()
    }

    pub fn pt_to_term(&self, position: Point<Pixels>) -> (f32, f32) {
        let bounds = self.bounds;
        (
            f32::from(position.x - bounds.origin.x),
            f32::from(position.y - bounds.origin.y),
        )
    }

    pub fn cell_metrics(&self, _font_size: f32, line_height: f32) -> Option<(f32, f32)> {
        let cell_width = self.cell_width;
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

    pub fn cursor_to_grid_position(
        &self,
        x: f32,
        y: f32,
        pyonji: &mut Pyonji,
    ) -> Option<(f32, f32)> {
        let font_size = pyonji.font_size;
        let line_height = pyonji.line_height * font_size;
        let (cell_width, line_height) = self.cell_metrics(font_size, line_height)?;
        let col = (x.max(0.0) / cell_width).clamp(0.0, f32::from(self.cols));
        let row = (y.max(0.0) / line_height).clamp(0.0, f32::from(self.rows));
        Some((col, row))
    }

    pub fn resize_dragged_divider(&self, drag: &DividerDrag, x: f32, y: f32, pyonji: &mut Pyonji) {
        let Some((col, row)) = self.cursor_to_grid_position(x, y, pyonji) else {
            return;
        };
        let position = match drag.direction {
            SplitDirection::Vertical => col,
            SplitDirection::Horizontal => row,
        };
        let Some(tab) = pyonji.current_tab else {
            return;
        };
        let area = PaneGeometry {
            x: 0,
            y: 0,
            cols: self.cols,
            rows: self.rows,
        };
        let Some(tab) = pyonji.tabs[tab].as_mut() else {
            return;
        };
        tab.resize_split_by_position(area, &drag.path, drag.direction, position);
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
        self.applied_metrics = None;
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

        let py = pyonji.read(cx);
        self.focused = py.focus_handle.is_focused(window);
        let font_size = py.font_size * scale_factor;
        let line_height = font_size * py.line_height;
        let metrics = (py.font_family.clone(), font_size, line_height);
        if self.renderer.is_none() {
            self.renderer = Renderer::new(
                context.queue().clone(),
                context.device().clone(),
                target.format(),
                py.font_family.as_deref(),
                font_size,
                line_height,
            )
            .ok();
            self.applied_metrics = Some(metrics.clone());
        } else if self.applied_metrics.as_ref() != Some(&metrics) {
            let renderer = self.renderer.as_mut().unwrap();
            renderer.set_font_metrics(font_size, line_height);
            renderer.set_font_family(py.font_family.as_deref());
            renderer.evict_glyphs();
            self.applied_metrics = Some(metrics);
        }
        let Some(renderer) = self.renderer.as_ref() else {
            return;
        };
        self.cell_width = renderer.cell_width() / scale_factor;
        let cols = ((target_size.width.0 as f32 / renderer.cell_width()) as u16).max(1);
        let rows = ((target_size.height.0 as f32 / line_height) as u16).max(1);
        self.cols = cols;
        self.rows = rows;
        // Topology, active tab and split ratios can change without a window resize.
        // SessionManager skips unchanged sizes and retries failed resizes on the next frame.
        pyonji.update(cx, |this, _| {
            for (id, geometry) in this.tab_layouts(rows, cols) {
                this.session_manager
                    .resize_session(id, geometry.rows.max(1), geometry.cols.max(1));
            }
        });
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
                cursor_visible: !session.cursor_blink || self.blink_visible,
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
        if self.ime_session != Some(active_session) {
            return None;
        }
        let session = pyonji.session_manager.session(active_session)?;
        let (_, geometry) = pyonji
            .tab_layouts(self.rows, self.cols)
            .into_iter()
            .find(|(session_id, _)| *session_id == active_session)?;
        let (row, col) = session.vt.screen().cursor_position();
        let row = row.min(geometry.rows.saturating_sub(1));
        let col = col.min(geometry.cols.saturating_sub(1));

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
            let entity = cx.entity();
            let focus = this
                .pyonji
                .upgrade()
                .map(|py| py.read(cx).focus_handle.clone());
            div()
                .relative()
                .size_full()
                .on_prepaint(cx.processor(Self::on_prepaint))
                .children(this.target.as_ref().map(|target| {
                    target
                        .surface()
                        .object_fit(gpui::ObjectFit::Fill)
                        .size_full()
                }))
                .child(
                    canvas(
                        |_, _, _| (),
                        move |bounds, _, window, cx| {
                            if let Some(focus) = focus {
                                window.handle_input(
                                    &focus,
                                    ElementInputHandler::new(bounds, entity),
                                    cx,
                                );
                            }
                        },
                    )
                    .absolute()
                    .size_full(),
                )
        }))
        .size_full()
    }
}

// Clamp platform UTF-16 ranges to scalar boundaries before slicing UTF-8.
fn utf16_slice(
    text: &str,
    range: std::ops::Range<usize>,
) -> (usize, usize, std::ops::Range<usize>) {
    let mut units = 0;
    let mut start = None;
    let mut end = None;
    for (byte, ch) in text.char_indices() {
        if start.is_none() && units + ch.len_utf16() > range.start {
            start = Some((byte, units));
        }
        if end.is_none() && units >= range.end.max(range.start) {
            end = Some((byte, units));
        }
        units += ch.len_utf16();
    }
    let (start_byte, start_units) = start.unwrap_or((text.len(), units));
    let (end_byte, end_units) = end.unwrap_or((text.len(), units));
    (
        start_byte,
        end_byte.max(start_byte),
        start_units..end_units.max(start_units),
    )
}

impl EntityInputHandler for Terminal {
    fn text_for_range(
        &mut self,
        range: std::ops::Range<usize>,
        adjusted: &mut Option<std::ops::Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let text = self.ime_preedit.as_deref().unwrap_or("");
        let (start, end, range) = utf16_slice(text, range);
        *adjusted = Some(range);
        Some(text[start..end].to_owned())
    }
    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.ime_selection.clone(),
            reversed: false,
        })
    }
    fn marked_text_range(
        &self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.ime_preedit
            .as_ref()
            .map(|text| 0..text.encode_utf16().count())
    }
    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        self.ime_preedit = None;
        self.ime_session = None;
        self.ime_selection = 0..0;
        cx.notify();
    }
    fn replace_text_in_range(
        &mut self,
        _: Option<std::ops::Range<usize>>,
        text: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(py) = self.pyonji.upgrade() {
            py.update(cx, |py, cx| {
                if let Some(id) = py.active_session() {
                    if self.ime_session.is_none() || self.ime_session == Some(id) {
                        if let Some(session) = py.session_manager.session_mut(id) {
                            session.reset_scrollback();
                        }
                        py.session_manager.send_text(id, text);
                        cx.notify();
                    }
                }
            });
        }
        self.unmark_text(window, cx);
    }
    fn paste(&mut self, item: ClipboardItem, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = item.text()
            && let Some(py) = self.pyonji.upgrade()
        {
            py.update(cx, |py, cx| {
                if let Some(id) = py.active_session()
                    && let Some(session) = py.session_manager.session_mut(id)
                {
                    session.paste(&text);
                    cx.notify();
                }
            });
        }
    }
    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<std::ops::Range<usize>>,
        text: &str,
        selected: Option<std::ops::Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let session = self
            .pyonji
            .upgrade()
            .and_then(|py| py.read(cx).active_session());
        if self.ime_session != session {
            self.ime_preedit = None;
        }
        self.ime_session = session;
        let old = self.ime_preedit.as_deref().unwrap_or("");
        let old_len = old.encode_utf16().count();
        let range = range.unwrap_or(0..old_len);
        let (start, end, _) = utf16_slice(old, range);
        let mut next = String::from(&old[..start]);
        next.push_str(text);
        next.push_str(&old[end..]);
        let offset = old[..start].encode_utf16().count();
        let inserted_len = text.encode_utf16().count();
        self.ime_selection = selected
            .map(|range| {
                offset + range.start.min(inserted_len)
                    ..offset
                        + range
                            .end
                            .min(inserted_len)
                            .max(range.start.min(inserted_len))
            })
            .unwrap_or(offset + inserted_len..offset + inserted_len);
        self.ime_preedit = Some(next);
        self.blink_visible = true;
        cx.notify();
    }
    fn bounds_for_range(
        &mut self,
        _: std::ops::Range<usize>,
        _: Bounds<Pixels>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let py = self.pyonji.upgrade()?.read(cx);
        let id = py.active_session()?;
        let (_, pane) = py
            .tab_layouts(self.rows, self.cols)
            .into_iter()
            .find(|(sid, _)| *sid == id)?;
        let (row, col) = py
            .session_manager
            .session(id)?
            .vt
            .screen()
            .cursor_position();
        let line_height = py.font_size * py.line_height;
        Some(Bounds::new(
            point(
                self.bounds.origin.x
                    + px(self.cell_width * f32::from(pane.x + col.min(pane.cols.saturating_sub(1)))),
                self.bounds.origin.y
                    + px(line_height * f32::from(pane.y + row.min(pane.rows.saturating_sub(1)))),
            ),
            size(px(self.cell_width), px(line_height)),
        ))
    }
    fn character_index_for_point(
        &mut self,
        _: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        None
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
                if (x - line_x).abs() <= HIT_SLOP && y >= min_y - HIT_SLOP && y <= max_y + HIT_SLOP
                {
                    return Some(divider.clone());
                }
            }
            SplitDirection::Horizontal => {
                let line_y = line_height * f32::from(divider.y);
                let min_x = cell_width * f32::from(divider.x);
                let max_x = cell_width * f32::from(divider.x + divider.cols);
                if (y - line_y).abs() <= HIT_SLOP && x >= min_x - HIT_SLOP && x <= max_x + HIT_SLOP
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
