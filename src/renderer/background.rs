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
    @location(1) origin: vec2<f32>,
    @location(2) size: vec2<f32>,
    @location(3) color: vec4<u32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) color: vec4<u32>,
};

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.origin + in.pos * in.size, 0.0, 1.0);
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

const QUAD_VERTS: &[f32; 8] = &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
const QUAD_INDICES: &[u16; 6] = &[0, 1, 2, 1, 2, 3];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    origin: [f32; 2],
    size: [f32; 2],
    color: [u8; 4],
}

pub struct BackgroundRenderer {
    _shader: ShaderModule,
    pipeline: RenderPipeline,
    vertex_buffer: Buffer,
    index_buffer: Buffer,
    instance_buffer: Buffer,
    instances: Vec<Instance>,
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

        let vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("background quad vertices"),
            size: mem::size_of_val(QUAD_VERTS) as u64,
            usage: BufferUsages::VERTEX,
            mapped_at_creation: true,
        });
        vertex_buffer.slice(..).get_mapped_range_mut().copy_from_slice(bytemuck::cast_slice(QUAD_VERTS));
        vertex_buffer.unmap();

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("background quad indices"),
            size: mem::size_of_val(QUAD_INDICES) as u64,
            usage: BufferUsages::INDEX,
            mapped_at_creation: true,
        });
        index_buffer.slice(..).get_mapped_range_mut().copy_from_slice(bytemuck::cast_slice(QUAD_INDICES));
        index_buffer.unmap();

        let instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("background instances"),
            size: Self::DEFAULT_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("selection pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[
                    VertexBufferLayout {
                        array_stride: mem::size_of::<[f32; 2]>() as BufferAddress,
                        step_mode: VertexStepMode::Vertex,
                        attributes: &[VertexAttribute {
                            format: VertexFormat::Float32x2,
                            offset: 0,
                            shader_location: 0,
                        }],
                    },
                    VertexBufferLayout {
                        array_stride: mem::size_of::<Instance>() as BufferAddress,
                        step_mode: VertexStepMode::Instance,
                        attributes: &[
                            VertexAttribute {
                                format: VertexFormat::Float32x2,
                                offset: 0,
                                shader_location: 1,
                            },
                            VertexAttribute {
                                format: VertexFormat::Float32x2,
                                offset: mem::size_of::<[f32; 2]>() as BufferAddress,
                                shader_location: 2,
                            },
                            VertexAttribute {
                                format: VertexFormat::Uint8x4,
                                offset: mem::size_of::<[f32; 4]>() as BufferAddress,
                                shader_location: 3,
                            },
                        ],
                    },
                ],
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

        Self {
            _shader: shader,
            pipeline,
            vertex_buffer,
            index_buffer,
            instance_buffer,
            instances: Vec::new(),
        }
    }

    fn maybe_grow_buffer(&mut self, device: &Device) {
        let needed = (self.instances.len() * mem::size_of::<Instance>()) as u64;
        if needed >= self.instance_buffer.size() {
            self.instance_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("background instances"),
                size: needed.max(Self::DEFAULT_BUFFER_SIZE),
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
    }

    pub fn render(&mut self, device: &Device, queue: &Queue, pass: &mut RenderPass) {
        if self.instances.is_empty() {
            return;
        }

        self.maybe_grow_buffer(device);
        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&self.instances),
        );

        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_vertex_buffer(1, self.instance_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), IndexFormat::Uint16);
        pass.draw_indexed(0..6, 0, 0..self.instances.len() as u32);
        self.instances.clear();
    }

    pub fn add_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: [u8; 4]) {
        self.instances.push(Instance {
            origin: [x, y],
            size: [w, h],
            color,
        });
    }
}
