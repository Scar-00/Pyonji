use std::{borrow::Cow, fs, mem, path::PathBuf};

use anyhow::{Context, Result};
use bytemuck::{Pod, Zeroable};
use etagere::{AtlasAllocator, BucketedAtlasAllocator};
use smallvec::SmallVec;
use swash::{
    scale::{
        image::{Content, Image},
        Render, ScaleContext, Source, StrikeWith,
    },
    shape::{Direction, ShapeContext},
    zeno::{Format, Placement, Vector},
    CacheKey, Charmap, FontRef,
};
use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferAddress,
    BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState,
    MipmapFilterMode, MultisampleState, Origin3d, PipelineCompilationOptions,
    PipelineLayoutDescriptor, PrimitiveState, Queue, RenderPass, RenderPipeline,
    RenderPipelineDescriptor, SamplerBindingType, SamplerDescriptor, ShaderModule,
    ShaderModuleDescriptor, ShaderSource, ShaderStages, TexelCopyBufferLayout,
    TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor, TextureDimension,
    TextureFormat, TextureSampleType, TextureUsages, TextureViewDescriptor, TextureViewDimension,
    VertexAttribute, VertexBufferLayout, VertexState, VertexStepMode,
};

use crate::renderer::Color;

pub trait VertexData: Sized {
    const VERTEX_ATTRIBUTES: &[VertexAttribute];

    fn descriptor() -> VertexBufferLayout<'static> {
        VertexBufferLayout {
            array_stride: std::mem::size_of::<Self>() as BufferAddress,
            step_mode: VertexStepMode::Vertex,
            attributes: Self::VERTEX_ATTRIBUTES,
        }
    }
}

const SHADER_SRC: &str = r"
struct VertexInput {
    @location(0) pos: vec2<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) color: vec4<u32>,
    @location(3) variant: u32,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<u32>,
    @location(2) variant: u32,
};

@group(0) @binding(0)
var glyph_tex: texture_2d<f32>;
@group(0) @binding(1)
var glyph_samp: sampler;
@group(0) @binding(2)
var image_tex: texture_2d<f32>;
@group(0) @binding(3)
var image_samp: sampler;

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

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.variant = in.variant;
    return out;
}

fn sharpen_alpha(a: f32, amount: f32) -> f32 {
    return clamp((a - 0.5) * amount + 0.5, 0.0, 1.0);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    switch in.variant {
        case 0u: {
            let tex = textureSample(glyph_tex, glyph_samp, vec2<f32>(in.uv));
            var alpha = tex.r;
            alpha = sharpen_alpha(alpha, 1.2);
            let color = to_linear(in.color);
            return vec4<f32>(color.xyz, alpha);
        }
        case 1u: {
            return textureSample(image_tex, image_samp, vec2<f32>(in.uv));
        }
        default: {
            return vec4<f32>(1.0);
        }
    }
}
";

const GLYPH_VARIANT_GLYPH: u32 = 0;
const GLYPH_VARIANT_IMAGE: u32 = 1;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    pos: [f32; 2],
    uv: [f32; 2],
    color: [u8; 4],
    variant: u32,
}

impl VertexData for Vertex {
    const VERTEX_ATTRIBUTES: &[VertexAttribute] =
        &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x2, 2 => Uint8x4, 3 => Uint32];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum FontVariant {
    Normal,
    Bold,
    Ime,
    Emoji,
}

pub struct Font {
    data: Vec<u8>,
    offset: u32,
    key: CacheKey,
}

impl Font {
    fn from_file(path: PathBuf, index: usize) -> Option<Self> {
        let content = fs::read(path).ok()?;
        Self::from_data(&content, index)
    }

    fn from_data(data: &[u8], index: usize) -> Option<Self> {
        let font = FontRef::from_index(data, index)?;
        let (offset, key) = (font.offset, font.key);
        Some(Self {
            data: data.to_vec(),
            offset,
            key,
        })
    }

    pub fn charmap(&self) -> Charmap<'_> {
        self.as_ref().charmap()
    }

    pub fn as_ref(&self) -> FontRef<'_> {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Glyph {
    uv_min: [f32; 2],
    uv_max: [f32; 2],
    placement: Placement,
    content: Content,
}

pub struct TerminalRenderer {
    font_size: f32,
    font_db: fontdb::Database,

    _shader: ShaderModule,
    pipeline: RenderPipeline,
    vertex_buffer: Buffer,
    vertices: Vec<Vertex>,
    glyph_atlas_texture: Texture,
    image_atlas_texture: Texture,

    uniform_bind_group: BindGroup,

    normal_font: Font,
    bold_font: Font,
    ime_font: Font,
    icon_font: Font,
    scale_context: ScaleContext,
    shape_context: ShapeContext,
    normal_glyph_map: Vec<Option<Glyph>>,
    bold_glyph_map: Vec<Option<Glyph>>,
    ime_glyph_map: Vec<Option<Glyph>>,
    icon_glyph_map: Vec<Option<Glyph>>,
    glyph_atlas: BucketedAtlasAllocator,
    image_atlas: AtlasAllocator,
    atlas_size: [f32; 2],
}

impl TerminalRenderer {
    const NORMAL_FONT: &[u8] = include_bytes!("../../../resources/fonts/SGr-IosevkaTerm-Light.ttc");
    const BOLD_FONT: &[u8] =
        include_bytes!("../../../resources/fonts/SGr-IosevkaTerm-SemiBold.ttc");
    const IME_FONT: &[u8] =
        include_bytes!("../../../resources/fonts/NotoSansMonoCJKkr-Regular.otf");
    const ICON_FONT: &[u8] =
        include_bytes!("../../../resources/fonts/JetBrainsMonoNerdFontMono-Regular.ttf");
    const DEFAULT_BUFFER_SIZE: u64 = (1024 * 16) * 32;

    fn load_glyph(&mut self, variant: FontVariant, id: u16) -> Option<Image> {
        let font = match variant {
            FontVariant::Normal => &self.normal_font,
            FontVariant::Bold => &self.bold_font,
            FontVariant::Ime => &self.ime_font,
            FontVariant::Emoji => &self.icon_font,
        };
        let mut scaler = self
            .scale_context
            .builder(font.as_ref())
            .size(self.font_size)
            .hint(true)
            .build();

        let offset = Vector::new(0.0, 0.0);
        Render::new(&[
            Source::ColorOutline(0),
            Source::ColorBitmap(StrikeWith::BestFit),
            Source::Outline,
        ])
        .format(Format::Alpha)
        .offset(offset)
        .transform(None)
        .render(&mut scaler, id)
    }

    fn get_or_create_glyph(
        &mut self,
        queue: &Queue,
        variant: FontVariant,
        glyph: impl Into<u32>,
    ) -> Option<Glyph> {
        let glyph_id = self.font(variant).charmap().map(glyph);
        self.get_or_create_glyph_id(queue, variant, glyph_id)
    }

    fn get_or_create_glyph_id(
        &mut self,
        queue: &Queue,
        variant: FontVariant,
        glyph_id: u16,
    ) -> Option<Glyph> {
        if let Some(Some(glyph)) = self.glyph_map(variant).get(glyph_id as usize) {
            return Some(*glyph);
        }

        let image = self.load_glyph(variant, glyph_id)?;
        let size = etagere::size2(
            image.placement.width.cast_signed(),
            image.placement.height.cast_signed(),
        );
        let alloc = if image.content == Content::Mask {
            let alloc = self.glyph_atlas.allocate(size)?;

            queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &self.glyph_atlas_texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: alloc.rectangle.min.x.cast_unsigned(),
                        y: alloc.rectangle.min.y.cast_unsigned(),
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image.data,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(image.placement.width),
                    rows_per_image: Some(image.placement.height),
                },
                Extent3d {
                    width: image.placement.width,
                    height: image.placement.height,
                    depth_or_array_layers: 1,
                },
            );
            alloc
        } else {
            let alloc = self.image_atlas.allocate(size)?;

            queue.write_texture(
                TexelCopyTextureInfo {
                    texture: &self.image_atlas_texture,
                    mip_level: 0,
                    origin: Origin3d {
                        x: alloc.rectangle.min.x.cast_unsigned(),
                        y: alloc.rectangle.min.y.cast_unsigned(),
                        z: 0,
                    },
                    aspect: TextureAspect::All,
                },
                &image.data,
                TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * image.placement.width),
                    rows_per_image: Some(image.placement.height),
                },
                Extent3d {
                    width: image.placement.width,
                    height: image.placement.height,
                    depth_or_array_layers: 1,
                },
            );
            alloc
        };

        let rect = alloc.rectangle;

        let uv_min_x = (rect.min.x as f32) / self.atlas_size[0];
        let uv_min_y = (rect.min.y as f32) / self.atlas_size[1];
        let uv_max_x = (rect.min.x as f32 + image.placement.width as f32) / self.atlas_size[0];
        let uv_max_y = (rect.min.y as f32 + image.placement.height as f32) / self.atlas_size[1];
        let glyph = Glyph {
            uv_min: [uv_min_x, uv_min_y],
            uv_max: [uv_max_x, uv_max_y],
            placement: image.placement,
            content: image.content,
        };
        let glyph_map = self.glyph_map_mut(variant);
        if glyph_id as usize >= glyph_map.len() {
            glyph_map.resize(glyph_id as usize + 1, None);
        }
        glyph_map[glyph_id as usize] = Some(glyph);
        Some(glyph)
    }

    fn font(&self, variant: FontVariant) -> &Font {
        match variant {
            FontVariant::Normal => &self.normal_font,
            FontVariant::Bold => &self.bold_font,
            FontVariant::Ime => &self.ime_font,
            FontVariant::Emoji => &self.icon_font,
        }
    }

    fn glyph_map(&self, variant: FontVariant) -> &Vec<Option<Glyph>> {
        match variant {
            FontVariant::Normal => &self.normal_glyph_map,
            FontVariant::Bold => &self.bold_glyph_map,
            FontVariant::Ime => &self.ime_glyph_map,
            FontVariant::Emoji => &self.icon_glyph_map,
        }
    }

    fn glyph_map_mut(&mut self, variant: FontVariant) -> &mut Vec<Option<Glyph>> {
        match variant {
            FontVariant::Normal => &mut self.normal_glyph_map,
            FontVariant::Bold => &mut self.bold_glyph_map,
            FontVariant::Ime => &mut self.ime_glyph_map,
            FontVariant::Emoji => &mut self.icon_glyph_map,
        }
    }

    fn text_font_variant_for_cluster(&self, cluster: &str, bold: bool) -> FontVariant {
        if Self::font_supports_cluster(&self.normal_font, cluster) {
            if bold {
                FontVariant::Bold
            } else {
                FontVariant::Normal
            }
        } else if Self::font_supports_cluster(&self.ime_font, cluster) {
            FontVariant::Ime
        } else {
            FontVariant::Emoji
        }
    }

    fn font_supports_cluster(font: &Font, cluster: &str) -> bool {
        cluster.chars().all(|ch| {
            matches!(
                ch as u32,
                0x200C | 0x200D | 0xFE00..=0xFE0F | 0xE0100..=0xE01EF
            ) || font.charmap().map(ch) != 0
        })
    }

    pub fn new(
        device: &Device,
        font_family: Option<&str>,
        font_size: f32,
        format: TextureFormat,
    ) -> Self {
        use fontdb::{Database, Family, Query, Weight};
        let tex_limits = device.limits().max_texture_dimension_2d;

        let glyph_atlas_allocator = BucketedAtlasAllocator::new(etagere::size2(
            tex_limits.cast_signed(),
            tex_limits.cast_signed(),
        ));

        let image_atlas_allocator = AtlasAllocator::new(etagere::size2(
            tex_limits.cast_signed(),
            tex_limits.cast_signed(),
        ));

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("selection shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(SHADER_SRC)),
        });

        let glyph_texture = device.create_texture(&TextureDescriptor {
            label: Some("font-atlas-texture"),
            size: Extent3d {
                width: tex_limits,
                height: tex_limits,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::R8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let glyph_view = glyph_texture.create_view(&TextureViewDescriptor::default());

        let glyph_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("font-atlas-texture-sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Nearest,
            min_filter: FilterMode::Nearest,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let image_texture = device.create_texture(&TextureDescriptor {
            label: Some("font-atlas-texture"),
            size: Extent3d {
                width: tex_limits,
                height: tex_limits,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format: TextureFormat::Rgba8UnormSrgb,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let image_view = glyph_texture.create_view(&TextureViewDescriptor::default());

        let image_sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("font-atlas-texture-sampler"),
            address_mode_u: AddressMode::ClampToEdge,
            address_mode_v: AddressMode::ClampToEdge,
            address_mode_w: AddressMode::ClampToEdge,
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            mipmap_filter: MipmapFilterMode::Nearest,
            ..Default::default()
        });

        let uniform_bind_group_layout =
            device.create_bind_group_layout(&BindGroupLayoutDescriptor {
                label: Some("uniform-bind-group-layout"),
                entries: &[
                    BindGroupLayoutEntry {
                        binding: 0,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Float { filterable: true },
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 1,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 2,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Texture {
                            sample_type: TextureSampleType::Float { filterable: true },
                            view_dimension: TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    BindGroupLayoutEntry {
                        binding: 3,
                        visibility: ShaderStages::FRAGMENT,
                        ty: BindingType::Sampler(SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });

        let uniform_bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("uniform-bind-group"),
            layout: &uniform_bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&glyph_view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&glyph_sampler),
                },
                BindGroupEntry {
                    binding: 2,
                    resource: BindingResource::TextureView(&image_view),
                },
                BindGroupEntry {
                    binding: 3,
                    resource: BindingResource::Sampler(&image_sampler),
                },
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("selection pipeline layout"),
            bind_group_layouts: &[Some(&uniform_bind_group_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("selection pipeline"),
            layout: Some(&pipeline_layout),
            vertex: VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: PipelineCompilationOptions::default(),
                buffers: &[Vertex::descriptor()],
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

        let mut font_db = Database::new();
        font_db.load_system_fonts();

        let normal_font = Font::from_data(Self::NORMAL_FONT, 0).unwrap();
        let bold_font = Font::from_data(Self::BOLD_FONT, 0).unwrap();
        let ime_font = Font::from_data(Self::IME_FONT, 0).unwrap();
        let icon_font = Font::from_data(Self::ICON_FONT, 0).unwrap();

        let (normal_font, bold_font) = if let Some(font_family) = font_family {
            let normal_font = Self::load_font(
                &mut font_db,
                &Query {
                    families: &[Family::Name(font_family)],
                    weight: Weight::LIGHT,
                    ..Default::default()
                },
            )
            .unwrap_or(normal_font);

            let bold_font = Self::load_font(
                &mut font_db,
                &Query {
                    families: &[Family::Name(font_family)],
                    weight: Weight::SEMIBOLD,
                    ..Default::default()
                },
            )
            .unwrap_or(bold_font);
            (normal_font, bold_font)
        } else {
            (normal_font, bold_font)
        };

        Self {
            font_size,
            font_db,

            _shader: shader,
            pipeline,
            vertex_buffer,
            vertices: Vec::new(),
            uniform_bind_group,
            glyph_atlas_texture: glyph_texture,
            image_atlas_texture: image_texture,

            normal_glyph_map: Vec::new(),
            bold_glyph_map: Vec::new(),
            ime_glyph_map: Vec::new(),
            icon_glyph_map: Vec::new(),
            scale_context: ScaleContext::new(),
            shape_context: ShapeContext::new(),
            normal_font,
            bold_font,
            ime_font,
            icon_font,
            glyph_atlas: glyph_atlas_allocator,
            image_atlas: image_atlas_allocator,
            atlas_size: [tex_limits as f32, tex_limits as f32],
        }
    }

    pub fn evict_glyphs(&mut self, queue: &Queue) {
        self.glyph_atlas.clear();
        self.image_atlas.clear();
        self.normal_glyph_map.clear();
        self.bold_glyph_map.clear();
        self.ime_glyph_map.clear();
        self.icon_glyph_map.clear();
        Self::clear_texture(&self.image_atlas_texture, queue);
        Self::clear_texture(&self.glyph_atlas_texture, queue);
        queue.submit([]);
    }

    pub fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size;
    }

    pub fn set_font_family(&mut self, font_family: &str) {
        use fontdb::{Family, Query, Weight};
        if let Ok(normal_font) = Self::load_font(
            &mut self.font_db,
            &Query {
                families: &[Family::Name(font_family)],
                weight: Weight::LIGHT,
                ..Default::default()
            },
        ) {
            self.normal_font = normal_font;
        }

        if let Ok(bold_font) = Self::load_font(
            &mut self.font_db,
            &Query {
                families: &[Family::Name(font_family)],
                weight: Weight::SEMIBOLD,
                ..Default::default()
            },
        ) {
            self.bold_font = bold_font;
        }
    }

    fn load_font(font_db: &mut fontdb::Database, query: &fontdb::Query<'_>) -> Result<Font> {
        use fontdb::Source as FontSource;
        let id = font_db.query(query).context("font query failed")?;
        let (source, index) = font_db
            .face_source(id)
            .context("failed to find face source")?;
        match source {
            FontSource::File(path) => {
                Font::from_file(path, index as usize).context("failed to load font from file")
            }
            FontSource::Binary(data) => Font::from_data(data.as_ref().as_ref(), index as usize)
                .context("failed to load from memory"),
            FontSource::SharedFile(_, data) => {
                Font::from_data(data.as_ref().as_ref(), index as usize)
                    .context("failed to load from memory")
            }
        }
    }

    fn clear_texture(texture: &Texture, queue: &Queue) {
        let size = texture.size();
        let empty_data = vec![0u8; (size.width as usize * 4) * size.height as usize];
        queue.write_texture(
            TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: Origin3d { x: 0, y: 0, z: 0 },
                aspect: TextureAspect::All,
            },
            &empty_data,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size,
        );
    }

    fn finalize(&mut self, device: &Device, queue: &Queue) {
        self.maybe_grow_buffer(device);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        //queue.submit([]);
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
    }

    pub fn render(&mut self, device: &Device, queue: &Queue, pass: &mut RenderPass) {
        self.finalize(device, queue);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.draw(0..(self.vertices.len() as u32), 0..1);
        self.vertices.clear();
    }

    pub fn add_glyph(
        &mut self,
        queue: &Queue,
        pos: [f32; 2],
        screen_size: [f32; 2],
        glyph: char,
        color: Color,
        bold: bool,
    ) {
        let color = color.inner(); //.to_linear();

        let variant = if bold {
            FontVariant::Bold
        } else {
            FontVariant::Normal
        };
        let Some(glyph) = self.get_or_create_glyph(queue, variant, glyph) else {
            return;
        };

        let variant = match glyph.content {
            Content::Color => GLYPH_VARIANT_IMAGE,
            _ => GLYPH_VARIANT_GLYPH,
        };

        let ydt = self.font_size / 4.0;

        let x0 = pos[0] + glyph.placement.left as f32;
        let y0 = (pos[1] - ydt) - glyph.placement.top as f32;
        let x1 = x0 + glyph.placement.width as f32;
        let y1 = y0 + glyph.placement.height as f32;

        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [
                (x / screen_size[0]) * 2.0 - 1.0,
                1.0 - (y / screen_size[1]) * 2.0,
            ]
        };

        let tl = to_ndc(x0, y0);
        let tr = to_ndc(x1, y0);
        let bl = to_ndc(x0, y1);
        let br = to_ndc(x1, y1);

        self.vertices.push(Vertex {
            pos: tl,
            uv: [glyph.uv_min[0], glyph.uv_min[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: tr,
            uv: [glyph.uv_max[0], glyph.uv_min[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: bl,
            uv: [glyph.uv_min[0], glyph.uv_max[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: tr,
            uv: [glyph.uv_max[0], glyph.uv_min[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: bl,
            uv: [glyph.uv_min[0], glyph.uv_max[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: br,
            uv: [glyph.uv_max[0], glyph.uv_max[1]],
            color,
            variant,
        });
    }

    fn add_glyph_id(
        &mut self,
        queue: &Queue,
        pos: [f32; 2],
        screen_size: [f32; 2],
        glyph: u16,
        variant: FontVariant,
        color: Color,
    ) {
        let color = color.inner(); //.to_linear();

        let Some(glyph) = self.get_or_create_glyph_id(queue, variant, glyph) else {
            return;
        };

        let variant = match glyph.content {
            Content::Color => GLYPH_VARIANT_IMAGE,
            _ => GLYPH_VARIANT_GLYPH,
        };

        let ydt = self.font_size / 4.0;

        let x0 = pos[0] + glyph.placement.left as f32;
        let y0 = (pos[1] - ydt) - glyph.placement.top as f32;
        let x1 = x0 + glyph.placement.width as f32;
        let y1 = y0 + glyph.placement.height as f32;

        let to_ndc = |x: f32, y: f32| -> [f32; 2] {
            [
                (x / screen_size[0]) * 2.0 - 1.0,
                1.0 - (y / screen_size[1]) * 2.0,
            ]
        };

        let tl = to_ndc(x0, y0);
        let tr = to_ndc(x1, y0);
        let bl = to_ndc(x0, y1);
        let br = to_ndc(x1, y1);

        self.vertices.push(Vertex {
            pos: tl,
            uv: [glyph.uv_min[0], glyph.uv_min[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: tr,
            uv: [glyph.uv_max[0], glyph.uv_min[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: bl,
            uv: [glyph.uv_min[0], glyph.uv_max[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: tr,
            uv: [glyph.uv_max[0], glyph.uv_min[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: bl,
            uv: [glyph.uv_min[0], glyph.uv_max[1]],
            color,
            variant,
        });
        self.vertices.push(Vertex {
            pos: br,
            uv: [glyph.uv_max[0], glyph.uv_max[1]],
            color,
            variant,
        });
    }

    pub fn add_cluster(
        &mut self,
        queue: &Queue,
        pos: [f32; 2],
        screen_size: [f32; 2],
        cluster: &str,
        color: Color,
        bold: bool,
    ) {
        let variant = self.text_font_variant_for_cluster(cluster, bold);
        let font = match variant {
            FontVariant::Normal => &self.normal_font,
            FontVariant::Bold => &self.bold_font,
            FontVariant::Ime => &self.ime_font,
            FontVariant::Emoji => &self.icon_font,
        };

        let mut shaper = self
            .shape_context
            .builder(font.as_ref())
            .direction(Direction::LeftToRight)
            .size(self.font_size)
            .build();

        shaper.add_str(cluster);

        let mut glyphs = SmallVec::<[_; 16]>::new_const();
        shaper.shape_with(|cluster| {
            for glyph in cluster.glyphs {
                glyphs.push((glyph.id, glyph.x, glyph.y));
            }
        });
        for (id, x, y) in glyphs {
            self.add_glyph_id(
                queue,
                [pos[0] + x, pos[1] + y],
                screen_size,
                id,
                variant,
                color,
            );
        }
    }
}
