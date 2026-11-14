//! The game's screen effects and target effects: full-screen passes that
//! run the game's WGSL over the frame.
//!
//! [`Effects`] owns their pipelines by effect id (from `create_effect`
//! until the id is reused by a reload), the render targets that target
//! effects write (made on first use), and this frame's two lists of the
//! effects that run. It records its own passes when the post chain asks
//! (`post.rs`); what those passes bind of the rest of the renderer comes in
//! an [`EffectPass`], so nothing here reaches into the renderer.

use super::shader_checks::{self, Swap, end_scope, rejected};
use super::{
    EFFECT_VARIANT, GpuMaterial, PassTimer, Renderer3D, Targets, build_effect, compose_effect, effect_pipeline,
    for_samples, passes,
};
use crate::three::{ShaderDiagnostic, ShaderKind};

/// Every effect's pipelines, and the effects this frame runs.
#[derive(Default)]
pub(super) struct Effects {
    /// Screen effect pipelines, by effect id.
    pub(super) pipelines: Vec<wgpu::RenderPipeline>,
    /// Each effect's compiled shader, for building its target pipeline.
    pub(super) modules: Vec<wgpu::ShaderModule>,
    /// Target effect pipelines, by effect id: built when first used.
    pub(super) target_pipelines: Vec<Option<wgpu::RenderPipeline>>,
    /// Render targets target effects write: (texture id, texture, its view),
    /// made on first use.
    pub(super) targets: Vec<(u32, wgpu::Texture, wgpu::TextureView)>,
    /// This frame's screen effects, in order: (effect, material).
    pub(super) frame: Vec<(u32, u32)>,
    /// This frame's target effects, in order: (effect, index in `targets`,
    /// material).
    pub(super) target_frame: Vec<(u32, usize, u32)>,
}

/// What an effect's pass binds of the rest of the frame.
pub(super) struct EffectPass<'a, 't> {
    /// The main view's frame bind group.
    pub(super) frame: &'a wgpu::BindGroup,
    pub(super) materials: &'a [GpuMaterial],
    /// Bound in the GPU buffer slot, which effects don't use.
    pub(super) storage_dummy: Option<&'a wgpu::BindGroup>,
    pub(super) targets: &'a Targets,
    pub(super) time: &'a PassTimer<'a, 't>,
}

impl Effects {
    /// Keeps this frame's target effects that can run: a known effect,
    /// material and render target, and not one `skipped` by the hazard
    /// check (by its index in `wanted`; see `custom_passes.rs`, which also
    /// leaves out a target effect whose material samples the target it
    /// writes). Builds their pipelines on first use.
    pub(super) fn prepare_targets(
        &mut self,
        device: &wgpu::Device,
        wanted: &[(u32, u32, u32)],
        materials: &[GpuMaterial],
        render_targets: &[(u32, wgpu::Texture)],
        layout: &wgpu::PipelineLayout,
        skipped: impl Fn(usize) -> bool,
    ) {
        self.target_frame.clear();
        for (i, &(effect, target, material)) in wanted.iter().enumerate() {
            if effect as usize >= self.pipelines.len() || material as usize >= materials.len() || skipped(i) {
                continue;
            }
            let index = match self.targets.iter().position(|&(t, _, _)| t == target) {
                Some(i) => i,
                None => {
                    let Some((_, texture)) = render_targets.iter().find(|&&(t, _)| t == target) else {
                        continue;
                    };
                    let texture = texture.clone();
                    let view = texture.create_view(&Default::default());
                    self.targets.push((target, texture, view));
                    self.targets.len() - 1
                }
            };
            if self.target_pipelines[effect as usize].is_none() {
                self.target_pipelines[effect as usize] = Some(effect_pipeline(
                    device,
                    layout,
                    "3d target effect",
                    &self.modules[effect as usize],
                    "vs_effect_target",
                    "fs_effect_target",
                ));
            }
            self.target_frame.push((effect, index, material));
        }
    }

    /// Records this frame's target effects, in order.
    pub(super) fn record_targets(&self, encoder: &mut wgpu::CommandEncoder, ctx: &EffectPass<'_, '_>) {
        // Target effects, in order: each reads the settled frame (in
        // `hdr`) and writes only its own render target, so the screen
        // effects below can sample this frame's results.
        for &(effect, index, material) in &self.target_frame {
            passes::mark("target effects");
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d target effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.targets[index].2,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: (ctx.time)("target effects"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(self.target_pipelines[effect as usize].as_ref().unwrap());
            pass.set_bind_group(0, ctx.frame, &[]);
            pass.set_bind_group(1, &ctx.materials[material as usize].bind_group, &[]);
            pass.set_bind_group(2, &ctx.targets.scene_of_hdr, &[]);
            if let Some(dummy) = ctx.storage_dummy {
                pass.set_bind_group(3, dummy, &[]);
            }
            pass.draw(0..3, 0..1);
        }
    }

    /// Records this frame's screen effects, in order. True when the frame
    /// ends up in `hdr`, false when in the scene texture (after an odd
    /// number of effects).
    pub(super) fn record_screen(&self, encoder: &mut wgpu::CommandEncoder, ctx: &EffectPass<'_, '_>) -> bool {
        // Screen effects, in order, swapping between `hdr` and the scene
        // texture (free once the transparents are drawn): each reads the
        // frame so far from one and writes the other, with no copies.
        let mut in_hdr = true;
        for &(effect, material) in &self.frame {
            let (source, target) = if in_hdr {
                (&ctx.targets.scene_of_hdr, &ctx.targets.scene_view)
            } else {
                (&ctx.targets.scene, &ctx.targets.hdr)
            };
            passes::mark(if in_hdr { "screen effect" } else { "screen effect back" });
            in_hdr = !in_hdr;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("3d screen effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: (ctx.time)("screen effects"),
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.pipelines[effect as usize]);
            pass.set_bind_group(0, ctx.frame, &[]);
            pass.set_bind_group(1, &ctx.materials[material as usize].bind_group, &[]);
            pass.set_bind_group(2, source, &[]);
            if let Some(dummy) = ctx.storage_dummy {
                pass.set_bind_group(3, dummy, &[]);
            }
            pass.draw(0..3, 0..1);
        }
        in_hdr
    }
}

impl Renderer3D {
    /// Compiles a screen effect from the game's WGSL; returns its id.
    pub fn create_effect(
        &mut self,
        device: &wgpu::Device,
        name: &str,
        user: &str,
    ) -> Result<u32, Box<ShaderDiagnostic>> {
        let start = crate::perf::Instant::now();
        let made = self.compile_effect(device, name, user);
        self.shaders.cache.counters.compiled(start.elapsed());
        made
    }

    fn compile_effect(&mut self, device: &wgpu::Device, name: &str, user: &str) -> Result<u32, Box<ShaderDiagnostic>> {
        let (module, pipeline, verdict) = self.checked_effect(device, name, user)?;
        let id = self.effects.pipelines.len() as u32;
        self.installed(ShaderKind::Effect, id);
        if let Some(verdict) = verdict {
            let swap = Swap::Effect {
                new: pipeline.clone(),
                old: None,
            };
            self.wait_for_verdict(id, name, EFFECT_VARIANT, swap, verdict);
        }
        self.effects.pipelines.push(pipeline);
        self.effects.modules.push(module);
        self.effects.target_pipelines.push(None);
        Ok(id)
    }

    /// Checks an effect's WGSL (with its place in the game's code) and
    /// builds its pipeline in a GPU error scope; the verdict comes with it
    /// if the GPU answers later.
    fn checked_effect(
        &self,
        device: &wgpu::Device,
        name: &str,
        user: &str,
    ) -> Result<(wgpu::ShaderModule, wgpu::RenderPipeline, Option<shader_checks::Verdict>), Box<ShaderDiagnostic>> {
        let source = compose_effect(user);
        crate::render::diagnose(name, ShaderKind::Effect, EFFECT_VARIANT, &source)?;
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let (module, pipeline) = build_effect(
            device,
            &self.shaders.layout,
            name,
            &for_samples(&source, self.samples, self.float_depth),
        );
        match end_scope(scope, self.defer_checks()) {
            Ok(None) => Ok((module, pipeline, None)),
            Ok(Some(error)) => Err(rejected(name, ShaderKind::Effect, EFFECT_VARIANT, &error)),
            Err(verdict) => Ok((module, pipeline, Some(verdict))),
        }
    }

    /// Recompiles effect `id` (hot reload). On an error the old one stays.
    pub fn replace_effect(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        name: &str,
        user: &str,
    ) -> Result<(), Box<ShaderDiagnostic>> {
        let start = crate::perf::Instant::now();
        let made = self.recompile_effect(device, id, name, user);
        self.shaders.cache.counters.compiled(start.elapsed());
        made
    }

    fn recompile_effect(
        &mut self,
        device: &wgpu::Device,
        id: u32,
        name: &str,
        user: &str,
    ) -> Result<(), Box<ShaderDiagnostic>> {
        let (module, new, verdict) = self.checked_effect(device, name, user)?;
        let i = id as usize;
        if i >= self.effects.pipelines.len() {
            return Ok(());
        }
        let old_pipeline = std::mem::replace(&mut self.effects.pipelines[i], new.clone());
        let old_module = std::mem::replace(&mut self.effects.modules[i], module);
        // Rebuilt from the new shader when next used.
        self.effects.target_pipelines[i] = None;
        self.installed(ShaderKind::Effect, id);
        if let Some(verdict) = verdict {
            let swap = Swap::Effect {
                new,
                old: Some((old_module, old_pipeline)),
            };
            self.wait_for_verdict(id, name, EFFECT_VARIANT, swap, verdict);
        }
        self.history.content_changed = true;
        Ok(())
    }
}
