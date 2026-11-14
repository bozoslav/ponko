//! Pass preparation: everything a frame decides and uploads before any
//! pass is recorded.
//!
//! `Renderer3D::prepare` runs once per frame with `&mut self`; recording
//! (`record.rs`) then only reads. It is a fixed sequence of steps, each a
//! method here that writes one part of the renderer's per-frame state:
//! which effects are on, the temporal history decision, the draw lists,
//! the instance and uniform uploads, and the targets. A step that draws
//! nothing new for an effect stays out of the others' state: what one step
//! decides for a later one is passed along ([`FrameView`], the returned
//! values), not looked up.

use glam::{Mat4, Vec3};

use super::custom_passes::{Stage, Writer};
use super::*;

/// The frame's screen and camera, decided first and read by the later steps.
#[derive(Clone, Copy)]
struct FrameView {
    /// The size the scene is drawn at: the window's times the render scale.
    size: (u32, u32),
    /// Whether that differs from the window's (a scaling pass follows).
    upscaled: bool,
    /// The camera as the game set it, and as the passes draw with it:
    /// jittered by a sub-pixel amount with temporal antialiasing.
    unjittered: Mat4,
    view_proj: Mat4,
    jitter: [f32; 2],
    /// Whether ambient occlusion is on, and the contact shadows' length (0: off).
    ao_on: bool,
    contact: f32,
}

impl Renderer3D {
    /// Uploads this frame's instances, camera, light and post settings.
    ///
    /// The steps run in this order; each is a method below, and says what it
    /// decides for the later ones.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, input: &Prepare3D<'_>) {
        self.resources.mesh_arena.flush(device, queue);
        let &Prepare3D {
            scene,
            lut,
            size: window,
        } = input;
        let &Scene3D { frame, camera, .. } = scene;
        let view = self.prepare_view(device, queue, scene, lut, window);
        let gi_frame = frame.gi.as_deref().filter(|_| self.frame_sources.gi);
        self.prepare_gi(device, queue, gi_frame);
        let (rebuild, previous_view_proj) = self.begin_history(scene, &view);
        self.prepare_screen_passes(device, queue, scene, &view);
        let total = self.prepare_lists(device, queue, scene, gi_frame);
        if !self.upload_instances(device, queue, frame, gi_frame, total) {
            return;
        }
        self.prepare_shadows_and_lights(device, queue, frame, camera);
        self.upload_uniforms(device, queue, scene, &view, previous_view_proj, gi_frame);
        self.prepare_targets(device, &view, rebuild);
        self.advance_history(queue, scene, &view, previous_view_proj);
        self.prepare_occlusion(device, queue, scene, &view);
    }

    /// Switches between adapter-supported multisampled counts at a frame
    /// boundary. A pending asynchronous shader verdict must settle first,
    /// otherwise it could restore pipelines for the previous sample count.
    pub(crate) fn set_samples(&mut self, device: &wgpu::Device, samples: u32) -> bool {
        if samples == self.samples {
            return true;
        }
        if !matches!(samples, 2 | 4)
            || self.samples == 1
            || !self.shader_checks.is_empty()
            || (samples == 2
                && !device
                    .features()
                    .contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES))
        {
            return false;
        }
        self.shaders.set_samples(samples);
        self.sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("3d sky"),
            layout: Some(&self.sky_layout),
            vertex: wgpu::VertexState {
                module: &self.sky_module,
                entry_point: Some("vs_far"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::LessEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState {
                count: samples,
                ..Default::default()
            },
            fragment: Some(wgpu::FragmentState {
                module: &self.sky_module,
                entry_point: Some("fs_sky"),
                compilation_options: Default::default(),
                targets: &[Some(HDR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        self.particles = super::super::particles::Particles::new(device, &self.texture_layout, samples);
        if self.depth_copy.is_some() {
            self.depth_copy = Some(super::depth_copy::DepthCopy::new(device, samples));
        }
        self.gi = None; // Its capture attachments also use the scene's count.
        self.render_targets.views.clear();
        self.shadow_cache.invalidate();
        self.samples = samples;
        // Keep the old targets until prepare: targets_stale observes their
        // count and History::decide resets every temporal consumer together.
        true
    }

    /// Presentation stopped or its surface was recreated. The next drawn
    /// frame must have new targets even when its dimensions did not change.
    pub(crate) fn invalidate_targets(&mut self) {
        self.targets = None;
        // begin_history observes the missing targets and makes the single
        // reset decision before any temporal consumer prepares again.
    }

    /// Step 9: the scene-sized targets. Targets made for settings that have
    /// since changed (a different size, or an effect turned on or off) are
    /// replaced; the old ones are freed once the commands that used them
    /// have run.
    fn prepare_targets(&mut self, device: &wgpu::Device, view: &FrameView, rebuild: bool) {
        if rebuild {
            self.targets = Some(self.make_targets(device, view.size, view.upscaled));
        }
    }

    /// Step 11: what occlusion culling takes from this frame: the count of
    /// models it hid, and the depth grid the next frames test against.
    fn prepare_occlusion(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, scene: &Scene3D<'_>, view: &FrameView) {
        let &Scene3D {
            frame,
            camera,
            occlusion,
            ..
        } = scene;
        self.occluded = frame.occluded;
        if let Some(targets) = &self.targets {
            self.occlusion.prepare(
                device,
                queue,
                &targets.depth,
                view.size,
                self.samples,
                frame.view_proj,
                camera,
                occlusion,
            );
        }
    }

    /// Step 1: which effects this frame has (making their passes when first
    /// on), the post settings, and the camera every pass draws with.
    fn prepare_view(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene3D<'_>,
        lut: Option<(u32, u32)>,
        window: (u32, u32),
    ) -> FrameView {
        let &Scene3D {
            frame, lighting, post, ..
        } = scene;
        // The scene is drawn at `size` and scaled to the window's `window`.
        let size = scaled_size(window, post.render_scale, device.limits().max_texture_dimension_2d);
        let upscaled = size != window;
        self.post.prepare(
            queue,
            post,
            lut,
            (size, window),
            self.resources.white,
            self.screen.taa_supported,
        );
        self.screen.taa_on = post.anti_aliasing == crate::render::AntiAliasing::Taa && self.screen.taa_supported;
        if self.screen.taa_on && self.screen.taa.is_none() {
            self.screen.taa = Some(Taa::new(device));
        }
        // The camera the frame is drawn with: jittered by a sub-pixel amount
        // with TAA (depth, GTAO and particles all use it), and the one that
        // moves are measured with.
        let unjittered = frame.view_proj;
        let jitter = if self.screen.taa_on {
            taa::jitter(self.history.taa_frame)
        } else {
            [0.0; 2]
        };
        let view_proj = if self.screen.taa_on {
            taa::jittered(unjittered, jitter, size)
        } else {
            unjittered
        };
        self.lists.moved_runs.clear();
        self.particles.prepare(device, queue, &frame.particles, view_proj);
        let ao_on = post.ambient_occlusion.enabled
            && post.ambient_occlusion.radius.is_finite()
            && post.ambient_occlusion.intensity.is_finite()
            && post.ambient_occlusion.radius > 0.0
            && post.ambient_occlusion.intensity > 0.0;
        // Contact shadows block only the sun, and only while it's up.
        let contact = if post.contact_shadows.is_finite() && lighting.sun_intensity > 0.0 {
            post.contact_shadows.max(0.0)
        } else {
            0.0
        };
        // One full-screen pass makes both.
        self.screen.ao_enabled = (ao_on || contact > 0.0) && can_compute(device);
        if self.screen.ao_enabled && self.screen.ao.is_none() {
            self.screen.ao = Some(crate::render::screen::Ao::new(device));
        }
        self.screen.ssr_on = post.ssr.is_finite()
            && post.ssr > 0.0
            && post.ssr_max_distance.is_finite()
            && post.ssr_max_distance > 0.0
            && can_compute(device);
        if self.screen.ssr_on && self.screen.ssr.is_none() {
            self.screen.ssr = Some(crate::render::ssr::Ssr::new(device));
        }
        self.screen.velocity_on = self.screen.taa_on || (self.screen.ao_enabled && post.ao_temporal && ao_on);
        FrameView {
            size,
            upscaled,
            unjittered,
            view_proj,
            jitter,
            ao_on,
            contact,
        }
    }

    /// Step 2: global illumination's probes, made when first on and bound to
    /// every frame (again when their atlas is remade).
    fn prepare_gi(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, gi_frame: Option<&crate::three::gi::GiFrame>) {
        // Global illumination: the probes are made when first on, and their
        // atlas bound to every frame (again when it's remade).
        if let Some(g) = gi_frame {
            let gi = self.gi.get_or_insert_with(|| {
                crate::render::gi::Gi::new(device, self.samples, size_of::<FrameUniform>() as u64)
            });
            let mut remade = gi.prepare(device, queue, g) || gi.face_groups.is_empty();
            remade |= gi.lights.upload(device, queue, &g.capture.lights, &g.capture.decals);
            if remade {
                self.frame_sources.gi_view = gi.atlas_view.clone();
                self.rebind_frames(device);
            }
        } else if self.gi.take().is_some() {
            self.frame_sources.gi_view = self.no_gi.clone();
            self.rebind_frames(device);
        }
    }

    /// Step 3: whether the targets are made again and whether the temporal
    /// history is dropped. Returns the first, and the camera motion is
    /// measured from.
    fn begin_history(&mut self, scene: &Scene3D<'_>, view: &FrameView) -> (bool, Mat4) {
        let &Scene3D { frame, post, .. } = scene;
        let FrameView {
            size,
            upscaled,
            unjittered,
            ao_on,
            ..
        } = *view;
        // Temporal history: one decision for every effect that reuses the
        // last frame (see `HistoryReset`), made before any of them prepares.
        let rebuild = self.targets_stale(size, post, ao_on, upscaled);
        let reset = self.history.decide(frame.motion.reset, rebuild, unjittered);
        if reset.any() {
            self.forget_history();
        }
        // Camera motion is measured from the last frame's camera; after a
        // reset there is none (the last frame is no past of this one).
        let previous_view_proj = self.history.previous.unwrap_or(unjittered);
        (rebuild, previous_view_proj)
    }

    /// Drops every effect's memory of the frames before this one: TAA's
    /// history and the camera motion is measured from, temporal AO's
    /// history and SSR's last image. Each starts over from the next frame
    /// it draws, so a reset asked for on a frame that draws nothing holds.
    pub(super) fn forget_history(&mut self) {
        self.history.previous = None;
        if let Some(ao) = &mut self.screen.ao {
            ao.reset();
        }
        if let Some(ssr) = &mut self.screen.ssr {
            ssr.forget();
        }
    }

    /// Step 4: the screen-space passes' settings for this frame's camera.
    fn prepare_screen_passes(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene3D<'_>,
        view: &FrameView,
    ) {
        let &Scene3D {
            lighting, camera, post, ..
        } = scene;
        let FrameView {
            size,
            view_proj,
            ao_on,
            contact,
            ..
        } = *view;
        if let Some(ao) = &mut self.screen.ao {
            let sun = (-lighting.sun_direction).try_normalize().unwrap_or(Vec3::Y);
            ao.prepare(
                queue,
                view_proj,
                camera,
                ao_on.then_some(&post.ambient_occlusion),
                (sun, contact),
                (post.ao_half_resolution, post.ao_temporal && ao_on),
            );
        }
        let made_for = self.targets.as_ref().map(|t| t.size);
        self.post.prepare_smaa(device, queue, size, made_for);
    }

    /// The compute passes of `stage` that can run, from the frame's
    /// `wanted` (compute, target texture or GPU buffer with the top bit
    /// set, material), into `list` (compute, index into the render
    /// targets' `computes` or the GPU buffer with the top bit set,
    /// material). Freed passes, buffers and targets, and passes the hazard
    /// check skips, are left out.
    fn list_computes(
        &mut self,
        device: &wgpu::Device,
        wanted: &[(u32, u32, u32)],
        stage: Stage,
        list: &mut Vec<(u32, usize, u32)>,
    ) {
        list.clear();
        for (index, &(compute, target, material)) in wanted.iter().enumerate() {
            if material as usize >= self.resources.materials.len() || self.custom_passes.skips(stage, index) {
                continue;
            }
            if self.buffer_computes.get(compute as usize).copied() != Some(target & (1 << 31) != 0) {
                continue;
            }
            // A freed pass has no pipeline.
            if !self
                .compute_pipelines
                .get(compute as usize)
                .is_some_and(Option::is_some)
            {
                continue;
            }
            if target & (1 << 31) != 0 {
                let id = target & !(1 << 31);
                if self
                    .resources
                    .storage_buffers
                    .get(id as usize)
                    .is_some_and(Option::is_some)
                {
                    list.push((compute, target as usize, material));
                }
                continue;
            }
            if let Some(state) =
                self.render_targets
                    .compute_state(device, target, self.compute_layouts.as_ref().map(|l| &l.0))
            {
                list.push((compute, state, material));
            }
        }
    }

    /// Lists the frame's writers of render targets in frame order and
    /// checks them (see `custom_passes.rs`): `custom_passes.skips` then
    /// says which passes this frame leaves out.
    fn check_custom_passes(&mut self, frame: &Frame3D) {
        let writers = &mut self.custom_passes.writers;
        writers.clear();
        fn textures(stage: Stage, list: &[(u32, u32, u32)]) -> impl Iterator<Item = Writer> + '_ {
            list.iter()
                .enumerate()
                .filter(|(_, (_, target, _))| target & (1 << 31) == 0)
                .map(move |(index, &(shader, target, _))| Writer {
                    stage,
                    index,
                    shader,
                    target,
                })
        }
        writers.extend(textures(Stage::Compute, &frame.computes));
        writers.extend(frame.views.iter().enumerate().map(|(index, view)| Writer {
            stage: Stage::TargetCamera,
            index,
            shader: index as u32,
            target: view.target,
        }));
        writers.extend(textures(Stage::SceneCompute, &frame.scene_computes));
        writers.extend(textures(Stage::TargetEffect, &frame.target_effects));
        let (materials, targets) = (&self.resources.materials, &self.render_targets.textures);
        self.custom_passes.run(|pass| {
            // Only target effects render to their target (see `check`).
            let Some(&(_, _, material)) = frame.target_effects.get(pass.index) else {
                return false;
            };
            let target = targets.iter().find(|(t, _)| *t == pass.target);
            materials
                .get(material as usize)
                .zip(target)
                .is_some_and(|(m, (_, texture))| m.bindings.textures.contains(texture))
        });
    }

    /// Step 5: this frame's draw lists (runs of every view, effects, compute
    /// passes, material overrides, posed skins), sorted for the passes.
    /// Returns how many instances the frame uploads.
    fn prepare_lists(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene3D<'_>,
        gi_frame: Option<&crate::three::gi::GiFrame>,
    ) -> usize {
        let &Scene3D {
            frame,
            sky,
            camera,
            occlusion,
            ..
        } = scene;
        self.atmosphere = sky.atmosphere;
        self.lists.override_base = frame.override_base;
        self.lists.material_overrides.clear();
        let meshes = self.resources.meshes.len() as u32;
        self.lists
            .material_overrides
            .extend(frame.material_overrides.list.iter().map(|o| MaterialOverride {
                mesh: if o.mesh < meshes { o.mesh } else { 0 },
                ..*o
            }));
        self.lists.override_parts.clear();
        self.lists
            .override_parts
            .extend_from_slice(&frame.material_overrides.parts);
        // Skinned models carry their own: listed only when one has any.
        self.lists.skin_overrides.clear();
        let none = MaterialOverride::NONE;
        if frame.skins.draws.iter().any(|d| (d.material, d.parts) != none) {
            let offset = self.lists.override_parts.len() as u32;
            self.lists.override_parts.extend_from_slice(&frame.skins.part_materials);
            self.lists
                .skin_overrides
                .extend(frame.skins.draws.iter().map(|d| MaterialOverride {
                    mesh: d.mesh,
                    material: d.material,
                    parts: (d.parts.0 + offset, d.parts.1),
                }));
        }
        self.prepare_static(device, queue, frame, camera, occlusion);
        self.lists.runs.clear();
        self.lists.runs.extend_from_slice(&frame.runs);
        self.effects.frame.clear();
        let (effects, materials) = (self.effects.pipelines.len(), self.resources.materials.len());
        self.effects.frame.extend(
            frame
                .effects
                .iter()
                .filter(|&&(e, m)| (e as usize) < effects && (m as usize) < materials),
        );
        // The game's passes that would break a resource rule are left out
        // (see `custom_passes.rs`).
        self.check_custom_passes(frame);
        let custom = &self.custom_passes;
        self.effects.prepare_targets(
            device,
            &frame.target_effects,
            &self.resources.materials,
            &self.render_targets.textures,
            &self.shaders.layout,
            |i| custom.skips(Stage::TargetEffect, i),
        );
        self.lists.cascades = frame.cascades;
        self.lists.redrawn = frame.redrawn;
        self.lists.light_views = frame.light_views;
        self.lists.light_tiles.clear();
        self.lists.light_tiles.extend_from_slice(&frame.lights.shadow_tiles);
        for (mine, theirs) in self.lists.shadow_runs.iter_mut().zip(&frame.shadow_runs) {
            mine.clear();
            mine.extend_from_slice(theirs);
        }
        let mut computes = std::mem::take(&mut self.lists.computes);
        self.list_computes(device, &frame.computes, Stage::Compute, &mut computes);
        self.lists.computes = computes;
        let mut computes = std::mem::take(&mut self.lists.scene_computes);
        self.list_computes(device, &frame.scene_computes, Stage::SceneCompute, &mut computes);
        self.lists.scene_computes = computes;
        // Target cameras' instances follow the main view's in one buffer,
        // then those of global illumination's probe views.
        let total = frame.instances.len()
            + frame.views.iter().map(|v| v.frame.instances.len()).sum::<usize>()
            + gi_frame.map_or(0, |g| g.capture.instances.len());
        let mut views = std::mem::take(&mut self.lists.views);
        let mut used = 0;
        let mut offset = frame.instances.len() as u32;
        for (camera_index, view) in frame.views.iter().enumerate() {
            let Some(target) = self
                .render_targets
                .view_target(device, view.target, self.samples, &self.frame_sources)
            else {
                continue;
            };
            if used == views.len() {
                views.push((0, Vec::new(), 0));
            }
            let (index, runs, batch_view) = &mut views[used];
            *batch_view = 1 + MAX_CASCADES + frame.light_views + camera_index;
            *index = target;
            runs.clear();
            runs.extend(
                view.frame
                    .runs
                    .iter()
                    .map(|&(mesh, start, count)| (mesh, start + offset, count)),
            );
            offset += view.frame.instances.len() as u32;
            used += 1;
        }
        views.truncate(used);
        self.lists.views = views;
        self.lists.gi_runs.clear();
        self.lists.gi_face_runs.clear();
        self.lists.gi_faces.clear();
        if let Some(g) = gi_frame {
            let moved = |&(mesh, start, count): &Run| (mesh, start + offset, count);
            self.lists.gi_runs.extend(g.capture.runs.iter().map(moved));
            self.lists.gi_face_runs.extend(g.face_runs.iter().map(moved));
            self.lists.gi_faces.extend_from_slice(&g.faces);
        }
        // Skinned models: only those some pass draws are posed.
        let real = self.resources.meshes.len() as u32;
        let mut drawn = std::mem::take(&mut self.lists.skin_drawn);
        drawn.clear();
        drawn.resize(frame.skins.draws.len(), false);
        let runs = self
            .lists
            .runs
            .iter()
            .chain(shadow_views(self.lists.cascades, self.lists.light_views).flat_map(|v| &self.lists.shadow_runs[v]))
            .chain(self.lists.views.iter().flat_map(|(_, r, _)| r))
            .chain(&self.lists.gi_runs);
        for &(mesh, _, _) in runs {
            if let Some(d) = (mesh & !FADED)
                .checked_sub(real)
                .and_then(|k| drawn.get_mut(k as usize))
            {
                *d = true;
            }
        }
        let meshes = &self.resources.meshes;
        self.skinning.prepare(
            device,
            queue,
            &frame.skins,
            |id| {
                let m = meshes.get(id as usize)?;
                Some((&m.buffers.as_ref()?.0, m.skin.as_ref()?))
            },
            |k| drawn[k],
            // Poses that moved are posed as they were too, for the motion
            // pass (only when it draws something: not on a reset frame).
            self.screen.velocity_on && !frame.motion.runs.is_empty(),
        );
        self.lists.skin_drawn = drawn;
        self.order_runs();
        // Blended models are drawn from sorted copies of their instances,
        // which follow all the others.
        self.sort_blended(&frame.instances, camera, total as u32);
        self.order_solid_parts(&frame.instances, camera);
        total + self.lists.blended.len()
    }

    /// Step 6: the instances of every view, uploaded where they changed.
    /// False when the frame draws nothing (no instances, or more than the
    /// device's largest buffer holds): the later steps are skipped.
    fn upload_instances(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame3D,
        gi_frame: Option<&crate::three::gi::GiFrame>,
        total: usize,
    ) -> bool {
        if self.static_batches.iter().all(Option::is_none)
            && total == 0
            && frame.particles.instances.is_empty()
            && frame.particles.strips.runs.is_empty()
        {
            self.too_many = false;
            return false;
        }
        let record = size_of::<Instance3D>();
        let most = traffic::most_bytes(device, wgpu::BufferUsages::VERTEX) / record;
        if traffic::outgrown(&mut self.capacity, total, most) {
            self.instances = Self::instance_buffer(device, self.capacity);
            self.instance_mirror.forget();
        }
        let too_many = total > self.capacity;
        if too_many && !self.too_many {
            let then = "the 3D scene isn't drawn";
            traffic::too_big(
                "3d instances (models, every view's)",
                total * record,
                most * record,
                then,
            );
        }
        self.too_many = too_many;
        if too_many {
            return false;
        }
        // Only the pages that changed since the last frame are uploaded:
        // models that stand still (and keep their place in the lists) cost
        // nothing.
        let uploads = queue.uploading(Upload::Instances);
        let buffer = &self.instances;
        let mut upload = |at: usize, bytes: &[u8]| uploads.write_buffer(buffer, at as u64, bytes);
        // A frame uploads the instances that changed, with the few
        // unchanged ones between two of them, in at most `MAX_WRITES`
        // writes a list.
        let page = size_of::<Instance3D>();
        let mut at = 0;
        let views = frame.views.iter().map(|v| &v.frame.instances);
        for instances in std::iter::once(&frame.instances)
            .chain(views)
            .chain(gi_frame.map(|g| &g.capture.instances))
            .chain(std::iter::once(&self.lists.blended))
        {
            let bytes = bytemuck::cast_slice(instances);
            self.instance_mirror
                .sync_capped(at, bytes, (page, INSTANCE_BRIDGE), MAX_WRITES, &mut upload);
            at += bytes.len();
        }
        if self.screen.velocity_on && !frame.motion.runs.is_empty() {
            let moved = &frame.motion.instances;
            // Bound when drawn: a new buffer needs no rebinding.
            let moved = bytemuck::cast_slice(moved);
            self.moved.reserve(device, moved.len());
            // Past the most the device holds (reported), no motion is drawn.
            if moved.len() <= self.moved.fits() {
                self.moved.write(queue, 0, moved);
                self.lists.moved_runs.extend_from_slice(&frame.motion.runs);
            }
        }
        true
    }

    /// Step 7: the shadow maps for this frame's sizes, which shadow views
    /// are redrawn, the instance batches' buffers, and the lights and decals
    /// (with the frame bind groups made again where a binding changed).
    fn prepare_shadows_and_lights(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        frame: &Frame3D,
        camera: Vec3,
    ) {
        self.set_shadow_map(device, frame);
        self.set_light_shadow_map(device, frame);
        // Shadow views whose maps already hold what they'd draw are kept.
        self.plan_shadows(frame, camera);
        self.finish_static(device, queue);
        let rebind = self.bind_decal_array(frame.decal_array);
        if self.lights.upload(device, queue, &frame.lights, &frame.decals) || rebind {
            self.frame_bind_group = self.frame_sources.bind_group(device, &self.frame_buffer, &self.lights);
        }
        if rebind {
            for (_, target) in &mut self.render_targets.views {
                target.frame_bind_group = self
                    .frame_sources
                    .bind_group(device, &target.frame_buffer, &target.lights);
            }
            self.rebind_gi(device);
        }
    }

    /// Makes the sun's shadow map for this frame, if it isn't the one made
    /// already (see `ShadowMaps::set_sun_map`).
    fn set_shadow_map(&mut self, device: &wgpu::Device, frame: &Frame3D) {
        if self.shadows.set_sun_map(device, frame, &mut self.frame_sources) {
            self.shadow_cache.invalidate();
            self.rebind_frames(device);
        }
    }

    /// Makes, resizes or frees the point and spot lights' shadow atlas (see
    /// `ShadowMaps::set_light_map`).
    fn set_light_shadow_map(&mut self, device: &wgpu::Device, frame: &Frame3D) {
        if self.shadows.set_light_map(device, frame, &mut self.frame_sources) {
            self.shadow_cache.invalidate();
            self.rebind_frames(device);
        }
    }

    /// Step 8: the frame uniform of the main view, of every target camera,
    /// probe view and shadow view, and the environment's.
    fn upload_uniforms(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        scene: &Scene3D<'_>,
        view: &FrameView,
        previous_view_proj: Mat4,
        gi_frame: Option<&crate::three::gi::GiFrame>,
    ) {
        let &Scene3D {
            frame,
            lighting,
            sky,
            light,
            camera,
            time,
            ..
        } = scene;
        let FrameView {
            size,
            unjittered,
            view_proj,
            jitter,
            ..
        } = *view;
        let scaled = |c, k: f32| {
            let [r, g, b, _] = linear(c);
            [r * k, g * k, b * k, 0.0]
        };
        let ambient = |light: Vec3| (light * lighting.ambient_intensity).extend(0.0).to_array();
        let sun = (-lighting.sun_direction).try_normalize().unwrap_or(Vec3::Y);
        let mut light_view_proj = [Mat4::IDENTITY.to_cols_array_2d(); MAX_LIGHT_SHADOW_VIEWS];
        for (mine, theirs) in light_view_proj.iter_mut().zip(&frame.lights.shadow_views) {
            *mine = theirs.to_cols_array_2d();
        }
        let atlas = frame.lights.atlas.max(1) as f32;
        let mut light_rect = [[0.0; 4]; MAX_LIGHT_SHADOW_VIEWS];
        for (mine, &[x, y, side]) in light_rect.iter_mut().zip(&frame.lights.shadow_tiles) {
            *mine = [x as f32 / atlas, y as f32 / atlas, side as f32 / atlas, 0.0];
        }
        let mut uniform = FrameUniform {
            light_view_proj,
            light_rect,
            probes: frame.probes,
            // Light shadows move out along the normal by 1.5 texels too; w
            // is the sun shadow's filter (`Lighting::shadow_filter`).
            light_shadow: [
                frame.light_views as f32,
                1.0 / atlas,
                1.5,
                lighting.shadow_filter as u32 as f32,
            ],
            cluster_depth: cluster_depth(frame),
            cluster_view_proj: frame.view_proj.to_cols_array_2d(),
            view_proj: view_proj.to_cols_array_2d(),
            unjittered_view_proj: unjittered.to_cols_array_2d(),
            previous_view_proj: previous_view_proj.to_cols_array_2d(),
            jitter: [jitter[0], jitter[1], 0.0, 0.0],
            sun_view_proj: frame.sun_view_proj.map(|m| m.to_cols_array_2d()),
            caster_view_proj: Mat4::IDENTITY.to_cols_array_2d(),
            camera: camera.extend(1.0).to_array(),
            sun_dir: sun.extend(0.0).to_array(),
            sun_color: light.sun.extend(0.0).to_array(),
            sky: ambient(light.sky),
            ground: ambient(light.ground),
            // The shadow lookup moves out along the normal by 1.5 texels,
            // against acne on surfaces facing away from the sun.
            shadow: [
                frame.cascades as f32,
                1.0 / frame.shadow_size.max(1) as f32,
                1.5,
                lighting.shadow_distance,
            ],
            cascade_texel: frame.cascade_texel,
            time: [time.0, time.1, 0.0, 0.0],
            inv_view_proj: view_proj.inverse().to_cols_array_2d(),
            screen: [
                size.0 as f32,
                size.1 as f32,
                1.0 / size.0.max(1) as f32,
                1.0 / size.1.max(1) as f32,
            ],
            atmosphere: [
                if sky.atmosphere { 1.0 } else { 0.0 },
                sky.haze.max(0.0),
                (sky.sun_size.clamp(0.01, 20.0).to_radians() * 0.5).cos(),
                sky.stars.max(0.0),
            ],
            sky_light: {
                let [r, g, b, _] = scaled(lighting.sun_color, lighting.sun_intensity);
                [r, g, b, (camera.y / 1000.0).max(0.0)]
            },
            fog: [
                sky.fog_density.max(0.0),
                sky.fog_height,
                1.0 / sky.fog_falloff.max(1e-3),
                sky.light_shafts.clamp(0.0, 1.0),
            ],
            fog_color: scaled(sky.fog_color, 1.0),
            night: scaled(sky.night_light, 1.0),
            fog_far: [
                sky.fog_start.max(0.0),
                if sky.fog_end > sky.fog_start.max(0.0) {
                    sky.fog_end
                } else {
                    0.0
                },
                sky.fog_sun_glow.max(0.0),
                sky.fog_sun_focus.clamp(0.0, 0.99),
            ],
            gi: [[0.0; 4]; 3],
        };
        if let (Some(g), Some(gi)) = (gi_frame, &self.gi) {
            let (n, origin) = (g.grid.counts.as_vec3(), g.grid.origin.as_vec3());
            uniform.gi = [
                [0.0, 0.0, 0.0, g.grid.spacing],
                [n.x, n.y, n.z, g.intensity],
                [origin.x, origin.y, origin.z, gi.rows() as f32],
            ];
        }
        // A three-color ambient, without an atmosphere.
        let equator = match sky.equator_color {
            Some(c) if !sky.atmosphere => {
                let [r, g, b, _] = linear(c);
                [
                    r * lighting.ambient_intensity,
                    g * lighting.ambient_intensity,
                    b * lighting.ambient_intensity,
                    1.0,
                ]
            }
            _ => [0.0; 4],
        };
        self.environment.prepare(
            queue,
            [
                sun.x,
                sun.y,
                sun.z,
                sky.haze,
                camera.y,
                lighting.ambient_intensity,
                light.sky.x,
                light.sky.y,
                light.sky.z,
                light.ground.x,
                light.ground.y,
                light.ground.z,
                uniform.sky_light[0],
                uniform.sky_light[1],
                uniform.sky_light[2],
                uniform.night[0],
                uniform.night[1],
                uniform.night[2],
                equator[0],
                equator[1],
                equator[2],
                equator[3],
            ],
            sky.atmosphere,
            lighting.ambient_intensity,
            [uniform.sky, uniform.ground, equator],
        );
        self.environment.upload(device, queue, &frame.probe_uploads);
        self.environment.bakes.clear();
        self.environment.bakes.extend_from_slice(&frame.probe_bakes);
        self.environment.prepare_bakes(device, &self.render_targets.textures);
        sync_frame(queue, &self.frame_buffer, 0, &mut self.frame_mirror, &uniform);
        // Each target camera's frame: the same, seen from it.
        for view in &frame.views {
            let Some(target) = self.render_targets.views.iter().position(|&(id, _)| id == view.target) else {
                continue;
            };
            let target_lights = &mut self.render_targets.views[target].1;
            if target_lights
                .lights
                .upload(device, queue, &view.frame.lights, &view.frame.decals)
            {
                target_lights.frame_bind_group =
                    self.frame_sources
                        .bind_group(device, &target_lights.frame_buffer, &target_lights.lights);
            }
            let mut theirs = uniform;
            // Captures use sky IBL only, preventing recursive probe feedback.
            if frame.probe_bakes.iter().any(|(_, faces)| faces.contains(&view.target)) {
                theirs.probes.fill(Default::default());
            }
            theirs.cluster_depth = cluster_depth(&view.frame);
            theirs.cluster_view_proj = view.frame.view_proj.to_cols_array_2d();
            theirs.view_proj = view.view_proj.to_cols_array_2d();
            theirs.inv_view_proj = view.view_proj.inverse().to_cols_array_2d();
            // Target cameras are not resolved over time.
            theirs.unjittered_view_proj = theirs.view_proj;
            theirs.previous_view_proj = theirs.view_proj;
            theirs.jitter = [0.0; 4];
            theirs.camera = view.camera.extend(1.0).to_array();
            theirs.screen = [
                view.size.0 as f32,
                view.size.1 as f32,
                1.0 / view.size.0.max(1) as f32,
                1.0 / view.size.1.max(1) as f32,
            ];
            theirs.sky_light[3] = (view.camera.y / 1000.0).max(0.0);
            let target = &mut self.render_targets.views[target].1;
            sync_frame(queue, &target.frame_buffer, 0, &mut target.frame_mirror, &theirs);
        }
        // Global illumination's probe views: the same, from each probe, lit
        // by their own sun shadow map (one cascade around the grid, not
        // fading out), without point and spot light shadows.
        if let (Some(g), Some(gi)) = (gi_frame, &mut self.gi) {
            let mut theirs = uniform;
            theirs.cluster_depth = cluster_depth(&g.capture);
            theirs.cluster_view_proj = g.capture_view_proj.to_cols_array_2d();
            theirs.jitter = [0.0; 4];
            let size = crate::render::gi::VIEW_SIZE as f32;
            theirs.screen = [size, size, 1.0 / size, 1.0 / size];
            theirs.shadow = [1.0, 1.0 / GI_SHADOW_SIZE as f32, 1.5, f32::MAX];
            theirs.sun_view_proj[0] = g.sun_view_proj.to_cols_array_2d();
            theirs.cascade_texel[0] = g.sun_texel;
            theirs.light_shadow[0] = 0.0;
            for (i, view_proj) in g.face_view_proj.iter().enumerate() {
                let probe = g.updates[i / 6].position;
                theirs.view_proj = view_proj.to_cols_array_2d();
                theirs.inv_view_proj = view_proj.inverse().to_cols_array_2d();
                theirs.unjittered_view_proj = theirs.view_proj;
                theirs.previous_view_proj = theirs.view_proj;
                theirs.camera = probe.extend(1.0).to_array();
                theirs.sky_light[3] = (probe.y / 1000.0).max(0.0);
                sync_frame(queue, &gi.face_buffers[i], 0, &mut gi.face_mirrors[i], &theirs);
            }
        }
        // Copy 0 also serves the sky texture's pass. `camera.w` 0 tells
        // material shaders they're drawing a shadow map (`drawing_shadows`).
        uniform.camera[3] = 0.0;
        for i in 0..frame.cascades.max(1) {
            uniform.caster_view_proj = uniform.sun_view_proj[i.min(MAX_CASCADES - 1)];
            let mirror = &mut self.shadows.mirrors[i];
            sync_frame(
                queue,
                &self.shadows.frames,
                self.shadows.stride * i as u64,
                mirror,
                &uniform,
            );
        }
        for i in 0..frame.light_views {
            uniform.caster_view_proj = uniform.light_view_proj[i];
            let (at, mirror) = (MAX_CASCADES + i, &mut self.shadows.mirrors[MAX_CASCADES + i]);
            sync_frame(
                queue,
                &self.shadows.frames,
                self.shadows.stride * at as u64,
                mirror,
                &uniform,
            );
        }
        if let Some(g) = gi_frame {
            uniform.caster_view_proj = g.sun_view_proj.to_cols_array_2d();
            let mirror = &mut self.shadows.mirrors[MAX_SHADOW_VIEWS];
            sync_frame(
                queue,
                &self.shadows.frames,
                self.shadows.stride * MAX_SHADOW_VIEWS as u64,
                mirror,
                &uniform,
            );
        }
    }

    /// Step 10: the temporal effects' settings for this frame, and what the
    /// next frame measures motion and cuts from.
    fn advance_history(
        &mut self,
        queue: &wgpu::Queue,
        scene: &Scene3D<'_>,
        view: &FrameView,
        previous_view_proj: Mat4,
    ) {
        let &Scene3D { camera, post, .. } = scene;
        let FrameView {
            size,
            unjittered,
            view_proj,
            ..
        } = *view;
        // Temporal antialiasing: this frame's resolve, and what the next reads.
        if !self.screen.taa_on {
            self.history.previous = self.screen.velocity_on.then_some(unjittered);
        } else if self.has_draws() {
            // No camera of the last frame: the first frame with TAA, or
            // the history was just forgotten.
            let reset = self.history.previous.is_none();
            if let Some(taa) = &mut self.screen.taa {
                taa.prepare(
                    queue,
                    &taa::Resolve {
                        jittered: view_proj,
                        current: unjittered,
                        previous: previous_view_proj,
                        input_size: size,
                        output_size: size,
                        sharpness: post.taa_sharpness,
                        reset,
                    },
                );
            }
            self.history.resolved(unjittered);
        }
        // Screen-space reflections: the last frame's image is their colors,
        // unless they were off or the history was forgotten above.
        let drawn = self.has_draws();
        if let Some(ssr) = &mut self.screen.ssr {
            if !self.screen.ssr_on {
                ssr.forget();
            }
            if self.screen.ssr_on && drawn {
                ssr.prepare(queue, view_proj, camera, post.ssr.min(1.0), post.ssr_max_distance);
            }
        }
        // Cuts are measured from the last frame that left an image behind.
        if drawn {
            self.history.camera = Some(unjittered);
        }
    }
}
