mod background;
mod glyph;
use background::BackgroundRenderer;

use std::sync::Arc;

use anyhow::{Context, Result};
use bumpalo::Bump as Arena;
use unicode_segmentation::UnicodeSegmentation;
use vt100::Screen;
use wgpu::{
    Adapter, BackendOptions, Backends, CommandEncoderDescriptor, CompositeAlphaMode,
    CurrentSurfaceTexture, Device, DeviceDescriptor, ExperimentalFeatures, Features, Instance,
    InstanceDescriptor, InstanceFlags, Limits, LoadOp, MemoryBudgetThresholds, MemoryHints,
    Operations, PowerPreference, PresentMode, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, StoreOp, Surface, SurfaceConfiguration,
    TextureFormat, TextureUsages, TextureViewDescriptor, Trace,
};
use winit::{dpi::PhysicalSize, window::Window};

use crate::{
    renderer::glyph::TerminalRenderer,
    terminal::{CursorState, Divider, PaneGeometry, SplitDirection},
    UiLayer,
};

use egui_wgpu::{Renderer as UiRenderer, RendererOptions};

#[derive(Debug, Clone, Copy)]
pub struct Color([u8; 4]);

#[allow(unused)]
impl Color {
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 0xFF])
    }

    pub fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self([r, g, b, a])
    }

    pub fn u32(v: u32) -> Self {
        Color([
            ((v << 24) & 0xFF) as u8,
            ((v << 16) & 0xFF) as u8,
            ((v << 08) & 0xFF) as u8,
            ((v << 00) & 0xFF) as u8,
        ])
    }

    pub fn to_linear(self) -> [f32; 4] {
        fn srgb_to_linear(c: u8) -> f32 {
            let c = f32::from(c) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        }
        let [r, g, b, a] = self.0;
        [
            srgb_to_linear(r),
            srgb_to_linear(g),
            srgb_to_linear(b),
            f32::from(a) / 255.0,
        ]
    }

    pub fn to_wgpu(self) -> wgpu::Color {
        let [r, g, b, a] = self.to_linear();
        wgpu::Color {
            r: r.into(),
            g: g.into(),
            b: b.into(),
            a: a.into(),
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
    window: Arc<Window>,
    _instance: Instance,
    surface: Surface<'static>,
    _adapter: Adapter,
    device: Device,
    queue: Queue,
    format: TextureFormat,
    background_renderer: BackgroundRenderer,
    terminal_renderer: TerminalRenderer,
    divider_renderer: BackgroundRenderer,
    arena: Arena,
    font_size: f32,
    line_height: f32,

    pub ui_renderer: UiRenderer,
}

pub struct Pane<'a> {
    pub screen: &'a Screen,
    pub cursor_style: &'a CursorState,
    pub geometry: PaneGeometry,
    pub is_active: bool,
}

pub struct StatusTab {
    pub label: String,
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
        window: Arc<Window>,
        font_family: Option<&str>,
        font_size: f32,
        line_height: f32,
    ) -> Result<Self> {
        let size = window.inner_size();
        let instance = Instance::new(InstanceDescriptor {
            backends: Backends::VULKAN,
            flags: InstanceFlags::default(),
            memory_budget_thresholds: MemoryBudgetThresholds::default(),
            backend_options: BackendOptions::default(),
            display: Some(Box::new(window.clone())),
        });
        let surface = instance
            .create_surface(window.clone())
            .context("failed to create surface")?;

        let adapter = pollster::block_on(instance.request_adapter(&RequestAdapterOptions {
            power_preference: PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
            force_fallback_adapter: false,
        }))
        .context("failed to request adapter")?;

        let (device, queue) = pollster::block_on(adapter.request_device(&DeviceDescriptor {
            label: Some("device"),
            required_features: Features::empty(),
            required_limits: Limits::downlevel_defaults(),
            memory_hints: MemoryHints::Performance,
            experimental_features: ExperimentalFeatures::default(),
            trace: Trace::Off,
        }))
        .context("failed to request device")?;

        let surface_caps = surface.get_capabilities(&adapter);
        let format = surface_caps
            .formats
            .iter()
            .copied()
            .find(TextureFormat::is_srgb)
            .or_else(|| surface_caps.formats.first().copied())
            .context("no surface formats available")?;

        let config = SurfaceConfiguration {
            usage: TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width,
            height: size.height,
            present_mode: PresentMode::AutoVsync,
            alpha_mode: CompositeAlphaMode::Opaque,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let background_renderer = BackgroundRenderer::new(&device, format);
        let terminal_renderer = TerminalRenderer::new(&device, font_family, font_size, format);
        let divider_renderer = BackgroundRenderer::new(&device, format);
        let ui_renderer = UiRenderer::new(&device, format, RendererOptions::default());

        Ok(Renderer {
            window,
            _instance: instance,
            surface,
            _adapter: adapter,
            device,
            queue,
            format,
            background_renderer,
            terminal_renderer,
            divider_renderer,
            arena: Arena::new(),
            font_size,
            line_height,

            ui_renderer,
        })
    }

    fn ndc(&self, pos: [f32; 2]) -> [f32; 2] {
        let size = self.window.inner_size();
        let [x, y] = pos;
        let nx = (x / size.width as f32) * 2.0 - 1.0;
        let ny = 1.0 - (y / size.height as f32) * 2.0;
        [nx, ny]
    }

    pub fn resize(&mut self, size: PhysicalSize<u32>) {
        if size.width == 0 || size.height == 0 {
            return;
        }
        self.surface.configure(
            &self.device,
            &SurfaceConfiguration {
                usage: TextureUsages::RENDER_ATTACHMENT,
                format: self.format,
                width: size.width,
                height: size.height,
                present_mode: PresentMode::AutoVsync,
                alpha_mode: CompositeAlphaMode::Opaque,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
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
        panes: &[Pane<'_>],
        dividers: &[Divider],
        status_tabs: Option<&[StatusTab]>,
        ime_preedit: Option<&ImePreedit>,
        ui_layer: &mut Option<UiLayer>,
        ui_builder: impl FnMut(&mut egui::Ui),
    ) -> Result<()> {
        let size = self.window.inner_size();
        let screen_size = [size.width as f32, size.height as f32];
        let cell_width = self.font_size / 2.0;
        let grid_cols = (size.width as f32 / cell_width).floor().max(1.0) as usize;
        let grid_rows = (size.height as f32 / self.line_height).floor().max(1.0) as usize;
        let divider_color = [0.56, 0.60, 0.72, 0.95];
        let divider_px = 1.0f32;
        let divider_width = (divider_px / size.width.max(1) as f32) * 2.0;
        let divider_height = (divider_px / size.height.max(1) as f32) * 2.0;
        let surface = match self.surface.get_current_texture() {
            CurrentSurfaceTexture::Success(frame) => frame,
            CurrentSurfaceTexture::Lost
            | CurrentSurfaceTexture::Outdated
            | CurrentSurfaceTexture::Suboptimal(_) => {
                if size.width == 0 || size.height == 0 {
                    return Ok(());
                }
                self.surface.configure(
                    &self.device,
                    &SurfaceConfiguration {
                        usage: TextureUsages::RENDER_ATTACHMENT,
                        format: self.format,
                        width: size.width,
                        height: size.height,
                        present_mode: PresentMode::AutoVsync,
                        alpha_mode: CompositeAlphaMode::Opaque,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                    },
                );
                return Ok(());
            }
            CurrentSurfaceTexture::Occluded | CurrentSurfaceTexture::Timeout => {
                return Ok(());
            }
            CurrentSurfaceTexture::Validation => anyhow::bail!("surface out of memory"),
        };

        let [w, h] = [
            self.font_size / size.width as f32,
            (self.line_height * 2.0) / size.height as f32,
        ];
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
                        let [x, y] = self.ndc([x, y]);
                        let bg_color = if cell.inverse() { fg_color } else { bg_color };
                        self.background_renderer
                            .add_rect(x, y, w, h, bg_color.to_linear());
                    }
                    let fg_color = if cell.inverse() { bg_color } else { fg_color };
                    let contents = cell.contents();
                    let bold = cell.bold();
                    #[allow(clippy::if_not_else)]
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

            if pane.is_active && !pane.screen.hide_cursor() && pane.screen.scrollback() == 0 {
                let (row, col) = pane.screen.cursor_position();
                let x = self.font_size / 2.0 * (f32::from(pane.geometry.x) + f32::from(col));
                let y = self.line_height * (f32::from(pane.geometry.y) + f32::from(row) + 1.0);
                let [x, y] = self.ndc([x, y]);
                let [w, h] = match pane.cursor_style {
                    CursorState::Bar => [
                        (self.font_size * 0.18) / size.width as f32,
                        (self.line_height * 2.0) / size.height as f32,
                    ],
                    CursorState::Block => [
                        (self.font_size) / size.width as f32,
                        (self.line_height * 2.0) / size.height as f32,
                    ],
                    CursorState::Underline => [
                        (self.font_size) / size.width as f32,
                        (self.line_height * 0.1) / size.height as f32,
                    ],
                };
                self.background_renderer
                    .add_rect(x, y, w, h, [0.78, 0.82, 0.96, 0.45]);
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
                    let [x, y] = self.ndc([x, y]);
                    self.divider_renderer
                        .add_rect(x, y, divider_width, height, divider_color);
                }
                SplitDirection::Horizontal => {
                    let x = self.font_size / 2.0 * f32::from(divider.x);
                    let y = self.line_height * f32::from(divider.y) + divider_px;
                    let width =
                        divider_width * f32::from(divider.cols.max(1)) * (self.font_size / 2.0);
                    let [x, y] = self.ndc([x, y]);
                    self.divider_renderer
                        .add_rect(x, y, width, divider_height, divider_color);
                }
            }
        }
        if let Some(tabs) = status_tabs {
            self.draw_status_bar(grid_cols, grid_rows, screen_size, tabs);
        }
        let mut encoder = self
            .device
            .create_command_encoder(&CommandEncoderDescriptor {
                label: Some("render encoder"),
            });
        let view = surface
            .texture
            .create_view(&TextureViewDescriptor::default());
        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("main render pass"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
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
        {
            if let Some(layer) = ui_layer {
                if layer.shown() {
                    layer.render(
                        &mut self.ui_renderer,
                        &self.window,
                        &self.device,
                        &self.queue,
                        &mut encoder,
                        &view,
                        ui_builder,
                    );
                }
            }
        }
        self.queue.submit(Some(encoder.finish()));
        surface.present();
        self.arena.reset();
        Ok(())
    }

    fn draw_status_bar(
        &mut self,
        grid_cols: usize,
        grid_rows: usize,
        screen_size: [f32; 2],
        status_tabs: &[StatusTab],
    ) {
        if grid_cols == 0 || grid_rows == 0 {
            return;
        }

        let bar_row = grid_rows.saturating_sub(1);
        let bar_bottom = self.line_height * (bar_row as f32 + 1.0);
        let bar_height = (self.line_height * 2.0) / screen_size[1].max(1.0);
        let bar_y = self.ndc([0.0, bar_bottom])[1];

        let bg_color = None;
        let active_bg = Color::rgb(0xbb, 0x9a, 0xf7);
        let inactive_bg = Color::rgb(0x56, 0x5f, 0x89);
        let inactive_fg = Color::rgb(0xf0, 0xe7, 0xfa);
        let active_fg = inactive_fg;

        if let Some(bg) = bg_color {
            self.background_renderer
                .add_rect(-1.0, bar_y, 2.0, bar_height, bg);
        }

        let left_limit = grid_cols;

        let mut cursor = 0usize;
        for tab in status_tabs {
            if cursor >= left_limit {
                break;
            }
            let remaining = left_limit.saturating_sub(cursor);
            if remaining < 4 {
                break;
            }

            let label = fit_status_text(&format!(" {} ", tab.label), remaining);
            let width = label.chars().count();
            if width == 0 {
                continue;
            }

            self.draw_status_segment(
                cursor,
                bar_bottom,
                width,
                if tab.is_active {
                    active_bg
                } else {
                    inactive_bg
                },
            );
            self.draw_status_text(
                &label,
                cursor,
                bar_row,
                screen_size,
                if tab.is_active {
                    active_fg
                } else {
                    inactive_fg
                },
            );
            cursor += width;
        }
    }

    fn draw_status_segment(
        &mut self,
        start_col: usize,
        bottom_y: f32,
        width_cols: usize,
        color: Color,
    ) {
        if width_cols == 0 {
            return;
        }

        let x = self.font_size / 2.0 * start_col as f32;
        let width = ((self.font_size / 2.0) * width_cols as f32
            / self.window.inner_size().width as f32)
            * 2.0;
        let [x, y] = self.ndc([x, bottom_y]);
        let height = (self.line_height * 2.0) / self.window.inner_size().height.max(1) as f32;
        self.background_renderer
            .add_rect(x, y, width, height, color.to_linear());
    }

    fn draw_status_text(
        &mut self,
        text: &str,
        start_col: usize,
        row: usize,
        screen_size: [f32; 2],
        color: Color,
    ) {
        let x = self.font_size / 2.0 * start_col as f32;
        let y = self.line_height * (row as f32 + 1.0);
        for (col, cluster) in text.graphemes(true).enumerate() {
            let pos = [x + (self.font_size / 2.0) * col as f32, y];
            #[allow(clippy::if_not_else)]
            if cluster.len() != 1 {
                self.terminal_renderer.add_cluster(
                    &self.queue,
                    pos,
                    screen_size,
                    cluster,
                    color,
                    false,
                );
            } else {
                for ch in cluster.chars() {
                    self.terminal_renderer.add_glyph(
                        &self.queue,
                        pos,
                        screen_size,
                        ch,
                        color,
                        false,
                    );
                }
            }
        }
    }

    fn draw_ime_preedit(&mut self, preedit: &ImePreedit, screen_size: [f32; 2]) {
        let x = self.font_size / 2.0 * (f32::from(preedit.geometry.x) + f32::from(preedit.col));
        let y = self.line_height * (f32::from(preedit.geometry.y) + f32::from(preedit.row) + 1.0);
        let width_cols = preedit.text.graphemes(true).count().max(1);
        let width = ((self.font_size / 2.0) * width_cols as f32
            / self.window.inner_size().width.max(1) as f32)
            * 2.0;
        let height = (self.line_height * 2.0) / self.window.inner_size().height.max(1) as f32;
        let [bg_x, bg_y] = self.ndc([x, y]);

        self.background_renderer
            .add_rect(bg_x, bg_y, width, height, [0.22, 0.24, 0.32, 0.92]);

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

fn fit_status_text(text: &str, max_cols: usize) -> String {
    let len = text.chars().count();
    if len <= max_cols {
        return text.to_string();
    }
    if max_cols <= 3 {
        return ".".repeat(max_cols);
    }

    let mut out = String::new();
    for ch in text.chars().take(max_cols - 3) {
        out.push(ch);
    }
    out.push_str("...");
    out
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
