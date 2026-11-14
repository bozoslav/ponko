//! Post-processing: what happens to the lit frame after the last model is
//! drawn, in one place.
//!
//! The post part of a frame is a fixed chain ([`POST_CHAIN`]):
//! temporal antialiasing settles the frame into the HDR image, the game's
//! scene compute passes read it, its target effects and screen effects run
//! over it, bloom blurs its bright
//! parts, the post pass tone maps it (exposure, bloom, grading), FXAA or
//! SMAA smooth it, and with a render scale a last pass scales it to the
//! window. `Renderer3D::record_post` walks the chain; each link is recorded
//! by its owner: the temporal resolve by `taa.rs`, the scene compute
//! passes by `record.rs`, the effects by `effects.rs`, the rest by [`PostChain`] here.
//!
//! [`PostChain`] owns the pipelines of its links, the post settings
//! uniform, and this frame's choices (the antialiasing mode, whether bloom
//! runs, the lookup table). Its images are [`PostTargets`], made with the
//! scene targets and replaced with them (`targets.rs`).
//!
//! # Adding a post effect
//!
//! A new link (say, a depth of field blur before bloom) is added here,
//! and declared in the frame's description (`passes.rs`), and nowhere else
//! in the renderer:
//!
//! 1. its pipeline: a field of [`PostChain`], built in [`PostChain::new`]
//!    from an entry point in `post.wgsl` (the `full_screen` helper);
//! 2. its images, if it needs any: a field of [`PostTargets`], made in
//!    [`PostChain::targets`]; if they exist only while the link is on,
//!    [`PostTargets::stale`] says when they must be made again;
//! 3. whether it runs this frame: a field set in [`PostChain::prepare`]
//!    from the game's `Post` settings;
//! 4. its pass: recorded in [`PostChain::record`] where it belongs in the
//!    chain, and named in [`POST_CHAIN`] (a test keeps that list and the
//!    source's order in step);
//! 5. its draw calls, for the frame's statistics: counted in
//!    [`PostChain::draw_calls`];
//! 6. its declaration: a step of the post pass in `passes.rs` (what it
//!    reads and writes, and the feature it is recorded under), marked
//!    where it is recorded (`passes::mark`) and switched on in
//!    [`PostChain::features`]. Debug builds then check every drawn frame
//!    against it. A link with an image of its own also declares that
//!    resource and lists the real texture in `frame_checks.rs`
//!    (`textures`), so its format and sample count are checked too.
//!
//! The renderer can't do any of this for it: the chain's choices and
//! images are private to this file, and the renderer asks only for its
//! stale check, its draw count and (for the frame checks) its features
//! and images (`post_links_render_test.rs` holds the stale check, the
//! draw count and the features for every link switched on and off).
//!
//! It reads the frame from the image the link before it wrote and writes
//! the one the next link reads; nothing in pass preparation, the model
//! passes, the shadow maps or the resources changes. An effect that needs
//! the scene's depth or motion (as GTAO and reflections do) belongs before
//! the lit pass instead, and declares what it reads in `passes.rs`.

use super::{
    BLOOM_MIPS, BLOOM_SCATTER, EffectPass, FrameParts, HDR_FORMAT, PassTimer, Renderer3D, SCENE_COMPUTE_STEPS, passes,
};
use crate::render::AntiAliasing;
use crate::render::gpu::GpuTexture;
use crate::render::smaa::Smaa;
use crate::render::traffic::{MIRRORED, Mirror, NewBuffer, Upload};
use crate::three::PostUniform;

/// The links of the post chain, in the order they run. Each reads what the
/// one before it left.
/// Documentation that a test holds the recording to.
#[cfg_attr(not(test), allow(dead_code))]
pub(super) const POST_CHAIN: [&str; 8] = [
    "temporal antialiasing",
    "scene compute passes",
    "target effects",
    "screen effects",
    "bloom",
    "tone mapping",
    "antialiasing",
    "upscale",
];

/// The post chain's own passes: bloom, tone mapping, antialiasing and scaling.
pub(super) struct PostChain {
    /// The format of the image the chain ends in (the window's).
    output_format: wgpu::TextureFormat,
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    fxaa: wgpu::RenderPipeline,
    /// Scales the scene-sized LDR frame to the window (see `PostTargets::upscale`).
    upscale: wgpu::RenderPipeline,
    bloom_prefilter: wgpu::RenderPipeline,
    bloom_down: wgpu::RenderPipeline,
    bloom_up: wgpu::RenderPipeline,
    /// The post settings as the shader reads them, and what the buffer holds.
    pub(super) buffer: wgpu::Buffer,
    pub(super) mirror: Mirror,
    linear_clamp: wgpu::Sampler,
    /// Bound as the bloom input of the bloom steps themselves.
    pub(super) black: wgpu::TextureView,
    /// This frame's lookup table texture (or white), and whether bloom runs.
    lut: u32,
    bloom_on: bool,
    /// This frame's antialiasing, and SMAA's passes while it is the mode.
    aa: AntiAliasing,
    smaa: Option<Smaa>,
}

/// The post chain's images, of the scene's size.
pub(super) struct PostTargets {
    /// With FXAA: the tone mapped frame before it, and the bind group FXAA
    /// reads it by.
    ldr: Option<wgpu::TextureView>,
    aa_group: Option<wgpu::BindGroup>,
    /// With a render scale other than 1: the tone mapped (and antialiased)
    /// frame at the scene's size, and the bind group the scaling pass reads
    /// it by. The scaling pass then draws it into the window-sized output.
    upscale: Option<wgpu::TextureView>,
    upscale_group: Option<wgpu::BindGroup>,
    /// The post pass's input: the HDR frame and the bloom.
    post: wgpu::BindGroup,
    /// `post` and `bloom_prefilter`, reading the scene texture: for when an
    /// odd number of effects ran and the frame ended up there.
    post_of_scene: wgpu::BindGroup,
    bloom_prefilter_of_scene: wgpu::BindGroup,
    /// Bloom: render target of each mip, and the bind groups of its
    /// steps: the first downsample (reading the HDR frame), the other
    /// downsamples (reading the mip above) and the upsamples (reading the
    /// mip below).
    bloom_mips: Vec<wgpu::TextureView>,
    bloom_prefilter: wgpu::BindGroup,
    bloom_down: Vec<wgpu::BindGroup>,
    bloom_up: Vec<wgpu::BindGroup>,
}

impl PostTargets {
    /// Whether these images were made for other choices than `chain`'s
    /// this frame: a link with an image of its own (FXAA's input, the
    /// scaling pass's) switched on or off. The scene's images are then
    /// made again, these with them.
    pub(super) fn stale(&self, chain: &PostChain, upscaled: bool) -> bool {
        self.ldr.is_some() != (chain.aa == AntiAliasing::Fxaa) || self.upscale.is_some() != upscaled
    }

    /// The first (largest) bloom image, the tone mapped frame FXAA reads
    /// and the scene-sized frame the scaling pass reads: for the frame
    /// checks (`frame_checks.rs`), which compare the real textures with
    /// what the passes declare.
    pub(super) fn bloom_image(&self) -> &wgpu::TextureView {
        &self.bloom_mips[0]
    }

    pub(super) fn ldr(&self) -> Option<&wgpu::TextureView> {
        self.ldr.as_ref()
    }

    pub(super) fn upscaled(&self) -> Option<&wgpu::TextureView> {
        self.upscale.as_ref()
    }

    /// Tests: whether the frame ends in the scaling pass.
    #[cfg(test)]
    pub(super) fn scales_output(&self) -> bool {
        self.upscale.is_some()
    }
}

impl PostChain {
    /// The antialiasing this frame uses (what was asked for, or its
    /// stand-in on this device).
    #[cfg(test)]
    pub(super) fn antialiasing(&self) -> AntiAliasing {
        self.aa
    }

    /// The format of the image the chain ends in.
    pub(super) fn output_format(&self) -> wgpu::TextureFormat {
        self.output_format
    }

    /// Which of the chain's optional links this frame records, as the
    /// frame's description names them (`passes.rs`): the frame checks hold
    /// the recording to it.
    pub(super) fn features(&self, images: &PostTargets) -> passes::Features {
        use passes::Features;
        Features::BLOOM
            .only_if(self.bloom_on)
            .with(Features::FXAA.only_if(self.aa == AntiAliasing::Fxaa))
            .with(Features::SMAA.only_if(self.aa == AntiAliasing::Smaa))
            .with(Features::UPSCALE.only_if(images.upscale_group.is_some()))
    }

    /// Draw calls the chain's own links take this frame, for the frame's
    /// statistics: the post pass, the bloom steps and the antialiasing
    /// passes (the scaling pass isn't counted). `images`: the chain's, once
    /// made.
    pub(super) fn draw_calls(&self, images: Option<&PostTargets>) -> usize {
        let antialiasing = match self.aa {
            AntiAliasing::None | AntiAliasing::Taa => 0,
            AntiAliasing::Fxaa => 1,
            AntiAliasing::Smaa => 3,
        };
        let bloom = if self.bloom_on {
            images.map_or(0, |t| t.bloom_mips.len() * 2 - 1)
        } else {
            0
        };
        1 + antialiasing + bloom
    }

    pub(super) fn new(
        device: &wgpu::Device,
        texture_layout: &wgpu::BindGroupLayout,
        format: wgpu::TextureFormat,
        white: u32,
    ) -> Self {
        let uniform_entry = |visibility| wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let post_shader = device.create_shader_module(wgpu::include_wgsl!("../post.wgsl"));
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("3d post layout"),
            entries: &[
                texture_entry(0),
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    ..uniform_entry(wgpu::ShaderStages::FRAGMENT)
                },
                texture_entry(3),
            ],
        });
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d post pipeline layout"),
            bind_group_layouts: &[Some(&post_layout), Some(texture_layout)],
            immediate_size: 0,
        });
        let bloom_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("3d bloom pipeline layout"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let full_screen = |label, layout, entry_point, format, blend| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: &post_shader,
                    entry_point: Some("vs_full"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &post_shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let post_pipeline = full_screen("3d post pipeline", &post_pipeline_layout, "fs_tonemap", format, None);
        let fxaa_pipeline = full_screen("FXAA", &bloom_pipeline_layout, "fs_fxaa", format, None);
        let upscale_pipeline = full_screen("3d upscale", &bloom_pipeline_layout, "fs_upscale", format, None);
        let bloom_prefilter = full_screen(
            "3d bloom prefilter",
            &bloom_pipeline_layout,
            "fs_bloom_prefilter",
            HDR_FORMAT,
            None,
        );
        let bloom_down = full_screen(
            "3d bloom down",
            &bloom_pipeline_layout,
            "fs_bloom_down",
            HDR_FORMAT,
            None,
        );
        // Each upsample is blended over the mip's own downsample:
        // lerp(mip, upsampled, scatter), the scatter set as blend constant.
        let lerp = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Constant,
            dst_factor: wgpu::BlendFactor::OneMinusConstant,
            operation: wgpu::BlendOperation::Add,
        };
        let bloom_up = full_screen(
            "bloom",
            &bloom_pipeline_layout,
            "fs_bloom_up",
            HDR_FORMAT,
            Some(wgpu::BlendState {
                color: lerp,
                alpha: lerp,
            }),
        );
        let post_buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("3d post"),
            size: size_of::<PostUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | MIRRORED,
            mapped_at_creation: false,
        });
        let linear_clamp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("3d linear clamp"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let black = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("3d black"),
                size: wgpu::Extent3d {
                    width: 1,
                    height: 1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        Self {
            output_format: format,
            layout: post_layout,
            pipeline: post_pipeline,
            fxaa: fxaa_pipeline,
            upscale: upscale_pipeline,
            bloom_prefilter,
            bloom_down,
            bloom_up,
            buffer: post_buffer,
            mirror: Mirror::default(),
            linear_clamp,
            black,
            lut: white,
            bloom_on: false,
            aa: AntiAliasing::None,
            smaa: None,
        }
    }

    /// This frame's post settings: the uniform, the lookup table, whether
    /// bloom runs, and the antialiasing (temporal antialiasing needs
    /// compute-class devices; the others get FXAA).
    pub(super) fn prepare(
        &mut self,
        queue: &wgpu::Queue,
        post: &crate::render::Post,
        lut: Option<(u32, u32)>,
        (size, window): ((u32, u32), (u32, u32)),
        white: u32,
        taa_supported: bool,
    ) {
        let post_uniform = PostUniform::new(post, lut.map(|(_, n)| n)).scaled(post, size, window);
        let post_uniform = bytemuck::bytes_of(&post_uniform);
        self.mirror
            .upload(queue, Upload::Uniforms, &self.buffer, 0, post_uniform);
        self.lut = lut.map_or(white, |(id, _)| id);
        self.bloom_on = post.bloom > 0.0;
        self.aa = match post.anti_aliasing {
            AntiAliasing::Taa if !taa_supported => AntiAliasing::Fxaa,
            aa => aa,
        };
    }

    /// SMAA's passes for a scene of `size`: made when it becomes the mode
    /// or the size changes (`made_for`: the size of the targets so far),
    /// dropped when it isn't the mode.
    pub(super) fn prepare_smaa(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        size: (u32, u32),
        made_for: Option<(u32, u32)>,
    ) {
        if made_for.is_some_and(|s| s != size) || self.aa != AntiAliasing::Smaa {
            self.smaa = None;
        }
        if self.aa == AntiAliasing::Smaa && (self.smaa.is_none() || made_for.is_none_or(|s| s != size)) {
            self.smaa = Some(Smaa::new(device, queue, size.0, size.1, self.output_format));
        }
    }

    /// The chain's images for a scene of `size`, reading the frame from
    /// `hdr` or (after an odd number of screen effects) `scene_view`.
    pub(super) fn targets(
        &self,
        device: &wgpu::Device,
        size: (u32, u32),
        upscaled: bool,
        hdr: &wgpu::TextureView,
        scene_view: &wgpu::TextureView,
    ) -> PostTargets {
        let target = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d {
                        width: size.0,
                        height: size.1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        // Bloom starts at half size and halves until about 8 pixels.
        let half = (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1));
        let mips = (32 - half.0.min(half.1).max(1).leading_zeros())
            .saturating_sub(3)
            .clamp(1, BLOOM_MIPS);
        let bloom = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d bloom"),
            size: wgpu::Extent3d {
                width: half.0,
                height: half.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let bloom_mips: Vec<wgpu::TextureView> = (0..mips)
            .map(|mip| {
                bloom.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: mip,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect();
        let group = |source: &wgpu::TextureView, bloom: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("3d post"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(source),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&self.linear_clamp),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.buffer.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 3,
                        resource: wgpu::BindingResource::TextureView(bloom),
                    },
                ],
            })
        };
        let ldr = (self.aa == AntiAliasing::Fxaa).then(|| target("3d LDR before AA", self.output_format));
        let aa_group = ldr.as_ref().map(|ldr| group(ldr, &self.black));
        let upscale = upscaled.then(|| target("3d scaled frame", self.output_format));
        let upscale_group = upscale.as_ref().map(|u| group(u, &self.black));
        let n = bloom_mips.len();
        PostTargets {
            ldr,
            aa_group,
            upscale,
            upscale_group,
            post: group(hdr, &bloom_mips[0]),
            bloom_prefilter: group(hdr, &self.black),
            post_of_scene: group(scene_view, &bloom_mips[0]),
            bloom_prefilter_of_scene: group(scene_view, &self.black),
            bloom_down: (1..n).map(|i| group(&bloom_mips[i - 1], &self.black)).collect(),
            bloom_up: (0..n - 1).map(|i| group(&bloom_mips[i + 1], &self.black)).collect(),
            bloom_mips,
        }
    }

    /// Records the chain's own links: bloom, tone mapping, antialiasing and
    /// the scaling pass, ending in `view`. `in_hdr`: whether the frame is in
    /// the HDR image (or, after an odd number of screen effects, the scene
    /// texture).
    pub(super) fn record<'t>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        images: &PostTargets,
        in_hdr: bool,
        view: &wgpu::TextureView,
        textures: &[GpuTexture],
        time: &PassTimer<'_, 't>,
    ) {
        let (post, bloom_prefilter) = if in_hdr {
            (&images.post, &images.bloom_prefilter)
        } else {
            (&images.post_of_scene, &images.bloom_prefilter_of_scene)
        };
        if self.bloom_on {
            passes::mark("bloom");
            let step = |encoder: &mut wgpu::CommandEncoder,
                        label: &'static str,
                        pipeline,
                        group: &wgpu::BindGroup,
                        target: &wgpu::TextureView,
                        load| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some(label),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: time(label),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.set_blend_constant(wgpu::Color {
                    r: BLOOM_SCATTER,
                    g: BLOOM_SCATTER,
                    b: BLOOM_SCATTER,
                    a: BLOOM_SCATTER,
                });
                pass.draw(0..3, 0..1);
            };
            let clear = wgpu::LoadOp::Clear(wgpu::Color::BLACK);
            let mips = &images.bloom_mips;
            step(
                encoder,
                "bloom",
                &self.bloom_prefilter,
                bloom_prefilter,
                &mips[0],
                clear,
            );
            for (i, group) in images.bloom_down.iter().enumerate() {
                step(encoder, "bloom", &self.bloom_down, group, &mips[i + 1], clear);
            }
            for (i, group) in images.bloom_up.iter().enumerate().rev() {
                step(encoder, "bloom", &self.bloom_up, group, &mips[i], wgpu::LoadOp::Load);
            }
        }
        // With a render scale, everything up to antialiasing is drawn
        // at the scene's size, then scaled onto the window.
        let output = images.upscale.as_ref().unwrap_or(view);
        passes::mark("tone map");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: match self.aa {
                    crate::render::AntiAliasing::None | crate::render::AntiAliasing::Taa => output,
                    crate::render::AntiAliasing::Fxaa => images.ldr.as_ref().unwrap(),
                    crate::render::AntiAliasing::Smaa => self.smaa.as_ref().unwrap().input(),
                },
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: time("post"),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, post, &[]);
        pass.set_bind_group(1, &textures[self.lut as usize].bind_group, &[]);
        pass.draw(0..3, 0..1);
        drop(pass);
        match self.aa {
            crate::render::AntiAliasing::None | crate::render::AntiAliasing::Taa => {}
            crate::render::AntiAliasing::Smaa => {
                passes::mark("smaa");
                self.smaa.as_ref().unwrap().draw(encoder, output);
            }
            crate::render::AntiAliasing::Fxaa => {
                passes::mark("fxaa");
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("FXAA"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: output,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(&self.fxaa);
                pass.set_bind_group(0, images.aa_group.as_ref().unwrap(), &[]);
                pass.draw(0..3, 0..1);
            }
        }
        if let Some(group) = &images.upscale_group {
            passes::mark("upscale");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d upscale"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: time("upscale"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.upscale);
            pass.set_bind_group(0, group, &[]);
            pass.draw(0..3, 0..1);
        }
    }
}

impl Renderer3D {
    /// The post part of the frame: walks [`POST_CHAIN`], each link recorded
    /// by its owner.
    pub(super) fn record_post(&self, encoder: &mut wgpu::CommandEncoder, frame: &FrameParts<'_, '_>) {
        let FrameParts {
            view,
            textures,
            targets,
            time,
            ..
        } = *frame;
        // Temporal antialiasing first: effects and bloom then see the
        // settled image, in `hdr`.
        if self.screen.taa_on
            && let (Some(taa), Some(images)) = (&self.screen.taa, &targets.taa)
        {
            passes::mark("taa");
            taa.draw(encoder, images, self.history.taa_frame_read, &targets.hdr, time("taa"));
        }
        // GPU buffers that keep their last frame copy what this frame drew,
        // for the next frame's motion pass, before anything changes them.
        if self.screen.velocity_on && self.resources.keeps_buffer_pasts() {
            let timestamps = time("buffer motion copy").map(|t| wgpu::ComputePassTimestampWrites {
                query_set: t.query_set,
                beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                end_of_pass_write_index: t.end_of_pass_write_index,
            });
            self.resources.record_buffer_pasts(encoder, timestamps);
        }
        // The game's scene compute passes: they read the settled frame
        // (in `hdr`) and its depth, and write their own targets or buffers,
        // so the effects below see this frame's results.
        self.record_computes(
            encoder,
            &self.lists.scene_computes,
            &targets.scene_of_hdr,
            time,
            SCENE_COMPUTE_STEPS,
        );
        let ctx = EffectPass {
            frame: &self.frame_bind_group,
            materials: &self.resources.materials,
            storage_dummy: self.resources.storage_dummy.as_ref(),
            targets,
            time,
        };
        self.effects.record_targets(encoder, &ctx);
        let in_hdr = self.effects.record_screen(encoder, &ctx);
        self.post.record(encoder, &targets.post, in_hdr, view, textures, time);
    }
}

#[cfg(test)]
mod tests {
    use super::POST_CHAIN;

    /// What records each link of [`POST_CHAIN`]: `(link, the function it is
    /// recorded from, the text that records it)`.
    const RECORDED: [(&str, &str, &str); 8] = [
        ("temporal antialiasing", "fn record_post(", "time(\"taa\")"),
        ("scene compute passes", "fn record_post(", "SCENE_COMPUTE_STEPS"),
        ("target effects", "fn record_post(", ".record_targets("),
        ("screen effects", "fn record_post(", ".record_screen("),
        ("bloom", "fn record<'t>(", "\"bloom\","),
        ("tone mapping", "fn record<'t>(", "time(\"post\")"),
        ("antialiasing", "fn record<'t>(", "Some(\"FXAA\")"),
        ("upscale", "fn record<'t>(", "time(\"upscale\")"),
    ];

    /// This file without its tests, with `\n` line ends (a Windows checkout
    /// may have `\r\n`).
    fn source() -> String {
        let all = include_str!("post.rs").replace("\r\n", "\n");
        all[..all.find("\n#[cfg(test)]\nmod tests").unwrap()].to_string()
    }

    /// Keeps `POST_CHAIN` (the documented order) and this file's source in
    /// step: the text that records each link appears, in the function that
    /// records it, in the list's order. It reads the source, not a recorded
    /// frame; the frame's real pass order is checked by `passes.rs` and the
    /// image tests.
    #[test]
    fn the_post_chain_is_recorded_in_its_documented_order() {
        let source = &source();
        assert_eq!(POST_CHAIN.map(|link| link), RECORDED.map(|(link, ..)| link));
        // `record_post` walks the chain and ends in the chain's own links.
        let walk = &source[source.find("fn record_post(").unwrap()..];
        assert!(walk.contains("self.post.record("), "the chain's own links come last");
        let mut last = ("", 0);
        for (link, function, text) in RECORDED {
            let body = &source[source.find(function).unwrap()..];
            let body = &body[..body.find("\n    }\n").unwrap()];
            let at = body
                .find(text)
                .unwrap_or_else(|| panic!("{link} is not recorded by `{function}`"));
            if last.0 == function {
                assert!(at > last.1, "{link} is recorded before the link ahead of it");
            }
            last = (function, at);
        }
    }
}
