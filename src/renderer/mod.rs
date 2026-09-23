mod background;
mod glyph;
pub use background::BackgroundRenderer;
pub use glyph::TerminalRenderer;

use anyhow::Result;
use unicode_segmentation::UnicodeSegmentation;
use vt100::Screen;
use wgpu::{
    CommandEncoderDescriptor, Device, LoadOp, Operations, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, StoreOp, TextureFormat, TextureView,
};

use crate::terminal::{CursorState, Divider, PaneGeometry, SplitDirection};

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, PartialOrd, Ord)]
pub struct Color([u8; 4]);

#[allow(unused)]
impl Color {
    pub fn inner(self) -> [u8; 4] {
        self.0
    }

    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 0xFF])
    }

    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([r, g, b, a])
    }

    pub fn u32(v: u32) -> Self {
        Color([
            ((v >> 24) & 0xFF) as u8,
            ((v >> 16) & 0xFF) as u8,
            ((v >> 8) & 0xFF) as u8,
            (v & 0xFF) as u8,
        ])
    }

    /// Convert to the float color used for clear values. Like the shaders,
    /// this passes sRGB-encoded bytes through unchanged (see background.rs):
    /// GPUI composites in pass-through sRGB, so converting to linear here
    /// would darken the output.
    pub fn to_wgpu(self) -> wgpu::Color {
        let [r, g, b, a] = self.0;
        wgpu::Color {
            r: f64::from(r) / 255.0,
            g: f64::from(g) / 255.0,
            b: f64::from(b) / 255.0,
            a: f64::from(a) / 255.0,
        }
    }
}

impl From<vt100::Color> for Color {
    fn from(value: vt100::Color) -> Self {
        match value {
            vt100::Color::Idx(idx) => ansi_index_to_rgb(idx),
            vt100::Color::Rgb(r, g, b) => Self([r, g, b, 0xFF]),
            vt100::Color::Default => Self([0x18, 0x18, 0x18, 0xFF]),
        }
    }
}

pub struct Renderer {
    device: Device,
    queue: Queue,
    background_renderer: BackgroundRenderer,
    terminal_renderer: TerminalRenderer,
    divider_renderer: BackgroundRenderer,
    font_size: f32,
    line_height: f32,
}

#[derive(Debug)]
pub struct Pane<'a> {
    pub screen: &'a Screen,
    pub cursor_style: &'a CursorState,
    pub geometry: PaneGeometry,
    pub is_active: bool,
}

pub struct ImePreedit {
    pub text: String,
    pub geometry: PaneGeometry,
    pub row: u16,
    pub col: u16,
}

impl Renderer {
    pub fn new(
        queue: Queue,
        device: Device,
        format: TextureFormat,
        font_family: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> Result<Self> {
        let background_renderer = BackgroundRenderer::new(&device, format);
        let terminal_renderer = TerminalRenderer::new(&device, font_family, font_size, format);
        let divider_renderer = BackgroundRenderer::new(&device, format);

        Ok(Renderer {
            device,
            queue,
            background_renderer,
            terminal_renderer,
            divider_renderer,
            font_size,
            line_height,
        })
    }

    fn ndc(&self, pos: [f32; 2], size: [u32; 2]) -> [f32; 2] {
        let [x, y] = pos;
        let nx = (x / size[0] as f32) * 2.0 - 1.0;
        let ny = 1.0 - (y / size[1] as f32) * 2.0;
        [nx, ny]
    }

    pub fn evict_glyphs(&mut self) {
        self.terminal_renderer.evict_glyphs(&self.queue);
    }

    pub fn set_font_metrics(&mut self, font_size: f32, line_height: f32) {
        self.font_size = font_size;
        self.line_height = line_height;
        self.terminal_renderer.set_font_size(font_size);
    }

    pub fn set_font_family(&mut self, font_family: &str) {
        self.terminal_renderer.set_font_family(font_family);
    }

    pub fn render(
        &mut self,
        view: &TextureView,
        panes: &[Pane<'_>],
        dividers: &[Divider],
        ime_preedit: Option<&ImePreedit>,
        size: [u32; 2],
    ) -> Result<()> {
        let screen_size = [size[0] as f32, size[1] as f32];
        let divider_color = [197, 203, 221, 242]; //[0.56, 0.60, 0.72, 0.95];
        let divider_px = 1.0f32;
        let divider_width = (divider_px / size[0].max(1) as f32) * 2.0;
        let divider_height = (divider_px / size[1].max(1) as f32) * 2.0;

        let [w, h] = [
            self.font_size / size[0] as f32,
            (self.line_height * 2.0) / size[1] as f32,
        ];
        //  TODO(K): paraellize this -- maybe this will allow proper line shaping without having a
        //  performance impact
        for pane in panes {
            if pane.geometry.cols == 0 || pane.geometry.rows == 0 {
                continue;
            }
            let (rows, cols) = pane.screen.size();
            for row in 0..rows {
                for col in 0..cols {
                    let Some(cell) = pane.screen.cell(row, col) else {
                        continue;
                    };
                    let fg_color = match cell.fgcolor() {
                        vt100::Color::Default => Color::rgb(0xc6, 0xd0, 0xf5),
                        x => Color::from(x),
                    };
                    let bg_color = Color::from(cell.bgcolor());
                    let x = self.font_size / 2.0 * (f32::from(pane.geometry.x) + f32::from(col));
                    let y = self.line_height * (f32::from(pane.geometry.y) + f32::from(row) + 1.0);
                    {
                        let [x, y] = self.ndc([x, y], size);
                        let bg_color = if cell.inverse() { fg_color } else { bg_color };
                        self.background_renderer
                            .add_rect(x, y, w, h, bg_color.inner());
                    }
                    let fg_color = if cell.inverse() { bg_color } else { fg_color };
                    let contents = cell.contents();
                    let bold = cell.bold();
                    #[allow(clippy::if_not_else)]
                    if contents.is_ascii() {
                        for ch in contents.chars() {
                            self.terminal_renderer.add_glyph(
                                &self.queue,
                                [x, y],
                                screen_size,
                                ch,
                                fg_color,
                                bold,
                            );
                        }
                    } else {
                        for cluster in contents.graphemes(true) {
                            if cluster.len() != 1 {
                                self.terminal_renderer.add_cluster(
                                    &self.queue,
                                    [x, y],
                                    screen_size,
                                    cluster,
                                    fg_color,
                                    bold,
                                );
                            } else {
                                for ch in cluster.chars() {
                                    self.terminal_renderer.add_glyph(
                                        &self.queue,
                                        [x, y],
                                        screen_size,
                                        ch,
                                        fg_color,
                                        bold,
                                    );
                                }
                            }
                        }
                    }
                }
            }

            if pane.is_active && !pane.screen.hide_cursor() && pane.screen.scrollback() == 0 {
                let (row, col) = pane.screen.cursor_position();
                let x = self.font_size / 2.0 * (f32::from(pane.geometry.x) + f32::from(col));
                let y = self.line_height * (f32::from(pane.geometry.y) + f32::from(row) + 1.0);
                let [x, y] = self.ndc([x, y], size);
                let [w, h] = match pane.cursor_style {
                    CursorState::Bar => [
                        (self.font_size * 0.18) / size[0] as f32,
                        (self.line_height * 2.0) / size[1] as f32,
                    ],
                    CursorState::Block => [
                        (self.font_size) / size[0] as f32,
                        (self.line_height * 2.0) / size[1] as f32,
                    ],
                    CursorState::Underline => [
                        (self.font_size) / size[0] as f32,
                        (self.line_height * 0.1) / size[1] as f32,
                    ],
                };
                self.background_renderer
                    .add_rect(x, y, w, h, [229, 234, 250, 115]); //[0.78, 0.82, 0.96, 0.45]
            }
        }

        if let Some(preedit) = ime_preedit {
            self.draw_ime_preedit(preedit, screen_size);
        }

        for divider in dividers {
            match divider.direction {
                SplitDirection::Vertical => {
                    let x = self.font_size / 2.0 * f32::from(divider.x);
                    let y = self.line_height * f32::from(divider.y + divider.rows);
                    let height = divider_height * f32::from(divider.rows.max(1)) * self.line_height;
                    let [x, y] = self.ndc([x, y], size);
                    self.divider_renderer
                        .add_rect(x, y, divider_width, height, divider_color);
                }
                SplitDirection::Horizontal => {
                    let x = self.font_size / 2.0 * f32::from(divider.x);
                    let y = self.line_height * f32::from(divider.y) + divider_px;
                    let width =
                        divider_width * f32::from(divider.cols.max(1)) * (self.font_size / 2.0);
                    let [x, y] = self.ndc([x, y], size);
                    self.divider_renderer
                        .add_rect(x, y, width, divider_height, divider_color);
                }
            }
        }
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("render encoder"),
            });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("main render pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    ops: Operations {
                        load: LoadOp::Clear(Color([0x18, 0x18, 0x18, 0xFF]).to_wgpu()),
                        store: StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            self.background_renderer
                .render(&self.device, &self.queue, &mut pass);
            self.terminal_renderer
                .render(&self.device, &self.queue, &mut pass);
            self.divider_renderer
                .render(&self.device, &self.queue, &mut pass);
        }
        self.queue.submit(Some(encoder.finish()));
        Ok(())
    }

    fn draw_ime_preedit(&mut self, preedit: &ImePreedit, screen_size: [f32; 2]) {
        let size = [screen_size[0] as u32, screen_size[1] as u32];
        let x = self.font_size / 2.0 * (f32::from(preedit.geometry.x) + f32::from(preedit.col));
        let y = self.line_height * (f32::from(preedit.geometry.y) + f32::from(preedit.row) + 1.0);
        let width_cols = preedit.text.graphemes(true).count().max(1);
        let width = ((self.font_size / 2.0) * width_cols as f32 / size[0].max(1) as f32) * 2.0;
        let height = (self.line_height * 2.0) / size[1].max(1) as f32;
        let [bg_x, bg_y] = self.ndc([x, y], size);

        self.background_renderer
            .add_rect(bg_x, bg_y, width, height, [129, 134, 153, 235]); //[0.22, 0.24, 0.32, 0.92]

        for (col, cluster) in preedit.text.graphemes(true).enumerate() {
            let pos = [x + (self.font_size / 2.0) * col as f32, y];
            self.terminal_renderer.add_cluster(
                &self.queue,
                pos,
                screen_size,
                cluster,
                Color::rgb(0xf0, 0xe7, 0xfa),
                false,
            );
        }
    }
}

fn ansi_index_to_rgb(idx: u8) -> Color {
    const BASE16: [(u8, u8, u8); 16] = [
        (0x36, 0x38, 0x4a),
        (0xd4, 0x6c, 0x8a),
        (0x82, 0xb8, 0x7e),
        (0xd9, 0xb8, 0x8a),
        (0x6c, 0x8c, 0xd8),
        (0xc8, 0x96, 0xc8),
        (0x72, 0xb4, 0xa8),
        (0x94, 0x9c, 0xb4),
        (0x48, 0x4b, 0x5e),
        (0xd4, 0x6c, 0x8a),
        (0x82, 0xb8, 0x7e),
        (0xd9, 0xb8, 0x8a),
        (0x6c, 0x8c, 0xd8),
        (0xc8, 0x96, 0xc8),
        (0x72, 0xb4, 0xa8),
        (0x88, 0x8f, 0xa8),
    ];

    if idx < 16 {
        let rgb = BASE16[idx as usize];
        return Color::rgb(rgb.0, rgb.1, rgb.2);
    }

    if (16..=231).contains(&idx) {
        let n = idx - 16;
        let r = n / 36;
        let g = (n % 36) / 6;
        let b = n % 6;
        let step = [0, 95, 135, 175, 215, 255];
        return Color::rgb(step[r as usize], step[g as usize], step[b as usize]);
    }

    let gray = 8u8.saturating_add((idx - 232).saturating_mul(10));
    Color::rgb(gray, gray, gray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn u32_packs_rrggbbaa() {
        assert_eq!(Color::u32(0x11223344).inner(), [0x11, 0x22, 0x33, 0x44]);
        assert_eq!(Color::u32(0xFF000000).inner(), [0xFF, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn to_wgpu_passes_srgb_through() {
        // GPUI composites in pass-through sRGB: no gamma conversion.
        let c = Color::rgb(0x18, 0x18, 0x18).to_wgpu();
        let expected = 0x18 as f64 / 255.0;
        assert!((c.r - expected).abs() < 1e-9);
        assert!((c.g - expected).abs() < 1e-9);
        assert!((c.b - expected).abs() < 1e-9);
        assert!((c.a - 1.0).abs() < 1e-9);
    }

    #[test]
    fn ansi_palette_spot_checks() {
        // 6x6x6 cube corners.
        assert_eq!(ansi_index_to_rgb(16).inner(), [0, 0, 0, 0xFF]);
        assert_eq!(ansi_index_to_rgb(231).inner(), [255, 255, 255, 0xFF]);
        assert_eq!(ansi_index_to_rgb(196).inner(), [255, 0, 0, 0xFF]);
        // Grayscale ramp ends.
        assert_eq!(ansi_index_to_rgb(232).inner(), [8, 8, 8, 0xFF]);
        assert_eq!(ansi_index_to_rgb(255).inner(), [238, 238, 238, 0xFF]);
    }
}
