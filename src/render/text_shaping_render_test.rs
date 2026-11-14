//! Shaped text drawn offscreen (feature `text-shaping`): Arabic, Hindi,
//! Japanese and a mixed-direction line go through `Graphics::draw_text_ex`
//! (the shaper, the glyph atlas, the sprite pipeline and pass the window
//! uses) into a texture read back here, and each picture is compared with
//! its reference in `tests/fixtures/render`. Each has a negative control:
//! the same text with what shaping does for it taken away must not match.
//! A retained UI label in Arabic is checked to start at the right. Skips
//! without a GPU (a skip is not a pass).
use glam::{Vec2, vec2};

use super::batch::SpriteInstance;
use super::gpu::{GpuTexture, Renderer, SpritePass, sprite_pipeline};
use super::render_test_support::{
    Image, TestGpu, Tolerance, assert_region, check_reference, gpu, keep_image, reference_rejects, target,
    white_textures,
};
use crate::color::Color;
use crate::ctx::Ctx;
use crate::text::{Font, Text, TextParams};

const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8UnormSrgb;
const SIZE: (u32, u32) = (256, 56);
/// Text is rasterized on the CPU and sampled texel for texel: adapters
/// differ only in the last bits of blending.
const REFERENCE: Tolerance = Tolerance {
    channel: 24,
    outliers: 0.004,
    edge: 1,
};

/// The window's sprite drawing, offscreen.
struct Rig {
    gpu: TestGpu,
    pipeline: wgpu::RenderPipeline,
    camera_layout: wgpu::BindGroupLayout,
    textures: Vec<GpuTexture>,
}

impl Rig {
    fn new() -> Option<Self> {
        let gpu = gpu()?;
        let device = &gpu.device;
        let (texture_layout, _white, mut textures) = white_textures(device, &gpu.queue);
        let camera_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&camera_layout), Some(&texture_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::include_wgsl!("sprite.wgsl"));
        // The glyph atlas, texture 1 as in the window.
        let side = Text::ATLAS_SIZE;
        let atlas = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d {
                width: side,
                height: side,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let view = atlas.create_view(&Default::default());
        textures.push(GpuTexture {
            bind_group: device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &texture_layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                ],
            }),
            texture: atlas,
            nearest: false,
            mips: None,
        });
        Some(Self {
            pipeline: sprite_pipeline(device, &layout, &shader, FORMAT),
            camera_layout,
            textures,
            gpu,
        })
    }

    /// A `Ctx` with no window drawing into this rig's two textures, with
    /// the test fonts loaded: (Arabic, Devanagari, Japanese).
    fn ctx(&self) -> (Ctx, [Font; 3]) {
        let mut ctx = Ctx::new(1.0 / 60.0, vec2(SIZE.0 as f32, SIZE.1 as f32));
        ctx.gfx.stand_in_textures();
        let mut font = |name: &str| {
            let bytes = std::fs::read(format!("{}/assets/fonts/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap();
            ctx.gfx.text.add_font(&bytes).unwrap()
        };
        let fonts = [
            font("NotoSansArabic-Subset.ttf"),
            font("NotoSansDevanagari-Subset.ttf"),
            font("NotoSansJP-Subset.otf"),
        ];
        (ctx, fonts)
    }

    /// The picture of what `ctx` drew into its UI list since the last one.
    fn shot(&self, ctx: &mut Ctx) -> Image {
        use wgpu::util::DeviceExt;
        let (device, queue) = (&self.gpu.device, &self.gpu.queue);
        for (at, size, rgba) in ctx.gfx.text.uploads.drain(..) {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.textures[1].texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: at[0],
                        y: at[1],
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &rgba,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(4 * size[0]),
                    rows_per_image: Some(size[1]),
                },
                wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
            );
        }
        let (sprites, batches) = ctx.gfx.ui_list.finish();
        assert!(!sprites.is_empty(), "nothing was drawn");
        let instances = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice::<SpriteInstance, u8>(sprites),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let size = vec2(SIZE.0 as f32, SIZE.1 as f32);
        let camera = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&crate::ctx::screen_projection(size).to_cols_array()),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let camera = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.camera_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: camera.as_entire_binding(),
            }],
        });
        let output = target(device, SIZE, FORMAT);
        let view = output.create_view(&Default::default());
        let mut encoder = device.create_command_encoder(&Default::default());
        Renderer::sprite_pass(
            &mut encoder,
            &view,
            Some(Color::BLACK),
            None,
            &SpritePass {
                viewport: [SIZE.0, SIZE.1],
                scale: 1.0,
                pipeline: &self.pipeline,
                instances: &instances,
                textures: &self.textures,
                layers: &[(&camera, batches, 0)],
            },
        );
        queue.submit([encoder.finish()]);
        let image = Image::read(device, queue, &output);
        ctx.gfx.ui_list.clear();
        keep_image(&image);
        image
    }

    /// `text` in `font` at 24 px, white, 8 px in from the top left.
    fn draw(&self, ctx: &mut Ctx, text: &str, font: Font) -> Image {
        ctx.gfx.draw_text_ex(
            text,
            vec2(8.0, 8.0),
            TextParams::default().with_font(font).with_size(24.0),
        );
        self.shot(ctx)
    }
}

fn lit(p: [u8; 4]) -> bool {
    p[0] > 128
}

/// The columns that have any text in them: (leftmost, rightmost).
fn ink_columns(image: &Image) -> (u32, u32) {
    let columns: Vec<u32> = (0..image.width)
        .filter(|&x| (0..image.height).any(|y| lit(image.pixel(x, y))))
        .collect();
    (*columns.first().unwrap(), *columns.last().unwrap())
}

const SCENE: &str = "white text, 24 px line height, top left corner at (8, 8), on black; 256x56, drawn with the \
                     window's sprite pipeline into an sRGB target";

#[test]
fn arabic_is_drawn_joined_right_to_left() {
    let Some(rig) = Rig::new() else { return };
    let (mut ctx, [arabic, ..]) = rig.ctx();
    let image = rig.draw(&mut ctx, "مرحبا بالعالم", arabic);
    let (left, right) = ink_columns(&image);
    assert!(left >= 8 && right > left + 80, "the words are drawn: {left}..{right}");
    assert_region(&image, 0..8, 0..SIZE.1, 1.0, "nothing left of the text's origin", |p| {
        !lit(p)
    });
    check_reference(
        "text_shaping_arabic",
        &image,
        REFERENCE,
        &format!("\"مرحبا بالعالم\" (hello world) in Noto Sans Arabic, {SCENE}"),
        &rig.gpu.info,
    );
    // The same letters kept apart by zero-width non-joiners: each in its
    // isolated form, as a renderer without shaping draws them.
    let apart: String = "مرحبا بالعالم".chars().flat_map(|c| [c, '\u{200C}']).collect();
    let unjoined = rig.draw(&mut ctx, &apart, arabic);
    reference_rejects("text_shaping_arabic", &unjoined, REFERENCE, "the letters unjoined");
}

#[test]
fn hindi_is_drawn_with_its_conjuncts_and_reordered_vowels() {
    let Some(rig) = Rig::new() else { return };
    let (mut ctx, [_, devanagari, _]) = rig.ctx();
    let image = rig.draw(&mut ctx, "नमस्ते दुनिया", devanagari);
    let (left, right) = ink_columns(&image);
    assert!(left >= 8 && right > left + 80, "the words are drawn: {left}..{right}");
    check_reference(
        "text_shaping_hindi",
        &image,
        REFERENCE,
        &format!("\"नमस्ते दुनिया\" (hello world) in Noto Sans Devanagari, {SCENE}"),
        &rig.gpu.info,
    );
    // Zero-width non-joiners after each character break the conjunct
    // "स्ते" and the vowel signs' placement.
    let apart: String = "नमस्ते दुनिया".chars().flat_map(|c| [c, '\u{200C}']).collect();
    let broken = rig.draw(&mut ctx, &apart, devanagari);
    reference_rejects("text_shaping_hindi", &broken, REFERENCE, "the clusters broken apart");
}

#[test]
fn japanese_comes_from_the_fallback_font() {
    let Some(rig) = Rig::new() else { return };
    let (mut ctx, [.., japanese]) = rig.ctx();
    // Drawn in the built-in font, which has no Japanese: without its
    // fallback chain every character is the font's empty box.
    let boxes = rig.draw(&mut ctx, "日本語のテキスト", Font::DEFAULT);
    ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[japanese]);
    let image = rig.draw(&mut ctx, "日本語のテキスト", Font::DEFAULT);
    let (left, right) = ink_columns(&image);
    assert!(
        left >= 8 && right > left + 150,
        "eight characters are drawn: {left}..{right}"
    );
    check_reference(
        "text_shaping_japanese",
        &image,
        REFERENCE,
        &format!(
            "\"日本語のテキスト\" (Japanese text) in the built-in font with Noto Sans JP as its fallback, {SCENE}"
        ),
        &rig.gpu.info,
    );
    reference_rejects("text_shaping_japanese", &boxes, REFERENCE, "no fallback font");
}

#[test]
fn a_mixed_direction_line_keeps_each_run_in_reading_order() {
    let Some(rig) = Rig::new() else { return };
    let (mut ctx, [arabic, ..]) = rig.ctx();
    ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic]);
    let text = "Level 3: مرحبا بالعالم!";
    let image = rig.draw(&mut ctx, text, Font::DEFAULT);
    // "Level 3:" is on the left: its width in the built-in font.
    let latin = ctx.gfx.measure_text("Level 3:", 24.0).x;
    let (left, right) = ink_columns(&image);
    assert!((8..12).contains(&left), "the Latin run starts the line: {left}");
    assert!(right as f32 > 8.0 + latin + 80.0, "the Arabic run follows it: {right}");
    check_reference(
        "text_shaping_mixed",
        &image,
        REFERENCE,
        &format!(
            "\"Level 3: مرحبا بالعالم!\" in the built-in font with Noto Sans Arabic as its fallback: a left-to-right \
             line with a right-to-left run, {SCENE}"
        ),
        &rig.gpu.info,
    );
    // A left-to-right override (U+202D) lays the Arabic words out in
    // memory order, the way text without bidirectional ordering comes out.
    let forced = rig.draw(&mut ctx, &format!("\u{202D}{text}\u{202C}"), Font::DEFAULT);
    reference_rejects("text_shaping_mixed", &forced, REFERENCE, "the Arabic run left to right");
}

#[test]
fn a_retained_label_in_arabic_starts_at_the_right() {
    let Some(rig) = Rig::new() else { return };
    let size = Vec2::new(SIZE.0 as f32, SIZE.1 as f32);
    let draw = |text: &str| {
        let (mut ctx, [arabic, ..]) = rig.ctx();
        ctx.gfx.set_font_fallbacks(Font::DEFAULT, &[arabic]);
        let root = ctx.ui.root().layout(crate::ui::UiLayout::fill()).id();
        ctx.ui.label(root, text).layout(crate::ui::UiLayout::row(32.0));
        ctx.ui.push_screen(root, false);
        ctx.ui.layout(size);
        ctx.draw_retained_ui();
        rig.shot(&mut ctx)
    };
    // The same alignment (Start) for both: Arabic from the right edge,
    // English from the left.
    let (left, right) = ink_columns(&draw("مرحبا"));
    assert!(
        right > SIZE.0 - 24 && left > SIZE.0 / 2,
        "the Arabic label is at the right: {left}..{right}"
    );
    let (left, right) = ink_columns(&draw("Hello"));
    assert!(
        left < 24 && right < SIZE.0 / 2,
        "the English label is at the left: {left}..{right}"
    );
}
