mod background;
mod glyph;
use background::BackgroundRenderer;

use std::{sync::Arc, time::Instant};

use anyhow::{Context, Result};
use bumpalo::{collections::Vec as ArenaVec, Bump as Arena};
use unicode_segmentation::UnicodeSegmentation;
use vt100::Screen;
use wgpu::{
    Adapter, Backends, CommandEncoderDescriptor, CompositeAlphaMode, Device, DeviceDescriptor,
    ExperimentalFeatures, Features, Instance, InstanceDescriptor, Limits, LoadOp, MemoryHints,
    MultisampleState, Operations, PowerPreference, PresentMode, Queue, RenderPassColorAttachment,
    RenderPassDescriptor, RequestAdapterOptions, StoreOp, Surface, SurfaceConfiguration,
    SurfaceError, TextureFormat, TextureUsages, Trace,
};
use winit::{dpi::PhysicalSize, window::Window};

use crate::{renderer::glyph::TerminalRenderer, CursorState};

#[derive(Debug, Clone, Copy)]
pub struct Color([u8; 4]);

impl Color {
    pub fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self([r, g, b, 0xFF])
    }

    pub fn to_linear(&self) -> [f32; 4] {
        fn srgb_to_linear(c: u8) -> f32 {
            let c = c as f32 / 255.0;
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
            a as f32 / 255.0,
        ]
    }

    pub fn to_wgpu(&self) -> wgpu::Color {
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
            vt100::Color::Idx(idx) => Self::from(ansi_index_to_rgb(idx)),
            vt100::Color::Rgb(r, g, b) => Self([r, g, b, 0xFF]),
            vt100::Color::Default => Self([0x18, 0x18, 0x18, 0xFF]),
        }
    }
}

pub struct Renderer {
    window: Arc<Window>,
    instance: Instance,
    surface: Surface<'static>,
    adapter: Adapter,
    device: Device,
    queue: Queue,
    format: TextureFormat,
    background_renderer: BackgroundRenderer,
    terminal_renderer: TerminalRenderer,
    arena: Arena,
    font_size: f32,
    line_heigt: f32,
}

impl Renderer {
    pub fn new(window: Arc<Window>, font_size: f32, line_heigt: f32) -> Result<Self> {
        let size = window.inner_size();
        let instance = Instance::new(&InstanceDescriptor {
            backends: Backends::VULKAN,
            ..Default::default()
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
            .find(|f| f.is_srgb())
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
        let terminal_renderer = TerminalRenderer::new(&device, &queue, format);

        Ok(Renderer {
            window,
            instance,
            surface,
            adapter,
            device,
            queue,
            format,
            background_renderer,
            terminal_renderer,
            arena: Arena::new(),
            font_size,
            line_heigt,
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
        self.surface.configure(
            &self.device,
            &SurfaceConfiguration {
                usage: TextureUsages::RENDER_ATTACHMENT,
                format: self.format.clone(),
                width: size.width,
                height: size.height,
                present_mode: PresentMode::Fifo,
                alpha_mode: CompositeAlphaMode::Opaque,
                view_formats: vec![],
                desired_maximum_frame_latency: 2,
            },
        );
    }

    pub fn render(&mut self, screen: &Screen, cursor_style: &CursorState) -> Result<()> {
        let size = self.window.inner_size();
        let surface = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(SurfaceError::Lost | SurfaceError::Outdated) => {
                self.surface.configure(
                    &self.device,
                    &SurfaceConfiguration {
                        usage: TextureUsages::RENDER_ATTACHMENT,
                        format: self.format.clone(),
                        width: size.width,
                        height: size.height,
                        present_mode: PresentMode::Fifo,
                        alpha_mode: CompositeAlphaMode::Opaque,
                        view_formats: vec![],
                        desired_maximum_frame_latency: 2,
                    },
                );
                return Ok(());
            }
            Err(SurfaceError::OutOfMemory) => anyhow::bail!("surface out of memory"),
            Err(SurfaceError::Timeout) => return Ok(()),
            Err(SurfaceError::Other) => return Ok(()),
        };

        /*let (rows, cols) = screen.size();
        {
            let mut rows_vec = ArenaVec::new_in(&self.arena);
            for row in 0..rows {
                let mut cells = ArenaVec::new_in(&self.arena);
                for col in 0..cols {
                    let Some(cell) = screen.cell(row, col) else {
                        continue;
                    };
                    let mut attrs = attrs.clone();
                    let fg_color = match cell.fgcolor() {
                        vt100::Color::Rgb(r, g, b) => glyphon::Color::rgb(r, g, b),
                        vt100::Color::Idx(idx) => ansi_index_to_rgb(idx),
                        _ => glyphon::Color::rgb(0xc6, 0xd0, 0xf5),
                    };
                    let bg_color = match cell.bgcolor() {
                        vt100::Color::Rgb(r, g, b) => glyphon::Color::rgb(r, g, b),
                        vt100::Color::Idx(idx) => ansi_index_to_rgb(idx),
                        _ => glyphon::Color::rgb(0x18, 0x18, 0x18),
                    };
                    attrs = attrs.color(if cell.inverse() { bg_color } else { fg_color });
                    {
                        let x = self.font_size / 2.0 * col as f32;
                        let y = self.line_heigt * (row + 1) as f32;
                        let [x, y] = self.ndc([x, y]);
                        let [w, h] = [
                            self.font_size / size.width as f32,
                            (self.line_heigt * 2.0) / size.height as f32,
                        ];
                        let bg_color = if cell.inverse() { fg_color } else { bg_color };
                        self.background_renderer.add_rect(
                            x,
                            y,
                            w,
                            h,
                            Color::from(bg_color).to_linear(),
                        );
                    }
                    if cell.bold() {
                        attrs = attrs.weight(Weight::BOLD);
                    }
                    if cell.italic() {
                        attrs = attrs.style(Style::Italic);
                    }
                    if !cell.has_contents() {
                        cells.push((" ".to_string(), attrs.clone()));
                    } else {
                        cells.push((cell.contents(), attrs));
                    }
                }
                cells.push(("\n".to_string(), attrs.clone()));
                rows_vec.push(cells);
            }

            if self.line_buffers.len() <= rows as usize {
                let mut buffer = Buffer::new(
                    &mut self.font_system,
                    Metrics::new(self.font_size, self.line_heigt),
                );
                buffer.set_size(
                    &mut self.font_system,
                    Some(size.width as f32),
                    Some(size.height as f32),
                );
                self.line_buffers.resize(rows as usize, buffer);
            }

            //let start = Instant::now();
            for (i, row) in rows_vec.iter().enumerate() {
                self.line_buffers[i].set_rich_text(
                    &mut self.font_system,
                    row.iter().map(|(str, attr)| (str.as_str(), attr.clone())),
                    &attrs,
                    Shaping::Advanced,
                    None,
                );
            }
            //println!("setting text = {:?}", Instant::now() - start);

            /**/

            /*let start = Instant::now();
            self.buffer.set_rich_text(
                &mut self.font_system,
                cells.iter().map(|(str, attr)| (str.as_str(), attr.clone())),
                &attrs,
                Shaping::Advanced,
                None,
            );
            println!("setting text = {:?}", Instant::now() - start);*/
        }*/
        //self.buffer.shape_until_scroll(&mut self.font_system, true);

        let start = Instant::now();
        let (rows, cols) = screen.size();
        for row in 0..rows {
            for col in 0..cols {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                let fg_color = match cell.fgcolor() {
                    vt100::Color::Rgb(r, g, b) => Color::rgb(r, g, b),
                    vt100::Color::Idx(idx) => ansi_index_to_rgb(idx),
                    _ => Color::rgb(0xc6, 0xd0, 0xf5),
                };
                let bg_color = match cell.bgcolor() {
                    vt100::Color::Rgb(r, g, b) => Color::rgb(r, g, b),
                    vt100::Color::Idx(idx) => ansi_index_to_rgb(idx),
                    _ => Color::rgb(0x18, 0x18, 0x18),
                };
                let x = self.font_size / 2.0 * col as f32;
                let y = self.line_heigt * (row + 1) as f32;
                {
                    let [x, y] = self.ndc([x, y]);
                    let [w, h] = [
                        self.font_size / size.width as f32,
                        (self.line_heigt * 2.0) / size.height as f32,
                    ];
                    let bg_color = if cell.inverse() { fg_color } else { bg_color };
                    self.background_renderer
                        .add_rect(x, y, w, h, bg_color.to_linear());
                }
                let fg_color = if cell.inverse() { bg_color } else { fg_color };
                let contents = cell.contents();
                for cluster in contents.graphemes(true) {
                    /*self.terminal_renderer.add_cluster(
                        &self.queue,
                        [x, y],
                        [size.width as f32, size.height as f32],
                        cluster,
                        fg_color,
                    );*/
                    if cluster.len() != 1 {
                        self.terminal_renderer.add_cluster(
                            &self.queue,
                            [x, y],
                            [size.width as f32, size.height as f32],
                            cluster,
                            fg_color,
                        );
                    } else {
                        for ch in cluster.chars() {
                            self.terminal_renderer.add_glyph(
                                &self.queue,
                                [x, y],
                                [size.width as f32, size.height as f32],
                                ch,
                                fg_color,
                            );
                        }
                    }
                }
            }
        }
        println!("layout cells = {:?}", Instant::now() - start);

        //=              ╮  │
        /*for cluster in "│ │".graphemes(true) {
            self.terminal_renderer.add_cluster(
                &self.queue,
                [0.0, 28.0],
                [size.width as f32, size.height as f32],
                cluster,
                Color::rgb(0xFF, 0xFF, 0xFF),
            );
        }*/

        if !screen.hide_cursor() {
            let (row, col) = screen.cursor_position();
            let x = self.font_size / 2.0 * col as f32;
            let y = self.line_heigt * (row + 1) as f32;
            let [x, y] = self.ndc([x, y]);
            let [w, h] = match cursor_style {
                CursorState::Bar => [
                    (self.font_size * 0.18) / size.width as f32,
                    (self.line_heigt * 2.0) / size.height as f32,
                ],
                CursorState::Block => [
                    (self.font_size) / size.width as f32,
                    (self.line_heigt * 2.0) / size.height as f32,
                ],
                CursorState::Underline => [
                    (self.font_size * 0.18) / size.width as f32,
                    (self.line_heigt * 2.0) / size.height as f32,
                ],
            };
            self.background_renderer
                .add_rect(x, y, w, h, [0.78, 0.82, 0.96, 0.45]);
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
                    view: &surface.texture.create_view(&Default::default()),
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
                .render(&self.device, &self.queue, &mut pass)
                .context("failed to render background")?;
            self.terminal_renderer
                .render(&self.device, &self.queue, &mut pass)
                .context("failed to render terminal")?;
        }
        self.queue.submit(Some(encoder.finish()));
        surface.present();
        self.arena.reset();
        Ok(())
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
