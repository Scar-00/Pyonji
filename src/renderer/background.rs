use std::{borrow::Cow, mem};

use bytemuck::{Pod, Zeroable};
use wgpu::{
    BlendState, Buffer, BufferAddress, BufferUsages, ColorTargetState, ColorWrites, Device,
    FragmentState, IndexFormat, MultisampleState, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, ShaderModule, ShaderModuleDescriptor, ShaderSource, TextureFormat,
    VertexAttribute, VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

const SHADER_SRC: &str = r"
struct VertexInput {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<u32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.pos, 0.0, 1.0);
    out.color = in.color;
    return out;
}

fn srgb_channel_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        return c / 12.92;
    }
    return pow((c + 0.055) / 1.055, 2.4);
}

fn to_linear(srgba: vec4<u32>) -> vec4<f32> {
    let c = vec4<f32>(srgba) / 255.0;
    return vec4<f32>(
        srgb_channel_to_linear(c.r),
        srgb_channel_to_linear(c.g),
        srgb_channel_to_linear(c.b),
        c.a,
    );
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return to_linear(in.color);
}
";

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    color: [u8; 4],
}

pub struct BackgroundRenderer {
    _shader: ShaderModule,
    pipeline: RenderPipeline,
    vertex_buffer: Buffer,
    index_buffer: Buffer,
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
}

impl BackgroundRenderer {
    const DEFAULT_BUFFER_SIZE: u64 = 1024 * 4;

    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("selection shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(SHADER_SRC)),
        });
        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("selection pipeline layout"),
            bind_group_layouts: &[],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("selection pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as BufferAddress,
                    step_mode: VertexStepMode::Vertex,
                    attributes: &[
                        VertexAttribute {
                            format: VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        VertexAttribute {
                            format: VertexFormat::Uint8x4,
                            offset: std::mem::size_of::<[f32; 2]>() as BufferAddress,
                            shader_location: 1,
                        },
                    ],
                }],
            },
            primitive: PrimitiveState::default(),
            depth_stencil: None,
            multisample: MultisampleState::default(),
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                targets: &[Some(ColorTargetState {
                    format,
                    blend: Some(BlendState::ALPHA_BLENDING),
                    write_mask: ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("background vertices"),
            size: Self::DEFAULT_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("background indices"),
            size: Self::DEFAULT_BUFFER_SIZE,
            usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            _shader: shader,
            pipeline,
            vertex_buffer,
            index_buffer,
            vertices: Vec::new(),
            indices: Vec::new(),
        }
    }

    fn maybe_grow_buffer(&mut self, device: &Device) {
        if self.vertices.len() * mem::size_of::<Vertex>() >= self.vertex_buffer.size() as usize {
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("background vertices"),
                size: (self.vertices.len() * mem::size_of::<Vertex>()) as u64,
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let index_size = self.indices.len() * mem::size_of::<u16>();
        if index_size >= self.index_buffer.size() as usize {
            self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("background indices"),
                size: index_size as u64,
                usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
    }

    pub fn render(&mut self, device: &Device, queue: &Queue, pass: &mut RenderPass) {
        self.maybe_grow_buffer(device);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&self.indices));

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), IndexFormat::Uint16);
        pass.draw_indexed(0..self.indices.len() as u32, 0, 0..1);
        self.vertices.clear();
        self.indices.clear();
    }

    pub fn add_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        let idx = self.vertices.len() as u16;
        self.vertices.push(Vertex { pos: [x, y], color });
        self.vertices.push(Vertex {
            pos: [x + w, y],
            color,
        });
        self.vertices.push(Vertex {
            pos: [x, y + h],
            color,
        });
        self.vertices.push(Vertex {
            pos: [x + w, y + h],
            color,
        });
        self.indices.extend_from_slice(&[
            idx,
            idx + 1,
            idx + 2,
            idx + 1,
            idx + 2,
            idx + 3,
        ]);
    }
}
