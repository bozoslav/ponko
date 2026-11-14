//! Pass encoding: turning a prepared frame into GPU commands.
//!
//! `Renderer3D::draw` lists the frame's parts ([`Part`], in the order
//! `passes.rs` declares valid) and records each into a command buffer of
//! its own, on several cores. Recording takes `&self`: everything it reads
//! was decided by `prepare` (`prepare.rs`), and nothing here changes what a
//! later frame draws. The few counters it bumps are atomics.
//!
//! One part is one arm of `Renderer3D::record`. The model passes share
//! `draw_model_runs`, which looks each mesh part's pipeline up by shader,
//! variant and role ([`Kind`] says which pass is drawing). The post part's
//! passes are recorded by the post chain (`post.rs`).

use super::*;

// Native command recording uses rayon; browser GPU handles stay on one thread.
#[cfg(not(target_arch = "wasm32"))]
pub(super) type PassTimer<'a, 't> = dyn Fn(&'static str) -> Option<wgpu::RenderPassTimestampWrites<'t>> + Sync + 'a;
#[cfg(target_arch = "wasm32")]
pub(super) type PassTimer<'a, 't> = dyn Fn(&'static str) -> Option<wgpu::RenderPassTimestampWrites<'t>> + 'a;

/// The names a stage of compute passes records under: its steps in
/// `passes.rs` (for GPU buffers, for textures) and its GPU timer.
#[derive(Clone, Copy)]
pub(super) struct ComputeSteps {
    pub(super) buffer: &'static str,
    pub(super) texture: &'static str,
    pub(super) timer: &'static str,
}

/// `RenderPasses::compute`, in the prelude.
pub(super) const COMPUTE_STEPS: ComputeSteps = ComputeSteps {
    buffer: "buffer compute",
    texture: "texture compute",
    timer: "compute",
};

/// `RenderPasses::scene_compute`, in the post part.
pub(super) const SCENE_COMPUTE_STEPS: ComputeSteps = ComputeSteps {
    buffer: "scene buffer compute",
    texture: "scene texture compute",
    timer: "scene compute",
};

/// A part of a frame, recorded into a command buffer of its own (see
/// `Renderer3D::draw`).
#[derive(Clone, Copy, Debug)]
pub(super) enum Part {
    /// Compute passes and the sky texture.
    Prelude,
    /// A shadow cascade's runs `first..end`: big cascades are drawn in
    /// several passes (the first clears the map), recorded on several cores.
    Shadow {
        cascade: usize,
        first: u32,
        end: u32,
    },
    /// Every target camera.
    Views,
    /// Global illumination's probe views and their update.
    Gi,
    Prepass,
    /// The opaque pass, with the transparent surfaces when none reads the scene.
    Opaque,
    /// A copy of the opaque scene, then the transparent surfaces.
    Transparent,
    /// Screen effects, bloom and tone mapping.
    Post,
}

impl Part {
    /// Its pass in [`passes::FRAME_PASSES`]: what it reads and writes.
    pub(super) fn pass(self) -> usize {
        match self {
            Part::Prelude => passes::PRELUDE,
            Part::Shadow { .. } => passes::SHADOW,
            Part::Views => passes::VIEWS,
            Part::Gi => passes::GI,
            Part::Prepass => passes::PREPASS,
            Part::Opaque => passes::OPAQUE,
            Part::Transparent => passes::TRANSPARENT,
            Part::Post => passes::POST,
        }
    }

    pub(super) fn label(self) -> &'static str {
        passes::FRAME_PASSES[self.pass()].name
    }
}

/// What every part of a frame is recorded with.
pub(super) struct FrameParts<'a, 't> {
    pub(super) view: &'a wgpu::TextureView,
    pub(super) clear: wgpu::Color,
    pub(super) textures: &'a [GpuTexture],
    pub(super) targets: &'a Targets,
    pub(super) split: bool,
    pub(super) time: &'a PassTimer<'a, 't>,
    /// What the parts are checked against in debug builds (see `passes`).
    pub(super) checks: frame_checks::FrameChecks,
}

/// Draws per shadow pass before a cascade is split into more passes, each
/// recorded on its own core. A pass costs little GPU time on a depth-only
/// map; recording a few hundred draws costs a fraction of a millisecond.
pub(super) const SHADOW_DRAWS_PER_PASS: usize = 300;

/// GPU timer names of the shadow cascades' passes.
pub(super) const SHADOW_PASSES: [&str; 8] = [
    "shadows 0",
    "shadows 1",
    "shadows 2",
    "shadows 3",
    "shadows 4",
    "shadows 5",
    "shadows 6",
    "shadows 7",
];

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Shadow(usize),
    Prepass,
    /// Cut-out materials' prepass with alpha to coverage (multisampling).
    PrepassCoverage,
    Opaque,
    Transparent,
    /// Motion vectors of the models that moved (temporal antialiasing).
    Velocity,
    /// Motion vectors of the models a shader's `vertex` moves while their
    /// matrix holds still, from the frame's own instances (drawn first;
    /// `Velocity` draws the ones that also moved over them).
    VelocityStill,
    /// Global illumination's probe views: lit, solid surfaces writing
    /// their own depth (cut-outs went through `PrepassCoverage`).
    Capture,
}

impl Kind {
    pub(super) fn pass(self) -> Pass {
        match self {
            Kind::Shadow(_) => Pass::Shadow,
            Kind::Prepass => Pass::Prepass,
            Kind::PrepassCoverage => Pass::PrepassCoverage,
            Kind::Opaque => Pass::Opaque,
            Kind::Transparent => Pass::Transparent,
            Kind::Velocity | Kind::VelocityStill => Pass::Velocity,
            Kind::Capture => Pass::Capture,
        }
    }
}

/// Whether a still model drawn with `shader` moves on its own, for the
/// motion pass: its `vertex` reads the time, or a GPU buffer that keeps its
/// last frame (`kept`).
pub(super) fn still_moves(shader: &super::shaders::ShaderPipelines, kept: bool) -> bool {
    let past = shader.past();
    past.time || (past.items && kept)
}

impl Renderer3D {
    /// Where the lit frame's color pass draws, and what it resolves into:
    /// the multisampled color target resolving into the lit image, or with
    /// one sample the lit image itself.
    pub(super) fn frame_color<'a>(
        &self,
        targets: &'a Targets,
    ) -> (&'a wgpu::TextureView, Option<&'a wgpu::TextureView>) {
        let lit = targets.lit_view(self.screen.taa_on);
        if self.samples > 1 {
            (&targets.color, Some(lit))
        } else {
            (lit, None)
        }
    }

    /// Records the compute passes of `list` (see `FrameLists::computes`)
    /// in order, with `scene` as what their shaders see of the scene, under
    /// `steps`' names (buffer, texture) and timer.
    pub(super) fn record_computes(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        list: &[(u32, usize, u32)],
        scene: &wgpu::BindGroup,
        time: &PassTimer<'_, '_>,
        steps: ComputeSteps,
    ) {
        for &(compute, state, material) in list {
            if state & (1 << 31) != 0 {
                passes::mark(steps.buffer);
                let buffer = self.resources.storage_buffers[state & !(1 << 31)].as_ref().unwrap();
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("GPU buffer update"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(self.compute_pipelines[compute as usize].as_ref().unwrap());
                pass.set_bind_group(0, &self.frame_bind_group, &[]);
                pass.set_bind_group(1, &self.resources.materials[material as usize].bind_group, &[]);
                pass.set_bind_group(2, scene, &[]);
                pass.set_bind_group(3, &buffer.write, &[]);
                pass.dispatch_workgroups(buffer.len.div_ceil(64), 1, 1);
                continue;
            }
            let state = &self.render_targets.computes[state].1;
            passes::mark(steps.texture);
            {
                let timestamps = time(steps.timer).map(|t| wgpu::ComputePassTimestampWrites {
                    query_set: t.query_set,
                    beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                    end_of_pass_write_index: t.end_of_pass_write_index,
                });
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("3d compute"),
                    timestamp_writes: timestamps,
                });
                pass.set_pipeline(self.compute_pipelines[compute as usize].as_ref().unwrap());
                pass.set_bind_group(0, &self.frame_bind_group, &[]);
                pass.set_bind_group(1, &self.resources.materials[material as usize].bind_group, &[]);
                pass.set_bind_group(2, scene, &[]);
                pass.set_bind_group(3, &state.group, &[]);
                let size = state.target.size();
                pass.dispatch_workgroups(size.width.div_ceil(8), size.height.div_ceil(8), 1);
            }
            encoder.copy_texture_to_texture(
                state.scratch.as_image_copy(),
                state.target.as_image_copy(),
                state.target.size(),
            );
        }
    }

    /// Records the occlusion depth grid's reduction, timed as a compute pass.
    pub(super) fn record_occlusion(&self, encoder: &mut wgpu::CommandEncoder, frame: &FrameParts<'_, '_>) {
        let timestamps = (frame.time)("occlusion depth").map(|t| wgpu::ComputePassTimestampWrites {
            query_set: t.query_set,
            beginning_of_pass_write_index: t.beginning_of_pass_write_index,
            end_of_pass_write_index: t.end_of_pass_write_index,
        });
        self.occlusion.record(encoder, timestamps);
    }

    /// Whether solid surfaces go through the depth prepass too: on GPUs
    /// that need it, and for the passes that read the complete depth
    /// before the opaque pass (GTAO, screen-space reflections).
    pub(super) fn full_prepass(&self) -> bool {
        self.solid_prepass || self.screen.ao_enabled || self.screen.ssr_on || self.screen.velocity_on
    }

    /// Draws the frame's passes (see the module docs) and leaves the tone
    /// mapped result in `view`. `clear` is the background, in linear color.
    ///
    /// Each part of the frame (each shadow cascade, the depth prepass, the
    /// opaque and transparent passes, ...) is recorded into its own command
    /// buffer, all on different cores: turning draw calls into GPU commands
    /// (wgpu's `finish`) is most of a busy frame's CPU time. The buffers come
    /// back in the order they must be submitted.
    pub fn draw(
        &self,
        device: &wgpu::Device,
        view: &wgpu::TextureView,
        clear: wgpu::Color,
        textures: &[GpuTexture],
        timer: Option<&GpuTimer>,
    ) -> Vec<wgpu::CommandBuffer> {
        self.state_counters.take();
        let Some(targets) = self.targets.as_ref().filter(|_| !self.too_many) else {
            return Vec::new();
        };
        let time = |name| timer.and_then(|t| t.pass(name));
        let split = self.needs_scene();
        self.shadow_cache.recorded();
        let mut parts = Vec::with_capacity(8 + self.lists.cascades + self.lists.light_views);
        parts.push(Part::Prelude);
        for cascade in shadow_views(self.lists.cascades, self.lists.light_views) {
            if cascade < MAX_CASCADES && self.lists.redrawn & (1 << cascade) == 0 {
                continue; // kept from an earlier frame: not even cleared
            }
            if cascade >= MAX_CASCADES && self.shadow_cache.light_kept & (1 << (cascade - MAX_CASCADES)) != 0 {
                continue; // its tile already holds this
            }
            let runs = &self.lists.shadow_parts[cascade];
            let mut first = 0;
            let mut draws = 0;
            for i in 0..runs.len() {
                draws += 1;
                if draws >= SHADOW_DRAWS_PER_PASS {
                    parts.push(Part::Shadow {
                        cascade,
                        first,
                        end: i as u32 + 1,
                    });
                    (first, draws) = (i as u32 + 1, 0);
                }
            }
            if first == 0 || (first as usize) < runs.len() {
                parts.push(Part::Shadow {
                    cascade,
                    first,
                    end: runs.len() as u32,
                });
            }
        }
        if !self.lists.views.is_empty() || self.environment.has_probes() {
            parts.push(Part::Views);
        }
        if self.gi.as_ref().is_some_and(|g| g.update_count() > 0) {
            parts.push(Part::Gi);
        }
        parts.extend([Part::Prepass, Part::Opaque]);
        if split {
            parts.push(Part::Transparent);
        }
        parts.push(Part::Post);
        // The frame must keep what its passes declare (see `passes`):
        // checked in debug builds, so in every render test.
        let checks = self.check_frame(&parts, targets, view);
        let frame = FrameParts {
            view,
            clear,
            textures,
            targets,
            split,
            time: &time,
            checks,
        };
        let record = |part: &Part| {
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some(part.label()),
            });
            let outer = checks.begin(part.pass());
            self.record(*part, &mut encoder, &frame);
            checks.end(*part, outer);
            encoder.finish()
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            use rayon::prelude::*;
            parts.par_iter().map(record).collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            parts.iter().map(record).collect()
        }
    }

    /// Records one part of the frame (see [`draw`](Self::draw)).
    pub(super) fn record(&self, part: Part, encoder: &mut wgpu::CommandEncoder, frame: &FrameParts<'_, '_>) {
        let FrameParts {
            clear,
            textures,
            targets,
            split,
            time,
            checks,
            ..
        } = *frame;
        match part {
            Part::Prelude => {
                self.cull_static(encoder, time);
                // Skinned models first: every pass draws them posed.
                if self.skinning.has_work() {
                    passes::mark("skinning");
                    let timestamps = time("skinning").map(|t| wgpu::ComputePassTimestampWrites {
                        query_set: t.query_set,
                        beginning_of_pass_write_index: t.beginning_of_pass_write_index,
                        end_of_pass_write_index: t.end_of_pass_write_index,
                    });
                    self.skinning.record(encoder, timestamps);
                }
                // Compute passes first, so this frame's materials see their results.
                self.record_computes(encoder, &self.lists.computes, &self.no_scene, time, COMPUTE_STEPS);
                if self.atmosphere && self.environment.needs_sky() {
                    passes::mark("sky texture");
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("3d sky texture"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &self.sky_view,
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: time("sky texture"),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_pipeline(&self.sky_lut);
                    pass.set_bind_group(0, &self.shadows.bind_group, &[0]);
                    pass.draw(0..3, 0..1);
                }
                self.environment.sky(encoder, &self.sky_texture);
            }
            Part::Shadow { cascade, first, end } => {
                // Light views share the atlas: the first one clears all of
                // it, unless some keep their tiles; then each drawn view
                // clears its own.
                let whole = self.shadow_cache.light_kept == 0;
                let (view, clear) = if cascade < MAX_CASCADES {
                    (&self.shadows.layers[cascade], first == 0)
                } else if let Some(atlas) = &self.shadows.light_atlas {
                    (atlas, whole && cascade == MAX_CASCADES && first == 0)
                } else {
                    return;
                };
                passes::mark(if cascade < MAX_CASCADES {
                    "sun shadows"
                } else {
                    "light shadows"
                });
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("3d shadows"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view,
                        depth_ops: Some(wgpu::Operations {
                            load: if clear {
                                wgpu::LoadOp::Clear(1.0)
                            } else {
                                wgpu::LoadOp::Load
                            },
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: time(if cascade < MAX_CASCADES {
                        SHADOW_PASSES[cascade.min(SHADOW_PASSES.len() - 1)]
                    } else {
                        "light shadows"
                    }),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                if cascade >= MAX_CASCADES
                    && let Some(&[x, y, side]) = self.lists.light_tiles.get(cascade - MAX_CASCADES)
                {
                    pass.set_viewport(x as f32, y as f32, side as f32, side as f32, 0.0, 1.0);
                    pass.set_scissor_rect(x, y, side, side);
                    if !whole
                        && first == 0
                        && let Some(tile_clear) = &self.shadows.tile_clear
                    {
                        pass.set_pipeline(tile_clear);
                        pass.draw(0..3, 0..1);
                    }
                }
                let offset = (self.shadows.stride * cascade as u64) as u32;
                pass.set_bind_group(0, &self.shadows.bind_group, &[offset]);
                pass.set_bind_group(2, &self.no_scene, &[]);
                let runs = &self.lists.shadow_parts[cascade][first as usize..end as usize];
                self.record_model_parts(
                    &mut pass,
                    runs.iter().map(|p| (p.run, Some(p.part))),
                    Kind::Shadow(cascade),
                    &self.instances,
                );
                if first == 0 {
                    self.draw_models(&mut pass, &[], Kind::Shadow(cascade), &self.instances);
                }
            }
            Part::Views => {
                // Target cameras, before the main view, whose materials may read them.
                for (target, runs, batch_view) in &self.lists.views {
                    let (id, target) = &self.render_targets.views[*target];
                    // Probe captures start empty (alpha 0): the sky fills in behind them.
                    let capture = self.environment.is_capture(*id);
                    let clear = if capture { wgpu::Color::TRANSPARENT } else { clear };
                    self.draw_view(encoder, target, runs, *batch_view, clear, time);
                }
                self.environment.probes(encoder);
            }
            Part::Gi => self.record_gi(encoder, time),
            Part::Prepass => {
                passes::mark("depth");
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("3d depth prepass"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &targets.depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: time("depth prepass"),
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_bind_group(0, &self.frame_bind_group, &[]);
                pass.set_bind_group(2, &self.no_scene, &[]);
                self.draw_solid(&mut pass, Kind::Prepass);
                drop(pass);
                self.coverage_prepass(
                    encoder,
                    &targets.color,
                    &targets.depth,
                    &self.frame_bind_group,
                    &self.lists.solid_runs,
                    Some(0),
                );
                // With every solid surface in it, the depth is complete here.
                if self.solid_prepass {
                    self.record_occlusion(encoder, frame);
                }
                if self.screen.velocity_on
                    && let Some(taa) = &targets.velocity
                {
                    // Motion of the models that moved, over the final depth
                    // (read-only); everything else moves with the camera.
                    passes::mark("motion vectors");
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("3d motion vectors"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &taa.velocity_color,
                            depth_slice: None,
                            resolve_target: Some(taa.velocity()),
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Discard,
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &targets.depth,
                            depth_ops: None,
                            stencil_ops: None,
                        }),
                        timestamp_writes: time("motion vectors"),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_bind_group(0, &self.frame_bind_group, &[]);
                    pass.set_bind_group(2, &self.no_scene, &[]);
                    if self.screen.still_motion {
                        self.draw_solid(&mut pass, Kind::VelocityStill);
                    }
                    self.draw_models(&mut pass, &self.lists.moved_runs, Kind::Velocity, self.moved.buffer());
                }
                if self.screen.ao_enabled {
                    self.screen
                        .ao
                        .as_ref()
                        .unwrap()
                        .draw(encoder, targets.ao.as_ref().unwrap(), time);
                }
                if let (Some(ssr), Some(images)) = (&self.screen.ssr, &targets.ssr) {
                    ssr.trace(encoder, images, &time);
                }
            }
            Part::Opaque => {
                {
                    passes::mark("color");
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("3d opaque"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.frame_color(targets).0,
                            depth_slice: None,
                            resolve_target: self.frame_color(targets).1,
                            ops: wgpu::Operations {
                                // Alpha 0 marks the background for the post pass.
                                load: wgpu::LoadOp::Clear(wgpu::Color { a: 0.0, ..clear }),
                                // Without multisampling this is the lit frame itself.
                                store: checks.store(
                                    passes::OPAQUE,
                                    "color",
                                    frame_checks::Kept::Color,
                                    if split || self.samples <= 1 {
                                        wgpu::StoreOp::Store
                                    } else {
                                        wgpu::StoreOp::Discard
                                    },
                                ),
                            },
                        })],
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &targets.depth,
                            depth_ops: Some(wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                // Later passes read it (the motion pass,
                                // scene compute passes and screen effects
                                // too); without a solid
                                // prepass, it's only complete here.
                                store: checks.store(
                                    passes::OPAQUE,
                                    "color",
                                    frame_checks::Kept::Depth,
                                    if split
                                        || self.screen.taa_on
                                        || !self.effects.frame.is_empty()
                                        || !self.effects.target_frame.is_empty()
                                        || !self.lists.scene_computes.is_empty()
                                        || !self.full_prepass()
                                        // The occlusion grid is reduced from it below.
                                        || (!self.solid_prepass && self.occlusion.recording())
                                    {
                                        wgpu::StoreOp::Store
                                    } else {
                                        wgpu::StoreOp::Discard
                                    },
                                ),
                            }),
                            stencil_ops: None,
                        }),
                        timestamp_writes: time("opaque"),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    let opaque_scene = targets.opaque_scene.as_ref().unwrap_or(&self.no_scene);
                    pass.set_bind_group(0, &self.frame_bind_group, &[]);
                    pass.set_bind_group(2, opaque_scene, &[]);
                    self.draw_solid(&mut pass, Kind::Opaque);
                    if self.atmosphere {
                        pass.set_pipeline(&self.sky);
                        pass.set_bind_group(0, &self.frame_bind_group, &[]);
                        pass.draw(0..3, 0..1);
                        pass.set_bind_group(2, opaque_scene, &[]);
                    }
                    if !split {
                        self.draw_models(&mut pass, &self.lists.blended_runs, Kind::Transparent, &self.instances);
                        self.particles.draw(&mut pass, textures, self.resources.white);
                    }
                }
                if let (Some(ssr), Some(images)) = (&self.screen.ssr, &targets.ssr) {
                    ssr.keep(encoder, images);
                }
                if !self.solid_prepass {
                    self.record_occlusion(encoder, frame);
                }
            }
            Part::Transparent => {
                if split {
                    let lit = targets.lit_texture(self.screen.taa_on);
                    passes::mark("scene copy");
                    passes::mark("color");
                    encoder.copy_texture_to_texture(
                        lit.as_image_copy(),
                        targets.scene_texture.as_image_copy(),
                        lit.size(),
                    );
                    if let Some(copy) = &targets.depth_copy {
                        passes::mark("depth copy");
                        copy.record(encoder);
                    }
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("3d transparent"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.frame_color(targets).0,
                            depth_slice: None,
                            resolve_target: self.frame_color(targets).1,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Load,
                                store: checks.store(
                                    passes::TRANSPARENT,
                                    "color",
                                    frame_checks::Kept::Color,
                                    if self.samples <= 1 {
                                        wgpu::StoreOp::Store
                                    } else {
                                        wgpu::StoreOp::Discard
                                    },
                                ),
                            },
                        })],
                        // Read-only depth: the shaders sample it at the same time.
                        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                            view: &targets.depth,
                            depth_ops: None,
                            stencil_ops: None,
                        }),
                        timestamp_writes: time("transparent"),
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_bind_group(0, &self.frame_bind_group, &[]);
                    pass.set_bind_group(2, &targets.scene, &[]);
                    self.draw_models(&mut pass, &self.lists.blended_runs, Kind::Transparent, &self.instances);
                    self.particles.draw(&mut pass, textures, self.resources.white);
                }
            }
            Part::Post => self.record_post(encoder, frame),
        }
    }

    /// Global illumination's part of the frame (see `gi.rs`): the probe
    /// views' sun shadow map, the views (cut-outs' coverage first, then
    /// lit), and the probes' update from them.
    pub(super) fn record_gi<'t>(&self, encoder: &mut wgpu::CommandEncoder, time: &PassTimer<'_, 't>) {
        let Some(gi) = &self.gi else {
            return;
        };
        passes::mark("shadows");
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d gi shadows"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &gi.shadow,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: time("gi shadows"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            let offset = (self.shadows.stride * MAX_SHADOW_VIEWS as u64) as u32;
            pass.set_bind_group(0, &self.shadows.bind_group, &[offset]);
            pass.set_bind_group(2, &self.no_scene, &[]);
            // Models only: instance batches are culled for the main camera.
            self.draw_model_runs(&mut pass, &self.lists.gi_runs, Kind::Shadow(0), &self.instances);
        }
        // Each view in its own square of the views image.
        let views = |pass: &mut wgpu::RenderPass<'_>, kind| {
            let size = crate::render::gi::VIEW_SIZE;
            pass.set_bind_group(2, &self.no_scene, &[]);
            for (i, &(start, end)) in self.lists.gi_faces.iter().enumerate() {
                let (x, y) = ((i % 6) as u32 * size, (i / 6) as u32 * size);
                pass.set_viewport(x as f32, y as f32, size as f32, size as f32, 0.0, 1.0);
                pass.set_scissor_rect(x, y, size, size);
                pass.set_bind_group(0, &gi.face_groups[i], &[]);
                self.draw_model_runs(
                    pass,
                    &self.lists.gi_face_runs[start as usize..end as usize],
                    kind,
                    &self.instances,
                );
            }
        };
        let pass = |encoder: &mut wgpu::CommandEncoder, label, first: bool, timestamps, kind| {
            passes::mark(if first { "cut-out views" } else { "views" });
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &gi.views,
                    depth_slice: None,
                    resolve_target: (!first).then_some(&gi.resolve),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &gi.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: if first {
                            wgpu::LoadOp::Clear(1.0)
                        } else {
                            wgpu::LoadOp::Load
                        },
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: timestamps,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            views(&mut pass, kind);
        };
        pass(encoder, "3d gi cut-out views", true, None, Kind::PrepassCoverage);
        pass(encoder, "3d gi views", false, time("gi views"), Kind::Capture);
        let timestamps = time("gi update").map(|t| wgpu::ComputePassTimestampWrites {
            query_set: t.query_set,
            beginning_of_pass_write_index: t.beginning_of_pass_write_index,
            end_of_pass_write_index: t.end_of_pass_write_index,
        });
        gi.update(encoder, timestamps);
    }

    /// One target camera's view: depth prepass, then the lit solid models
    /// and the sky, resolved into its render target.
    pub(super) fn draw_view<'t>(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        target: &ViewTarget,
        runs: &[(u32, u32, u32)],
        batch_view: usize,
        clear: wgpu::Color,
        time: &PassTimer<'_, 't>,
    ) {
        passes::mark("prepass");
        passes::mark("color");
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d target camera prepass"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &target.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: time("target cameras"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_bind_group(0, &target.frame_bind_group, &[]);
            pass.set_bind_group(2, &self.no_scene, &[]);
            self.draw_models_view(&mut pass, runs, Kind::Prepass, &self.instances, batch_view);
        }
        self.coverage_prepass(
            encoder,
            &target.color,
            &target.depth,
            &target.frame_bind_group,
            runs,
            Some(batch_view),
        );
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d target camera"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: if self.samples > 1 {
                    &target.color
                } else {
                    &target.resolve
                },
                depth_slice: None,
                resolve_target: (self.samples > 1).then_some(&target.resolve),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(clear),
                    store: if self.samples > 1 {
                        wgpu::StoreOp::Discard
                    } else {
                        wgpu::StoreOp::Store
                    },
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Discard,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: time("target cameras"),
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, &target.frame_bind_group, &[]);
        pass.set_bind_group(2, &self.no_scene, &[]);
        self.draw_models_view(&mut pass, runs, Kind::Opaque, &self.instances, batch_view);
        if self.atmosphere {
            pass.set_pipeline(&self.sky);
            pass.set_bind_group(0, &target.frame_bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
    }

    /// Whether a transparent surface this frame reads the scene behind it.
    pub(super) fn needs_scene(&self) -> bool {
        self.lists
            .runs
            .iter()
            .map(|&(mesh, _, _)| mesh)
            .chain(
                self.static_batches
                    .iter()
                    .flatten()
                    .flat_map(|b| b.levels.iter().map(|l| l.mesh.slot())),
            )
            .any(|mesh| {
                self.resources.meshes[self.base(mesh) as usize]
                    .parts
                    .iter()
                    .enumerate()
                    .any(|(part, &(_, m))| {
                        let m = &self.resources.materials[self.part_material(mesh, part, m) as usize];
                        m.transparent && self.shaders.sets[m.shader as usize].reads_scene
                    })
            })
    }

    /// The mesh parts of `runs` that `kind` draws, each with its material
    /// and all its models at once.
    /// With multisampling, the cut-out materials' prepass, with alpha to
    /// coverage: a pass of its own, as coverage needs a color target (`color`,
    /// attached but not written; the opaque pass clears it).
    pub(super) fn coverage_prepass(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        color: &wgpu::TextureView,
        depth: &wgpu::TextureView,
        frame_bind_group: &wgpu::BindGroup,
        runs: &[(u32, u32, u32)],
        batches: Option<usize>,
    ) {
        if !self.has_cutouts(runs, batches.is_some()) {
            return;
        }
        passes::mark("cut-outs");
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("3d cut-out prepass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: color,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: depth,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_bind_group(0, frame_bind_group, &[]);
        pass.set_bind_group(2, &self.no_scene, &[]);
        if let Some(view) = batches {
            self.draw_models_view(&mut pass, runs, Kind::PrepassCoverage, &self.instances, view);
        } else {
            self.draw_model_runs(&mut pass, runs, Kind::PrepassCoverage, &self.instances);
        }
    }

    pub(super) fn draw_models(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        runs: &[(u32, u32, u32)],
        kind: Kind,
        instances: &wgpu::Buffer,
    ) {
        let view = match kind {
            Kind::Shadow(v) => v + 1,
            _ => 0,
        };
        self.draw_models_view(pass, runs, kind, instances, view);
    }

    pub(super) fn draw_models_view(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        runs: &[(u32, u32, u32)],
        kind: Kind,
        instances: &wgpu::Buffer,
        view: usize,
    ) {
        if runs.as_ptr() == self.lists.solid_runs.as_ptr() && runs.len() == self.lists.solid_runs.len() {
            self.record_model_parts(
                pass,
                self.lists.solid_parts.iter().map(|d| (d.run, Some(d.part))),
                kind,
                instances,
            );
        } else if let Some(i) = self
            .lists
            .views
            .iter()
            .position(|(_, r, _)| r.as_ptr() == runs.as_ptr() && r.len() == runs.len())
        {
            self.record_model_parts(
                pass,
                self.lists.view_parts[i].iter().map(|p| (p.run, Some(p.part))),
                kind,
                instances,
            );
        } else {
            self.draw_model_runs(pass, runs, kind, instances);
        }
        if kind != Kind::Velocity {
            self.draw_static(pass, kind, view);
        }
    }

    pub(super) fn draw_model_runs(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        runs: &[(u32, u32, u32)],
        kind: Kind,
        instances: &wgpu::Buffer,
    ) {
        self.record_model_parts(pass, runs.iter().copied().map(|r| (r, None)), kind, instances);
    }

    fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>, kind: Kind) {
        self.record_model_parts(
            pass,
            self.lists.solid_parts.iter().map(|d| (d.run, Some(d.part))),
            kind,
            &self.instances,
        );
        self.draw_static(pass, kind, 0);
    }

    fn record_model_parts(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        runs: impl Iterator<Item = (Run, Option<usize>)>,
        kind: Kind,
        instances: &wgpu::Buffer,
    ) {
        let mut n = StateChanges::default();
        // Pipeline lookups, added to the cache's counters once at the end.
        let mut lookups = 0;
        pass.set_vertex_buffer(1, instances.slice(..));
        n.vertex_buffers += 1;
        if let Some(dummy) = &self.resources.storage_dummy {
            pass.set_bind_group(3, dummy, &[]);
            n.bind_groups += 1;
        }
        // Whether group 3 holds the dummy and slot 1 the whole `instances`
        // (storage meshes and batches change them).
        let (mut dummy_bound, mut instances_bound) = (true, true);
        // Only what changes is set again: each `set_*` costs wgpu
        // validation and a Metal/Vulkan call per draw otherwise.
        let mut bound = None;
        let mut bound_material = u32::MAX;
        let mut bound_buffers = None;
        for ((mesh, start, models), only_part) in runs {
            let fading = mesh & FADED != 0;
            let mesh = mesh & !FADED;
            let run_mesh = mesh;
            let posed = (mesh as usize)
                .checked_sub(self.resources.meshes.len())
                .filter(|_| self.material_override(mesh).is_none());
            let mesh = &self.resources.meshes[self.base(mesh) as usize];
            let storage = if let Some((id, count)) = mesh.gpu_buffer {
                let Some(buffer) = self.resources.storage_buffers.get(id as usize).and_then(Option::as_ref) else {
                    continue;
                };
                Some((buffer, count.min(buffer.len)))
            } else {
                None
            };
            let Some((rest, indices)) = mesh.buffers(&self.resources.mesh_arena) else {
                continue;
            };
            let vertices = match posed {
                None => rest,
                Some(k) => match self.skinning.vertices(k) {
                    Some(v) => v,
                    None => continue,
                },
            };
            // The motion pass also reads where each corner was: a posed
            // model whose pose changed has its last pose posed apart.
            let previous = match (kind, posed) {
                (Kind::Velocity, Some(k)) => self.skinning.previous_vertices(k).unwrap_or(vertices),
                _ => vertices,
            };
            let mut buffers_set = false;
            let (first_part, parts) = match only_part {
                Some(part) => (part, &mesh.parts[part..part + 1]),
                None => (0, mesh.parts.as_slice()),
            };
            for (index, (range, material)) in parts.iter().enumerate() {
                let part = first_part + index;
                let material = &self.part_material(run_mesh, part, *material);
                let m = &self.resources.materials[*material as usize];
                // The role for the material's blend in this pass (see
                // `PipelineRole::for_pass`): transparent surfaces are drawn
                // last and cast no shadows.
                let Some(role) = PipelineRole::for_pass(
                    kind.pass(),
                    m.cutout,
                    m.transparent,
                    self.samples > 1,
                    self.full_prepass(),
                ) else {
                    continue;
                };
                let shader = &self.shaders.sets[m.shader as usize];
                if kind == Kind::VelocityStill && !still_moves(shader, storage.is_some_and(|(b, _)| b.keeps_past())) {
                    continue;
                }
                // Crossfading instances dither in every pass (their
                // pipelines are built the first time they're drawn, unless
                // `warm_pipelines` built them while loading).
                let variant = ShaderVariant::new(Sides::of(m.double_sided), Instancing::of(fading, storage.is_some()));
                lookups += 1;
                let Some(pipeline) = shader.variant(variant).for_kind(kind, role) else {
                    continue;
                };
                if bound != Some(pipeline as *const _) {
                    pass.set_pipeline(pipeline);
                    n.pipelines += 1;
                    bound = Some(pipeline as *const _);
                }
                if !buffers_set && bound_buffers != Some((vertices, indices, previous)) {
                    pass.set_vertex_buffer(0, vertices.slice(..));
                    pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
                    n.vertex_buffers += 1;
                    n.index_buffers += 1;
                    if kind == Kind::Velocity {
                        pass.set_vertex_buffer(2, previous.slice(..));
                        n.vertex_buffers += 1;
                    }
                    buffers_set = true;
                    bound_buffers = Some((vertices, indices, previous));
                }
                if bound_material != *material {
                    pass.set_bind_group(1, &m.bind_group, &[]);
                    n.bind_groups += 1;
                    bound_material = *material;
                }
                if let Some((b, count)) = storage {
                    // Each model gets its own draw: its one instance record
                    // (transform and tint) is shared by all `count` instances.
                    if count > 0 {
                        pass.set_bind_group(3, &b.read, &[]);
                        n.bind_groups += 1;
                        (dummy_bound, instances_bound) = (false, false);
                        let record = if kind == Kind::Velocity {
                            size_of::<MovedInstance>()
                        } else {
                            size_of::<Instance3D>()
                        } as u64;
                        for model in start..start + models {
                            pass.set_vertex_buffer(
                                1,
                                instances.slice(model as u64 * record..(model as u64 + 1) * record),
                            );
                            pass.draw_indexed(range.clone(), 0, 0..count);
                            n.vertex_buffers += 1;
                            n.draws += 1;
                        }
                    }
                } else {
                    if let Some(dummy) = self.resources.storage_dummy.as_ref().filter(|_| !dummy_bound) {
                        pass.set_bind_group(3, dummy, &[]);
                        n.bind_groups += 1;
                        dummy_bound = true;
                    }
                    if !instances_bound {
                        pass.set_vertex_buffer(1, instances.slice(..));
                        n.vertex_buffers += 1;
                        instances_bound = true;
                    }
                    pass.draw_indexed(range.clone(), 0, start..start + models);
                    n.draws += 1;
                }
            }
        }
        self.state_counters.add(&n);
        self.shaders.cache.counters.looked_up(lookups);
    }
}
