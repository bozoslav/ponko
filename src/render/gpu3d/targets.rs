//! What the passes draw into and bind: the scene-sized targets, the render
//! targets of target cameras and compute passes, and the sources every
//! frame bind group is made of.
//!
//! - [`Targets`] are the main view's images, all of one size (the window's
//!   times the render scale). They are made by `Renderer3D::prepare` when
//!   the size or an effect's on/off state changes (`targets_stale`) and
//!   replaced whole; the old ones are freed once the commands that used
//!   them have run. Effects' temporal history lives in them, so replacing
//!   them resets the history (see `history.rs`).
//! - [`RenderTargets`] holds the game's render target textures by id and
//!   what target cameras and compute passes need of them, made the first
//!   time one is used and dropped when the texture is freed.
//! - [`FrameSources`] holds the views bound in every frame bind group
//!   (shadow maps, sky, decal images, probe atlas). Whoever swaps one of
//!   them makes the frame bind groups again (`Renderer3D::rebind_frames`).

use super::{DEPTH_FORMAT, FrameUniform, HDR_FORMAT, PostTargets, Renderer3D, taa};
use crate::render::lights::LightTextures;
use crate::render::traffic::{MIRRORED, Mirror, NewBuffer};

/// The game's render targets, and what drawing into them takes.
#[derive(Default)]
pub(super) struct RenderTargets {
    /// Every render target's texture, by texture id.
    pub(super) textures: Vec<(u32, wgpu::Texture)>,
    /// What target cameras need of the render targets they draw into,
    /// made the first time one does, by texture id.
    pub(super) views: Vec<(u32, ViewTarget)>,
    /// What compute passes need of the render targets they run on, made
    /// the first time one does, by texture id.
    pub(super) computes: Vec<(u32, ComputeState)>,
}

impl RenderTargets {
    /// Keeps render target `id` (`texture`, in `HDR_FORMAT`), for target
    /// cameras and compute passes.
    pub(super) fn add(&mut self, id: u32, texture: &wgpu::Texture) {
        self.textures.push((id, texture.clone()));
    }

    /// Render target `id` was freed: target cameras and compute passes let
    /// go of it and of what they made for it.
    pub(super) fn free(&mut self, id: u32) {
        self.textures.retain(|&(t, _)| t != id);
        self.views.retain(|&(t, _)| t != id);
        self.computes.retain(|&(t, _)| t != id);
    }

    /// The index in `views` of render target `id`, made on first use.
    pub(super) fn view_target(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        samples: u32,
        sources: &FrameSources,
    ) -> Option<usize> {
        if let Some(i) = self.views.iter().position(|&(t, _)| t == id) {
            return Some(i);
        }
        let texture = &self.textures.iter().find(|&&(t, _)| t == id)?.1;
        let target = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: texture.size(),
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let frame_buffer = device.new_buffer(&wgpu::BufferDescriptor {
            label: Some("3d target camera frame"),
            size: size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST | MIRRORED,
            mapped_at_creation: false,
        });
        let lights = LightTextures::new(device);
        let frame_bind_group = sources.bind_group(device, &frame_buffer, &lights);
        let view = ViewTarget {
            color: target("3d target camera color", HDR_FORMAT),
            depth: target("3d target camera depth", DEPTH_FORMAT),
            resolve: texture.create_view(&Default::default()),
            frame_buffer,
            frame_mirror: Mirror::default(),
            frame_bind_group,
            lights,
        };
        self.views.push((id, view));
        Some(self.views.len() - 1)
    }

    /// The index in `computes` of render target `id`, made on first use.
    pub(super) fn compute_state(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        layout: Option<&wgpu::BindGroupLayout>,
    ) -> Option<usize> {
        if let Some(i) = self.computes.iter().position(|&(t, _)| t == id) {
            return Some(i);
        }
        let texture = &self.textures.iter().find(|&&(t, _)| t == id)?.1;
        let scratch = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("3d compute scratch"),
            size: texture.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d compute"),
            layout: layout?,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&scratch.create_view(&Default::default())),
                },
            ],
        });
        let state = ComputeState {
            target: texture.clone(),
            scratch,
            group,
        };
        self.computes.push((id, state));
        Some(self.computes.len() - 1)
    }
}

impl Renderer3D {
    /// Binds decal texture array `id` (`None`: the stand-in) in the frame
    /// bind groups' source. True when it changed, so they must be remade.
    pub(super) fn bind_decal_array(&mut self, id: Option<u32>) -> bool {
        let id = id.filter(|&id| (id as usize) < self.resources.arrays.len());
        if !self.frame_sources.decals || id == self.decal_array {
            return false;
        }
        self.decal_array = id;
        if let Some(id) = id {
            self.frame_sources.decal_view = self.resources.arrays[id as usize].clone();
        } else {
            self.frame_sources.decal_view = self.no_decal_view.clone();
        }
        true
    }

    /// Makes every frame bind group again (the main view's and each target
    /// camera's) from `frame_sources`, after a shadow map changed.
    pub(super) fn rebind_frames(&mut self, device: &wgpu::Device) {
        self.frame_bind_group = self.frame_sources.bind_group(device, &self.frame_buffer, &self.lights);
        for (_, target) in &mut self.render_targets.views {
            target.frame_bind_group = self
                .frame_sources
                .bind_group(device, &target.frame_buffer, &target.lights);
        }
        self.rebind_gi(device);
    }

    /// Makes global illumination's probe views' frame bind groups again.
    pub(super) fn rebind_gi(&mut self, device: &wgpu::Device) {
        if let Some(gi) = &mut self.gi {
            gi.set_sky(device, &self.frame_sources.sky_view, &self.frame_sources.sky_sampler);
            gi.face_groups = gi
                .face_buffers
                .iter()
                .map(|b| {
                    self.frame_sources
                        .bind_group_shadowed(device, b, &gi.lights, &gi.shadow_array)
                })
                .collect();
        }
    }

    /// Whether the targets were made for other settings than these: a
    /// different size (window or render scale), or an effect switched on or
    /// off, or another MSAA count. Pipelines and attachments must agree on
    /// samples; histories must agree on the scene size. A presentation
    /// interruption sets targets to None, forcing this same reset contract.
    /// They are then made again, and the history with them.
    pub(super) fn targets_stale(
        &self,
        size: (u32, u32),
        post: &crate::render::Post,
        ao_on: bool,
        upscaled: bool,
    ) -> bool {
        self.targets.as_ref().is_none_or(|t| {
            t.size != size
                || t.samples != self.samples
                || t.ao.is_some() != self.screen.ao_enabled
                || t.ao
                    .as_ref()
                    .is_some_and(|a| a.half != (post.ao_half_resolution && ao_on))
                || t.ao.as_ref().is_some_and(|a| a.temporal != (post.ao_temporal && ao_on))
                || t.ssr.is_some() != self.screen.ssr_on
                || t.taa.is_some() != self.screen.taa_on
                || t.velocity.is_some() != self.screen.velocity_on
                || t.post.stale(&self.post, upscaled)
        })
    }

    /// Makes the scene-sized targets for `size` and the effects that are on
    /// this frame (decided earlier in `prepare`).
    pub(super) fn make_targets(&self, device: &wgpu::Device, size: (u32, u32), upscaled: bool) -> Targets {
        let texture = |label, format, samples, usage| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let target = |label, format, samples, usage| {
            texture(label, format, samples, wgpu::TextureUsages::RENDER_ATTACHMENT | usage)
                .create_view(&Default::default())
        };
        let none = wgpu::TextureUsages::empty();
        let hdr_texture = texture(
            "3d hdr",
            HDR_FORMAT,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        );
        let hdr = hdr_texture.create_view(&Default::default());
        let scene_texture = texture(
            "3d scene copy",
            HDR_FORMAT,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
        );
        let scene_view = scene_texture.create_view(&Default::default());
        let depth = target(
            "3d depth",
            DEPTH_FORMAT,
            self.samples,
            wgpu::TextureUsages::TEXTURE_BINDING,
        );
        // Where the transparent pass can't sample the depth it tests
        // against (the GL backend), it samples a copy.
        let depth_copy = self.depth_copy.as_ref().map(|c| c.target(device, &depth, size));
        let scene = scene_bind_group(
            device,
            &self.scene_layout,
            &scene_view,
            depth_copy.as_ref().map_or(&depth, |c| &c.view),
            self.scene_depth_sampler.as_ref(),
        );
        let scene_of_hdr = scene_bind_group(
            device,
            &self.scene_layout,
            &hdr,
            &depth,
            self.scene_depth_sampler.as_ref(),
        );
        let velocity = self
            .screen
            .velocity_on
            .then(|| taa::VelocityTargets::new(device, size, self.samples));
        let taa = self
            .screen
            .taa
            .as_ref()
            .filter(|_| self.screen.taa_on)
            .map(|t| t.targets(device, size, velocity.as_ref().unwrap().velocity(), &depth));
        let lit = match &taa {
            Some(t) => &t.input_view,
            None => &hdr,
        };
        let ssr = self
            .screen
            .ssr
            .as_ref()
            .filter(|_| self.screen.ssr_on)
            .map(|s| s.targets(device, size, &depth, lit, self.screen.ao_enabled));
        // With reflections, GTAO writes the first level of their image.
        let ao = self.screen.ao.as_ref().filter(|_| self.screen.ao_enabled).map(|ao| {
            ao.targets(
                device,
                size,
                &depth,
                &self.post.black,
                ssr.as_ref().map(|s| &s.ao),
                velocity.as_ref().map(|v| v.velocity()).unwrap_or(&self.post.black),
            )
        });
        // The opaque pass reads both from its scene color slot (see `ssr.rs`).
        let screen = ssr.as_ref().map(|s| &s.screen).or(ao.as_ref().map(|a| &a.output));
        let opaque_scene = screen.map(|screen| {
            scene_bind_group(
                device,
                &self.scene_layout,
                screen,
                &self.dummy_depth,
                self.scene_depth_sampler.as_ref(),
            )
        });
        Targets {
            samples: self.samples,
            ao,
            ssr,
            opaque_scene,
            taa,
            velocity,
            post: self.post.targets(device, size, upscaled, &hdr, &scene_view),
            color: target("3d color", HDR_FORMAT, self.samples, none),
            depth,
            hdr_texture,
            scene_texture,
            scene,
            depth_copy,
            scene_of_hdr,
            scene_view,
            hdr,
            size,
        }
    }
}

/// Scene-sized targets: the window's size times `Post::render_scale`.
pub(super) struct Targets {
    pub(super) samples: u32,
    pub(super) ao: Option<crate::render::screen::AoTargets>,
    pub(super) ssr: Option<crate::render::ssr::SsrTargets>,
    /// With GTAO or screen-space reflections: what the opaque pass reads
    /// them by (its scene bind group).
    pub(super) opaque_scene: Option<wgpu::BindGroup>,
    /// With temporal antialiasing: the lit frame's image, history and motion.
    pub(super) taa: Option<crate::render::taa::TaaTargets>,
    pub(super) velocity: Option<crate::render::taa::VelocityTargets>,
    /// The post chain's images (see `post.rs`).
    pub(super) post: PostTargets,
    /// Multisampled HDR color, resolved into `hdr`.
    pub(super) color: wgpu::TextureView,
    pub(super) hdr: wgpu::TextureView,
    pub(super) hdr_texture: wgpu::Texture,
    /// A copy of the opaque scene, for transparent shaders that read it.
    pub(super) scene_texture: wgpu::Texture,
    /// The scene bind group of the transparent pass: `scene_texture` and
    /// `depth` (or its copy, `depth_copy`).
    pub(super) scene: wgpu::BindGroup,
    /// On the GL backend: the copy of `depth` the transparent pass reads.
    pub(super) depth_copy: Option<super::depth_copy::DepthCopyTarget>,
    /// `scene_texture` as a render target: screen effects swap between it
    /// and `hdr` (ping-pong), each reading one and writing the other.
    pub(super) scene_view: wgpu::TextureView,
    /// The scene bind group reading `hdr` (and `depth`), for effects.
    pub(super) scene_of_hdr: wgpu::BindGroup,
    /// Multisampled depth, written by the prepass.
    pub(super) depth: wgpu::TextureView,
    pub(super) size: (u32, u32),
}

impl Targets {
    /// What the 3D passes resolve the lit frame into: `hdr`, or with
    /// temporal antialiasing its input image (the resolve then writes `hdr`).
    pub(super) fn lit_view(&self, taa: bool) -> &wgpu::TextureView {
        match &self.taa {
            Some(t) if taa => &t.input_view,
            _ => &self.hdr,
        }
    }

    pub(super) fn lit_texture(&self, taa: bool) -> &wgpu::Texture {
        match &self.taa {
            Some(t) if taa => &t.input,
            _ => &self.hdr_texture,
        }
    }
}

/// The shared parts of every frame bind group: the shadow map and the sky.
pub(super) struct FrameSources {
    pub(super) layout: wgpu::BindGroupLayout,
    pub(super) shadow_view: wgpu::TextureView,
    /// Point and spot lights' shadow views (a 1x1 stand-in until a light
    /// first casts shadows).
    pub(super) light_shadow_view: wgpu::TextureView,
    pub(super) shadow_sampler: wgpu::Sampler,
    pub(super) sky_view: wgpu::TextureView,
    pub(super) sky_sampler: wgpu::Sampler,
    /// Whether the layout has the decal images (see `supports_decals`).
    pub(super) decals: bool,
    /// The decal images: a stand-in until the game sets some.
    pub(super) decal_view: wgpu::TextureView,
    pub(super) decal_sampler: wgpu::Sampler,
    /// Whether the layout has global illumination's probes (see
    /// `supports_gi`), and their atlas: a stand-in while it's off.
    pub(super) gi: bool,
    pub(super) gi_view: wgpu::TextureView,
}

impl FrameSources {
    pub(super) fn bind_group(
        &self,
        device: &wgpu::Device,
        frame: &wgpu::Buffer,
        lights: &LightTextures,
    ) -> wgpu::BindGroup {
        self.bind_group_shadowed(device, frame, lights, &self.shadow_view)
    }

    /// [`bind_group`](Self::bind_group) with another sun shadow map (global
    /// illumination's probe views have their own).
    pub(super) fn bind_group_shadowed(
        &self,
        device: &wgpu::Device,
        frame: &wgpu::Buffer,
        lights: &LightTextures,
        shadow: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        let mut entries = vec![
            wgpu::BindGroupEntry {
                binding: 0,
                resource: frame.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(shadow),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: wgpu::BindingResource::Sampler(&self.shadow_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wgpu::BindingResource::TextureView(&self.sky_view),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: wgpu::BindingResource::Sampler(&self.sky_sampler),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: wgpu::BindingResource::TextureView(&lights.records_view),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(&lights.indices_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: wgpu::BindingResource::TextureView(&self.light_shadow_view),
            },
        ];
        if self.decals {
            entries.extend(decal_entries(&self.decal_view, &self.decal_sampler));
        }
        if self.gi {
            entries.push(wgpu::BindGroupEntry {
                binding: 10,
                resource: wgpu::BindingResource::TextureView(&self.gi_view),
            });
        }
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("3d frame"),
            layout: &self.layout,
            entries: &entries,
        })
    }
}

/// The decal images' bindings in a frame bind group.
pub(super) fn decal_entries<'a>(
    view: &'a wgpu::TextureView,
    sampler: &'a wgpu::Sampler,
) -> [wgpu::BindGroupEntry<'a>; 2] {
    [
        wgpu::BindGroupEntry {
            binding: 8,
            resource: wgpu::BindingResource::TextureView(view),
        },
        wgpu::BindGroupEntry {
            binding: 9,
            resource: wgpu::BindingResource::Sampler(sampler),
        },
    ]
}

/// A render target a target camera draws into: multisampled color and
/// depth of its size, resolved into the target's texture, and its own
/// frame uniform.
pub(super) struct ViewTarget {
    pub(super) color: wgpu::TextureView,
    pub(super) depth: wgpu::TextureView,
    pub(super) resolve: wgpu::TextureView,
    pub(super) frame_buffer: wgpu::Buffer,
    /// What `frame_buffer` holds (see `sync_frame`).
    pub(super) frame_mirror: Mirror,
    pub(super) frame_bind_group: wgpu::BindGroup,
    pub(super) lights: LightTextures,
}

/// A render target compute passes run on: the scratch texture each pass
/// writes (then copied back into the target) and its bind group.
pub(super) struct ComputeState {
    pub(super) target: wgpu::Texture,
    pub(super) scratch: wgpu::Texture,
    pub(super) group: wgpu::BindGroup,
}

/// The frame's bindings (group 0) as material shaders see them. The
/// shadow and sky-texture passes bind the frame at a dynamic offset (one
/// per cascade) and stand-ins for the shadow map and sky, which they draw
/// into: a material shader that reads them (from `surface`, which runs
/// for cut-outs' alpha) must build there too.
pub(super) fn frame_entries(
    stages: wgpu::ShaderStages,
    per_cascade: bool,
    decals: bool,
    gi: bool,
) -> Vec<wgpu::BindGroupLayoutEntry> {
    let mut entries = vec![
        wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: stages,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: per_cascade,
                min_binding_size: per_cascade
                    .then(|| wgpu::BufferSize::new(size_of::<FrameUniform>() as u64))
                    .flatten(),
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: stages,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 2,
            visibility: stages,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 3,
            visibility: stages,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 4,
            visibility: stages,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 5,
            visibility: stages,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        wgpu::BindGroupLayoutEntry {
            binding: 6,
            visibility: stages,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Uint,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
        // Not in compute passes, which would go over WebGPU's 16 sampled
        // textures per stage; they light nothing.
        wgpu::BindGroupLayoutEntry {
            binding: 7,
            visibility: stages - wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Depth,
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        },
    ];
    if decals {
        // Projected decals' images: the 17th sampled texture of the mesh
        // pipelines, so only where the device has it. Fragment only.
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 8,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2Array,
                multisampled: false,
            },
            count: None,
        });
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 9,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        });
    }
    if gi {
        // Global illumination's probes: the 18th sampled texture (see
        // `supports_gi`). Fragment only.
        entries.push(wgpu::BindGroupLayoutEntry {
            binding: 10,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        });
    }
    entries
}

/// What transparent shaders read of the scene behind them.
pub(in crate::render) fn scene_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    color: &wgpu::TextureView,
    depth: &wgpu::TextureView,
    depth_sampler: Option<&wgpu::Sampler>,
) -> wgpu::BindGroup {
    let mut entries = vec![
        wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(color),
        },
        wgpu::BindGroupEntry {
            binding: 1,
            resource: wgpu::BindingResource::TextureView(depth),
        },
    ];
    // The GL backend samples the depth (see `shaders::reads_depth_as_float`).
    if let Some(sampler) = depth_sampler {
        entries.push(wgpu::BindGroupEntry {
            binding: 2,
            resource: wgpu::BindingResource::Sampler(sampler),
        });
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("3d scene"),
        layout,
        entries: &entries,
    })
}
