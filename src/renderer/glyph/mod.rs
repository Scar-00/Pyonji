mod system;

pub use system::{DEFAULT_FAMILIES, prewarm as prewarm_system_fonts, resolve_family};

use std::{borrow::Cow, mem, sync::Arc};

use bytemuck::{Pod, Zeroable};
use etagere::{AtlasAllocator, BucketedAtlasAllocator};
use smallvec::SmallVec;
use swash::{
    CacheKey, Charmap, FontRef,
    scale::{
        Render, ScaleContext, Source, StrikeWith,
        image::{Content, Image},
    },
    shape::{Direction, ShapeContext},
    zeno::{Format, Placement, Vector},
};
use wgpu::{
    AddressMode, BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferAddress,
    BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState,
    IndexFormat, MipmapFilterMode, MultisampleState, Origin3d, PipelineCompilationOptions,
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

// NOTE: GPUI composites in pass-through sRGB (see background.rs): glyph
// colors arrive as sRGB-encoded bytes, so just normalize them instead of
// converting to linear (which would darken all text).
@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4<f32>(in.pos, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    out.variant = in.variant;
    return out;
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    switch in.variant {
        case 0u: {
            let tex = textureSample(glyph_tex, glyph_samp, vec2<f32>(in.uv));
            var alpha = pow(tex.r, 1.43);
            let color = vec4<f32>(in.color) / 255.0;
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
    Icon,
    Emoji,
}

type FontData = Arc<dyn AsRef<[u8]> + Send + Sync>;

/// A face's bytes, whether embedded, mapped from an installed file, or
/// shared with fontdb. None of them are copied.
pub struct Font {
    data: FontData,
    offset: u32,
    key: CacheKey,
}

impl Font {
    fn from_data(data: impl AsRef<[u8]> + Send + Sync + 'static, index: usize) -> Option<Self> {
        Self::from_shared(Arc::new(data), index)
    }

    fn from_shared(data: FontData, index: usize) -> Option<Self> {
        let font = FontRef::from_index((*data).as_ref(), index)?;
        let (offset, key) = (font.offset, font.key);
        Some(Self { data, offset, key })
    }

    pub fn charmap(&self) -> Charmap<'_> {
        self.as_ref().charmap()
    }

    pub fn as_ref(&self) -> FontRef<'_> {
        FontRef {
            data: (*self.data).as_ref(),
            offset: self.offset,
            key: self.key,
        }
    }

    fn size_for_width(&self, width: f32) -> f32 {
        let metrics = self.as_ref().metrics(&[]);
        width * f32::from(metrics.units_per_em) / metrics.max_width
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
    /// One column's width in pixels; see [`TerminalRenderer::measure_cell_width`].
    cell_width: f32,
    /// The installed family the normal and bold faces come from, so
    /// reloading an unchanged config doesn't load them again.
    font_family: String,

    _shader: ShaderModule,
    pipeline: RenderPipeline,
    vertex_buffer: Buffer,
    index_buffer: Buffer,
    vertices: Vec<Vertex>,
    indices: Vec<u16>,
    glyph_atlas_texture: Texture,
    image_atlas_texture: Texture,

    uniform_bind_group: BindGroup,

    normal_font: Font,
    bold_font: Font,
    /// `None` when no CJK family is installed.
    ime_font: Option<Font>,
    icon_font: Font,
    emoji_font: Font,
    scale_context: ScaleContext,
    shape_context: ShapeContext,
    normal_glyph_map: Vec<Option<Glyph>>,
    bold_glyph_map: Vec<Option<Glyph>>,
    ime_glyph_map: Vec<Option<Glyph>>,
    icon_glyph_map: Vec<Option<Glyph>>,
    emoji_glyph_map: Vec<Option<Glyph>>,
    glyph_atlas: BucketedAtlasAllocator,
    image_atlas: AtlasAllocator,
    atlas_size: [f32; 2],
}

impl TerminalRenderer {
    /// Nerd Font icons, which no platform ships. Also draws text when no
    /// monospaced font is installed.
    pub(crate) const ICON_FONT: &[u8] =
        include_bytes!("../../../resources/fonts/JetBrainsMonoNerdFontMono-Regular.ttf");
    const EMOJI_FONT: &[u8] = include_bytes!("../../../resources/fonts/NotoEmoji.ttf");
    const DEFAULT_BUFFER_SIZE: u64 = (1024 * 16) * 32;
    const MAX_ATLAS_DIMENSION: u32 = 2048;

    fn load_glyph(&mut self, variant: FontVariant, id: u16) -> Option<Image> {
        let font = match variant {
            FontVariant::Normal => &self.normal_font,
            FontVariant::Bold => &self.bold_font,
            FontVariant::Ime => self.ime_font.as_ref().unwrap_or(&self.normal_font),
            FontVariant::Icon => &self.icon_font,
            FontVariant::Emoji => &self.emoji_font,
        };
        let font_size = self.font_size_for_variant(variant);
        let mut scaler = self
            .scale_context
            .builder(font.as_ref())
            .size(font_size)
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
            // Without a CJK family, nothing is routed here: the normal face
            // was already tried first.
            FontVariant::Ime => self.ime_font.as_ref().unwrap_or(&self.normal_font),
            FontVariant::Icon => &self.icon_font,
            FontVariant::Emoji => &self.emoji_font,
        }
    }

    fn font_size_for_variant(&self, variant: FontVariant) -> f32 {
        if variant == FontVariant::Emoji {
            // Noto Emoji's advance is wider than one em, so fit its outlines
            // to the two terminal cells an emoji spans.
            self.emoji_font.size_for_width(2.0 * self.cell_width)
        } else {
            self.font_size
        }
    }

    fn glyph_map(&self, variant: FontVariant) -> &Vec<Option<Glyph>> {
        match variant {
            FontVariant::Normal => &self.normal_glyph_map,
            FontVariant::Bold => &self.bold_glyph_map,
            FontVariant::Ime => &self.ime_glyph_map,
            FontVariant::Icon => &self.icon_glyph_map,
            FontVariant::Emoji => &self.emoji_glyph_map,
        }
    }

    fn glyph_map_mut(&mut self, variant: FontVariant) -> &mut Vec<Option<Glyph>> {
        match variant {
            FontVariant::Normal => &mut self.normal_glyph_map,
            FontVariant::Bold => &mut self.bold_glyph_map,
            FontVariant::Ime => &mut self.ime_glyph_map,
            FontVariant::Icon => &mut self.icon_glyph_map,
            FontVariant::Emoji => &mut self.emoji_glyph_map,
        }
    }

    fn text_font_variant_for_cluster(&self, cluster: &str, bold: bool) -> FontVariant {
        let text_variant = if bold {
            FontVariant::Bold
        } else {
            FontVariant::Normal
        };
        Self::font_variant_for_cluster(
            cluster,
            &[
                (text_variant, self.font(text_variant)),
                (FontVariant::Ime, self.font(FontVariant::Ime)),
                (FontVariant::Icon, &self.icon_font),
                (FontVariant::Emoji, &self.emoji_font),
            ],
        )
    }

    fn font_variant_for_cluster(cluster: &str, fonts: &[(FontVariant, &Font)]) -> FontVariant {
        fonts
            .iter()
            .find_map(|(variant, font)| {
                Self::font_supports_cluster(font, cluster).then_some(*variant)
            })
            .unwrap_or(FontVariant::Icon)
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
        font_family: &str,
        font_size: f32,
        format: TextureFormat,
    ) -> Self {
        // GPU limits describe supported dimensions, not a useful atlas size.
        // Bound allocations and full-atlas uploads when reloading the config.
        let atlas_dimension = device
            .limits()
            .max_texture_dimension_2d
            .min(Self::MAX_ATLAS_DIMENSION);

        let glyph_atlas_allocator = BucketedAtlasAllocator::new(etagere::size2(
            atlas_dimension.cast_signed(),
            atlas_dimension.cast_signed(),
        ));

        let image_atlas_allocator = AtlasAllocator::new(etagere::size2(
            atlas_dimension.cast_signed(),
            atlas_dimension.cast_signed(),
        ));

        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("selection shader"),
            source: ShaderSource::Wgsl(Cow::Borrowed(SHADER_SRC)),
        });

        let glyph_texture = device.create_texture(&TextureDescriptor {
            label: Some("font-atlas-texture"),
            size: Extent3d {
                width: atlas_dimension,
                height: atlas_dimension,
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
                width: atlas_dimension,
                height: atlas_dimension,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            // NOTE: must stay non-sRGB. An Srgb format would make sampling
            // decode to linear, darkening color glyphs on GPUI's
            // pass-through (Bgra8Unorm, no conversions) pipeline.
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        let image_view = image_texture.create_view(&TextureViewDescriptor::default());

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
            label: Some("glyph vertices"),
            size: Self::DEFAULT_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("glyph indices"),
            size: Self::DEFAULT_BUFFER_SIZE,
            usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let (normal_font, bold_font) = Self::text_faces(font_family);
        let icon_font = Font::from_data(Self::ICON_FONT, 0).unwrap();
        let emoji_font = Font::from_data(Self::EMOJI_FONT, 0).unwrap();

        Self {
            font_size,
            cell_width: Self::measure_cell_width(&normal_font, font_size),
            font_family: font_family.to_owned(),

            _shader: shader,
            pipeline,
            vertex_buffer,
            index_buffer,
            vertices: Vec::new(),
            indices: Vec::new(),
            uniform_bind_group,
            glyph_atlas_texture: glyph_texture,
            image_atlas_texture: image_texture,

            normal_glyph_map: Vec::new(),
            bold_glyph_map: Vec::new(),
            ime_glyph_map: Vec::new(),
            icon_glyph_map: Vec::new(),
            emoji_glyph_map: Vec::new(),
            scale_context: ScaleContext::new(),
            shape_context: ShapeContext::new(),
            normal_font,
            bold_font,
            ime_font: system::cjk_face(),
            icon_font,
            emoji_font,
            glyph_atlas: glyph_atlas_allocator,
            image_atlas: image_atlas_allocator,
            atlas_size: [atlas_dimension as f32, atlas_dimension as f32],
        }
    }

    pub fn evict_glyphs(&mut self) {
        // New glyph uploads overwrite reused regions before they are rendered.
        self.glyph_atlas.clear();
        self.image_atlas.clear();
        self.normal_glyph_map.clear();
        self.bold_glyph_map.clear();
        self.ime_glyph_map.clear();
        self.icon_glyph_map.clear();
        self.emoji_glyph_map.clear();
    }

    pub fn set_font_size(&mut self, font_size: f32) {
        self.font_size = font_size;
        self.cell_width = Self::measure_cell_width(&self.normal_font, font_size);
    }

    pub fn cell_width(&self) -> f32 {
        self.cell_width
    }

    /// The regular face's advance at `font_size`, rounded so columns start on
    /// whole pixels. Monospaced faces differ in width (Iosevka is half an em,
    /// Cascadia Mono nearly 0.6), so a fixed fraction of the font size would
    /// overlap or space out their glyphs.
    fn measure_cell_width(font: &Font, font_size: f32) -> f32 {
        let font = font.as_ref();
        let advance = font
            .glyph_metrics(&[])
            .scale(font_size)
            .advance_width(font.charmap().map('0'));
        advance.round().max(1.0)
    }

    /// Draws the grid in an installed `font_family`, as resolved by
    /// [`resolve_family`].
    pub fn set_font_family(&mut self, font_family: &str) {
        if self.font_family == font_family {
            return;
        }
        (self.normal_font, self.bold_font) = Self::text_faces(font_family);
        self.cell_width = Self::measure_cell_width(&self.normal_font, self.font_size);
        self.font_family = font_family.to_owned();
        self.evict_glyphs();
    }

    /// The regular and bold faces of `font_family`, or the bundled icon
    /// font's when it can't be loaded.
    fn text_faces(font_family: &str) -> (Font, Font) {
        system::text_faces(font_family).unwrap_or_else(|| {
            let bundled = || Font::from_data(Self::ICON_FONT, 0).unwrap();
            (bundled(), bundled())
        })
    }

    fn finalize(&mut self, device: &Device, queue: &Queue) {
        self.maybe_grow_buffer(device);
        queue.write_buffer(&self.vertex_buffer, 0, bytemuck::cast_slice(&self.vertices));
        queue.write_buffer(&self.index_buffer, 0, bytemuck::cast_slice(&self.indices));
    }

    fn maybe_grow_buffer(&mut self, device: &Device) {
        if self.vertices.len() * mem::size_of::<Vertex>() >= self.vertex_buffer.size() as usize {
            self.vertex_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glyph vertices"),
                size: (self.vertices.len() * mem::size_of::<Vertex>()) as u64,
                usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let index_size = self.indices.len() * mem::size_of::<u16>();
        if index_size >= self.index_buffer.size() as usize {
            self.index_buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("glyph indices"),
                size: index_size as u64,
                usage: BufferUsages::INDEX | BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
    }

    pub fn render(&mut self, device: &Device, queue: &Queue, pass: &mut RenderPass) {
        self.finalize(device, queue);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.uniform_bind_group, &[]);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        pass.set_index_buffer(self.index_buffer.slice(..), IndexFormat::Uint16);
        pass.draw_indexed(0..self.indices.len() as u32, 0, 0..1);
        self.vertices.clear();
        self.indices.clear();
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
        let color = color.inner();

        let mut cluster = [0; 4];
        let variant = self.text_font_variant_for_cluster(glyph.encode_utf8(&mut cluster), bold);
        let Some(glyph) = self.get_or_create_glyph(queue, variant, glyph) else {
            return;
        };

        let variant = match glyph.content {
            Content::Color => GLYPH_VARIANT_IMAGE,
            _ => GLYPH_VARIANT_GLYPH,
        };

        let ydt = self.font_size / 4.0;

        let x0 = (pos[0] + glyph.placement.left as f32).round();
        let y0 = ((pos[1] - ydt) - glyph.placement.top as f32).round();
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

        let idx = self.vertices.len() as u16;
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
            pos: br,
            uv: [glyph.uv_max[0], glyph.uv_max[1]],
            color,
            variant,
        });
        self.indices
            .extend_from_slice(&[idx, idx + 1, idx + 2, idx + 1, idx + 2, idx + 3]);
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
        let color = color.inner();

        let Some(glyph) = self.get_or_create_glyph_id(queue, variant, glyph) else {
            return;
        };

        let variant = match glyph.content {
            Content::Color => GLYPH_VARIANT_IMAGE,
            _ => GLYPH_VARIANT_GLYPH,
        };

        let ydt = self.font_size / 4.0;

        let x0 = (pos[0] + glyph.placement.left as f32).round();
        let y0 = ((pos[1] - ydt) - glyph.placement.top as f32).round();
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

        let idx = self.vertices.len() as u16;
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
            pos: br,
            uv: [glyph.uv_max[0], glyph.uv_max[1]],
            color,
            variant,
        });
        self.indices
            .extend_from_slice(&[idx, idx + 1, idx + 2, idx + 1, idx + 2, idx + 3]);
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
            FontVariant::Ime => self.ime_font.as_ref().unwrap_or(&self.normal_font),
            FontVariant::Icon => &self.icon_font,
            FontVariant::Emoji => &self.emoji_font,
        };
        let font_size = self.font_size_for_variant(variant);

        let mut shaper = self
            .shape_context
            .builder(font.as_ref())
            .direction(Direction::LeftToRight)
            .size(font_size)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_fallbacks_cover_emoji_and_preserve_text() {
        // The bundled icon face stands in for the installed text face, so the
        // test doesn't depend on the fonts of the machine running it.
        let text = Font::from_data(TerminalRenderer::ICON_FONT, 0).unwrap();
        let icon = Font::from_data(TerminalRenderer::ICON_FONT, 0).unwrap();
        let emoji = Font::from_data(TerminalRenderer::EMOJI_FONT, 0).unwrap();

        for text_variant in [FontVariant::Normal, FontVariant::Bold] {
            let fonts = [
                (text_variant, &text),
                // No CJK family installed: the text face is tried again.
                (FontVariant::Ime, &text),
                (FontVariant::Icon, &icon),
                (FontVariant::Emoji, &emoji),
            ];
            for (cluster, expected) in [
                ("A", text_variant),
                ("\u{f015}", text_variant),
                // Covered by nothing: the icon face draws its missing glyph.
                ("한", FontVariant::Icon),
                ("📦", FontVariant::Emoji),
                ("📦\u{fe0f}", FontVariant::Emoji),
                ("👩\u{200d}💻", FontVariant::Emoji),
                ("👍🏽", FontVariant::Emoji),
                ("🇩🇪", FontVariant::Emoji),
            ] {
                assert_eq!(
                    TerminalRenderer::font_variant_for_cluster(cluster, &fonts),
                    expected,
                    "{cluster}"
                );
            }
        }
    }

    #[test]
    fn cells_are_as_wide_as_the_text_face_advance() {
        // JetBrains Mono advances 600 of its 1000 units per em.
        let font = Font::from_data(TerminalRenderer::ICON_FONT, 0).unwrap();
        assert_eq!(TerminalRenderer::measure_cell_width(&font, 24.0), 14.0);
        assert_eq!(TerminalRenderer::measure_cell_width(&font, 38.0), 23.0);
    }

    #[test]
    fn bundled_emoji_shape_and_rasterize_as_monochrome_masks() {
        let font = Font::from_data(TerminalRenderer::EMOJI_FONT, 0).unwrap();
        let cell_span = 38.0;
        let font_size = font.size_for_width(cell_span);
        let mut shape_context = ShapeContext::new();
        let mut scale_context = ScaleContext::new();
        for cluster in ["📦", "📦\u{fe0f}", "👩\u{200d}💻", "👍🏽", "🇩🇪"] {
            let mut shaper = shape_context
                .builder(font.as_ref())
                .direction(Direction::LeftToRight)
                .size(font_size)
                .build();
            shaper.add_str(cluster);
            let mut glyph_ids = Vec::new();
            shaper.shape_with(|cluster| {
                glyph_ids.extend(cluster.glyphs.iter().map(|glyph| glyph.id))
            });
            assert_eq!(glyph_ids.len(), 1, "{cluster} should shape as one emoji");
            let id = glyph_ids[0];
            assert_ne!(id, 0, "{cluster} must not use the missing glyph");
            let mut scaler = scale_context
                .builder(font.as_ref())
                .size(font_size)
                .hint(true)
                .build();
            let image = Render::new(&[
                Source::ColorOutline(0),
                Source::ColorBitmap(StrikeWith::BestFit),
                Source::Outline,
            ])
            .format(Format::Alpha)
            .render(&mut scaler, id)
            .unwrap();
            assert_eq!(image.content, Content::Mask, "{cluster}");
            assert!(image.placement.left >= 0, "{cluster}");
            assert!(
                image.placement.left as f32 + image.placement.width as f32 <= cell_span,
                "{cluster} must fit two terminal cells"
            );
            assert_eq!(
                image.data.len(),
                (image.placement.width * image.placement.height) as usize
            );
            assert!(image.data.iter().any(|&alpha| alpha != 0), "{cluster}");
        }
    }
}
