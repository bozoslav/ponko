//! Ties the frame's description (see `passes`) to the renderer: reads a
//! frame's features off its state, and checks, in debug builds, that the
//! frame `draw` is about to record and what each part then records agree
//! with the description, and that the textures have the formats and sample
//! counts it declares. Release builds do none of it.

use super::passes::{self, Features, Format, Resource, Samples};
use super::{DEPTH_FORMAT, FADED, HDR_FORMAT, MAX_CASCADES, Part, Renderer3D, Run, Targets};

/// What a frame's parts are recorded against: the frame's features. Empty
/// in release builds.
#[derive(Clone, Copy)]
pub(super) struct FrameChecks {
    features: Features,
    /// The renderer the frame is drawn by (its address), so a test can ask
    /// what its passes did with their attachments.
    #[cfg(test)]
    renderer: usize,
}

/// An attachment of the lit frame whose contents a pass may discard.
#[derive(Clone, Copy)]
pub(super) enum Kept {
    /// The main camera's depth.
    Depth,
    /// What the frame's color passes draw into: the multisampled color, or
    /// with one sample the lit image itself.
    Color,
}

impl FrameChecks {
    /// Starts noting the steps a part of pass `pass` records on this thread.
    pub(super) fn begin(&self, pass: usize) -> (usize, u64) {
        if cfg!(debug_assertions) {
            passes::recording(pass)
        } else {
            (0, 0)
        }
    }

    /// Checks the steps noted since [`begin`](Self::begin) against the ones
    /// `part`'s pass declares for this frame.
    pub(super) fn end(&self, part: Part, outer: (usize, u64)) {
        if cfg!(debug_assertions) {
            let steps = passes::recorded(outer);
            let features = part_features(part, self.features);
            if let Err(error) = passes::check_recorded(&passes::FRAME_PASSES, part.pass(), features, steps) {
                panic!("invalid 3d frame: {error}");
            }
        }
    }

    /// `store`, as a pass's attachment is about to be given it: panics in
    /// debug builds if it discards what a later step of the frame reads.
    pub(super) fn store(&self, pass: usize, step: &'static str, kept: Kept, store: wgpu::StoreOp) -> wgpu::StoreOp {
        #[cfg(test)]
        LAST_STORES
            .lock()
            .unwrap()
            .insert((self.renderer, pass, matches!(kept, Kept::Depth)), store);
        if cfg!(debug_assertions) && store == wgpu::StoreOp::Discard {
            let resource = match kept {
                Kept::Depth => Resource::SceneDepth,
                Kept::Color if self.features.contains(Features::MSAA) => Resource::SceneColor,
                Kept::Color if self.features.contains(Features::TAA) => Resource::TaaInput,
                Kept::Color => Resource::Hdr,
            };
            if let Err(error) = passes::check_discard(&passes::FRAME_PASSES, pass, step, resource, self.features) {
                panic!("invalid 3d frame: {error}");
            }
        }
        store
    }
}

/// The features one part is recorded under: a shadow part draws either a
/// sun cascade or a light view.
fn part_features(part: Part, features: Features) -> Features {
    match part {
        Part::Shadow { cascade, .. } if cascade < MAX_CASCADES => features.without(Features::LIGHT_SHADOWS),
        Part::Shadow { .. } => features.without(Features::SUN_SHADOWS),
        _ => features,
    }
}

/// What a texture is declared as, in wgpu's terms: its format (`None`: not
/// checked) and sample count.
fn declared(resource: Resource, output: wgpu::TextureFormat, samples: u32) -> (Option<wgpu::TextureFormat>, u32) {
    let desc = resource.desc();
    let format = match desc.format {
        Format::Depth => Some(DEPTH_FORMAT),
        Format::Hdr => Some(HDR_FORMAT),
        Format::Velocity => Some(crate::render::taa::VELOCITY_FORMAT),
        Format::Occlusion => Some(wgpu::TextureFormat::Rg8Unorm),
        Format::Screen => Some(crate::render::ssr::SCREEN_FORMAT),
        Format::Output => Some(output),
        Format::Buffer | Format::Any => None,
    };
    let samples = match desc.samples {
        Samples::One => 1,
        Samples::Scene => samples,
    };
    (format, samples)
}

/// Checks a texture's format and sample count against what `resource` is
/// declared as (see [`declared`]).
fn check_texture(
    resource: Resource,
    format: wgpu::TextureFormat,
    sample_count: u32,
    declared: (Option<wgpu::TextureFormat>, u32),
) -> Result<(), String> {
    if declared.0.is_some_and(|d| d != format) {
        return Err(format!(
            "{} is a {format:?} texture, declared as {:?}",
            resource.name(),
            declared.0.unwrap()
        ));
    }
    if sample_count != declared.1 {
        return Err(format!(
            "{} has {sample_count} sample(s) per pixel, declared with {}",
            resource.name(),
            declared.1
        ));
    }
    Ok(())
}

impl Renderer3D {
    /// Whether a cut-out prepass over `runs` (and, with `batches`, the
    /// static batches) has anything to draw. `coverage_prepass` records one
    /// when it does, and the frame's features say so.
    pub(super) fn has_cutouts(&self, runs: &[Run], batches: bool) -> bool {
        if self.samples <= 1 {
            return false;
        }
        runs.iter().any(|&(mesh, _, _)| {
            self.resources.meshes[self.base(mesh & !FADED) as usize]
                .parts
                .iter()
                .enumerate()
                .any(|(part, &(_, m))| self.resources.materials[self.part_material(mesh, part, m) as usize].cutout)
        }) || (batches
            && self.static_batches.iter().flatten().any(|b| {
                b.levels.iter().any(|l| {
                    self.resources.meshes[l.mesh.slot() as usize]
                        .parts
                        .iter()
                        .any(|(_, m)| self.resources.materials[*m as usize].cutout)
                })
            }))
    }

    /// What the frame made of `parts` does (see [`Features`]).
    fn frame_features(&self, parts: &[Part], targets: &Targets) -> Features {
        let has = |f: fn(&Part) -> bool| parts.iter().any(f);
        let sun = has(|p| matches!(p, Part::Shadow { cascade, .. } if *cascade < MAX_CASCADES));
        let lights = has(|p| matches!(p, Part::Shadow { cascade, .. } if *cascade >= MAX_CASCADES));
        let buffer = |&(_, state, _): &(u32, usize, u32)| state & (1 << 31) != 0;
        let effects = self.effects.frame.len();
        let cameras = !self.lists.views.is_empty();
        // What `Environment::probes` records: a bake with a face that
        // isn't a render target is left out, though its part is in the frame.
        let probes = self.environment.records_probes();
        let gi = has(|p| matches!(p, Part::Gi)) && self.gi.is_some();
        let sky = self.environment.needs_sky();
        [
            Features::MSAA.only_if(self.samples > 1),
            Features::STATIC_CULL.only_if(self.static_culling.is_some()),
            Features::SKINNING.only_if(self.skinning.has_work()),
            Features::BUFFER_COMPUTE.only_if(self.lists.computes.iter().any(buffer)),
            Features::TEXTURE_COMPUTE.only_if(self.lists.computes.iter().any(|c| !buffer(c))),
            Features::SCENE_BUFFER_COMPUTE.only_if(self.lists.scene_computes.iter().any(buffer)),
            Features::SCENE_TEXTURE_COMPUTE.only_if(self.lists.scene_computes.iter().any(|c| !buffer(c))),
            Features::SKY_TEXTURE.only_if(self.atmosphere && sky),
            Features::SKY_BRDF.only_if(self.environment.needs_lookup()),
            Features::SKY_MAPS.only_if(sky),
            Features::SHADOWS.only_if(sun || lights),
            Features::SUN_SHADOWS.only_if(sun),
            Features::LIGHT_SHADOWS.only_if(lights && self.shadows.light_atlas.is_some()),
            Features::VIEWS.only_if(has(|p| matches!(p, Part::Views))),
            Features::TARGET_CAMERAS.only_if(cameras),
            Features::TARGET_CUTOUTS.only_if(self.lists.views.iter().any(|(_, runs, _)| self.has_cutouts(runs, true))),
            Features::PROBES.only_if(probes),
            Features::GI.only_if(gi),
            Features::GI_UPDATE.only_if(gi && self.gi.as_ref().is_some_and(|g| g.updates_probes())),
            Features::AO_TEMPORAL.only_if(self.screen.ao_enabled && targets.ao.as_ref().is_some_and(|a| a.temporal)),
            Features::SSR_LEVELS
                .only_if(self.screen.ssr.is_some() && targets.ssr.as_ref().is_some_and(|s| s.has_levels())),
            Features::CUTOUTS.only_if(self.has_cutouts(&self.lists.solid_runs, true)),
            Features::SOLID_PREPASS.only_if(self.solid_prepass),
            Features::OCCLUSION.only_if(self.occlusion.recording()),
            Features::VELOCITY.only_if(self.screen.velocity_on && targets.velocity.is_some()),
            Features::AO.only_if(self.screen.ao_enabled),
            Features::SSR.only_if(self.screen.ssr.is_some() && targets.ssr.is_some()),
            Features::SPLIT.only_if(has(|p| matches!(p, Part::Transparent))),
            Features::DEPTH_COPY.only_if(targets.depth_copy.is_some()),
            Features::TAA.only_if(self.screen.taa_on && self.screen.taa.is_some() && targets.taa.is_some()),
            Features::TARGET_EFFECTS.only_if(!self.effects.target_frame.is_empty()),
            Features::EFFECTS.only_if(effects >= 1),
            Features::EFFECTS_BACK.only_if(effects >= 2),
            Features::EFFECTS_ODD.only_if(effects % 2 == 1),
            // Bloom, FXAA, SMAA and the scaling pass.
            self.post.features(&targets.post),
        ]
        .into_iter()
        .fold(Features::NONE, Features::with)
    }

    /// The textures behind the frame's resources, where the renderer holds
    /// them, with the resource each is.
    fn textures<'a>(
        &'a self,
        targets: &'a Targets,
        output: &'a wgpu::TextureView,
        features: Features,
    ) -> impl Iterator<Item = (Resource, &'a wgpu::Texture)> {
        let view = |resource, view: &'a wgpu::TextureView| (resource, view.texture());
        let frame = [
            Some(view(Resource::SceneDepth, &targets.depth)),
            targets
                .depth_copy
                .as_ref()
                .map(|c| view(Resource::SceneDepthCopy, &c.view)),
            Some(view(Resource::SceneColor, &targets.color)),
            Some((Resource::Hdr, &targets.hdr_texture)),
            // One texture, in two roles (see `Resource::EffectScratch`).
            Some((Resource::SceneCopy, &targets.scene_texture)),
            Some(view(Resource::EffectScratch, &targets.scene_view)),
            Some(view(Resource::BloomMips, targets.post.bloom_image())),
            Some((Resource::SkyTexture, &self.sky_texture)),
            targets.taa.as_ref().map(|t| (Resource::TaaInput, &t.input)),
            targets
                .velocity
                .as_ref()
                .map(|v| view(Resource::VelocityColor, &v.velocity_color)),
            targets
                .velocity
                .as_ref()
                .map(|v| view(Resource::Velocity, v.velocity())),
            // With reflections, ambient occlusion is a level of their image.
            targets
                .ao
                .as_ref()
                .filter(|_| !features.contains(Features::SSR))
                .map(|a| view(Resource::AmbientOcclusion, &a.output)),
            targets.ssr.as_ref().map(|s| view(Resource::ScreenImage, &s.screen)),
            targets
                .ssr
                .as_ref()
                .map(|s| view(Resource::ReflectionHistory, s.images().0)),
            targets
                .ssr
                .as_ref()
                .and_then(|s| s.images().1)
                .map(|levels| (Resource::ReflectionLevels, levels)),
            targets.ao.as_ref().map(|a| view(Resource::AoRaw, a.images().0)),
            // Without temporal accumulation the history is a stand-in.
            targets
                .ao
                .as_ref()
                .and_then(|a| a.images().1)
                .map(|history| view(Resource::AoHistory, history)),
            targets.taa.as_ref().map(|t| view(Resource::TaaHistory, t.history())),
            targets.post.ldr().map(|l| view(Resource::Ldr, l)),
            targets.post.upscaled().map(|u| view(Resource::Upscaled, u)),
            self.shadows.layers.first().map(|l| view(Resource::SunShadowMap, l)),
            self.shadows
                .light_atlas
                .as_ref()
                .map(|a| view(Resource::LightShadowAtlas, a)),
            Some(view(Resource::Output, output)),
        ];
        let gi = self.gi.iter().flat_map(move |gi| {
            [
                view(Resource::GiShadowMap, &gi.shadow),
                view(Resource::GiViewDepth, &gi.depth),
                view(Resource::GiViewColor, &gi.views),
                view(Resource::GiViewImage, &gi.resolve),
                view(Resource::GiProbes, &gi.atlas_view),
            ]
        });
        let cameras = self.lists.views.iter().flat_map(move |(target, _, _)| {
            let target = &self.render_targets.views[*target].1;
            [
                view(Resource::TargetDepth, &target.depth),
                view(Resource::TargetColor, &target.color),
                view(Resource::TargetImage, &target.resolve),
            ]
        });
        let environment = self
            .environment
            .images()
            .map(|texture| (Resource::Environment, texture));
        frame.into_iter().flatten().chain(gi).chain(cameras).chain(environment)
    }

    /// Checks the frame `draw` is about to record (its `parts`, into
    /// `output`) against the frame's description, and returns what its
    /// parts are then checked with. Debug builds only: every offscreen
    /// render test checks the frames it draws. Panics on a contradiction.
    pub(super) fn check_frame(&self, parts: &[Part], targets: &Targets, output: &wgpu::TextureView) -> FrameChecks {
        if !cfg!(debug_assertions) {
            return FrameChecks {
                features: Features::NONE,
                #[cfg(test)]
                renderer: 0,
            };
        }
        let features = self.frame_features(parts, targets);
        // The order must keep what each pass declares it reads and writes,
        // in the frames it declares it for.
        if let Err(error) = passes::check_frame(&passes::FRAME_PASSES, parts.iter().map(|p| p.pass()), features) {
            panic!("invalid 3d frame order: {error}");
        }
        // The window's texture may be viewed in another format than its own.
        let output_format = self.post.output_format();
        for (resource, texture) in self.textures(targets, output, features) {
            let mut declared = declared(resource, output_format, self.samples);
            if resource == Resource::Output {
                declared.0 = None;
            }
            if let Err(error) = check_texture(resource, texture.format(), texture.sample_count(), declared) {
                panic!("invalid 3d frame: {error}");
            }
        }
        // Resources that share memory really are one texture.
        debug_assert!(targets.scene_view.texture() == &targets.scene_texture);
        #[cfg(test)]
        LAST_FEATURES.with(|last| last.set(Some(features)));
        FrameChecks {
            features,
            #[cfg(test)]
            renderer: std::ptr::from_ref(self) as usize,
        }
    }
}

#[cfg(test)]
thread_local! {
    static LAST_FEATURES: std::cell::Cell<Option<Features>> = const { std::cell::Cell::new(None) };
}

/// What each renderer's passes last did with an attachment they may
/// discard: by (renderer, pass, whether it is the depth). Parts are recorded
/// on other threads, so this isn't per thread.
#[cfg(test)]
static LAST_STORES: std::sync::Mutex<std::collections::BTreeMap<(usize, usize, bool), wgpu::StoreOp>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// What `renderer`'s last frame did with the scene depth at the end of its
/// opaque pass (for tests).
#[cfg(test)]
pub(crate) fn depth_after_opaque(renderer: &Renderer3D) -> Option<wgpu::StoreOp> {
    let key = (std::ptr::from_ref(renderer) as usize, passes::OPAQUE, true);
    LAST_STORES.lock().unwrap().get(&key).copied()
}

/// The features of the last frame this thread drew (for tests).
#[cfg(test)]
pub(crate) fn last_features() -> Option<Features> {
    LAST_FEATURES.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_are_checked_against_their_declared_format_and_samples() {
        let output = wgpu::TextureFormat::Bgra8UnormSrgb;
        let depth = declared(Resource::SceneDepth, output, 4);
        assert_eq!(depth, (Some(DEPTH_FORMAT), 4));
        check_texture(Resource::SceneDepth, DEPTH_FORMAT, 4, depth).unwrap();
        assert_eq!(
            check_texture(Resource::SceneDepth, DEPTH_FORMAT, 1, depth).unwrap_err(),
            "scene depth has 1 sample(s) per pixel, declared with 4"
        );
        let hdr = declared(Resource::Hdr, output, 4);
        assert_eq!(hdr, (Some(HDR_FORMAT), 1));
        assert_eq!(
            check_texture(Resource::Hdr, wgpu::TextureFormat::Rgba8Unorm, 1, hdr).unwrap_err(),
            "lit frame is a Rgba8Unorm texture, declared as Rgba16Float"
        );
        // The frame before FXAA is in the output's format; target cameras
        // draw HDR; a compute pass's target has whatever format the game
        // gave it.
        assert_eq!(declared(Resource::Ldr, output, 4), (Some(output), 1));
        assert_eq!(declared(Resource::TargetColor, output, 4), (Some(HDR_FORMAT), 4));
        assert_eq!(declared(Resource::ComputeTargets, output, 4), (None, 1));
        // Without multisampling, the frame's sample count is one everywhere.
        assert_eq!(declared(Resource::SceneColor, output, 1).1, 1);
    }

    #[test]
    fn a_shadow_part_is_a_sun_cascade_or_a_light_view() {
        let both = Features::SHADOWS
            .with(Features::SUN_SHADOWS)
            .with(Features::LIGHT_SHADOWS);
        let shadow = |cascade| Part::Shadow {
            cascade,
            first: 0,
            end: 0,
        };
        assert!(!part_features(shadow(0), both).contains(Features::LIGHT_SHADOWS));
        assert!(!part_features(shadow(MAX_CASCADES), both).contains(Features::SUN_SHADOWS));
        assert_eq!(part_features(Part::Opaque, both), both);
    }
}

/// Offscreen frames, checked against the frame's description as they are
/// drawn (`draw` does, in debug builds).
#[cfg(test)]
mod render_tests {
    use super::*;
    use crate::d3::Transform as Transform3D;
    use crate::prelude::*;
    use crate::render::buffer_render_test::Rig;
    use crate::render::{MaterialDesc, MeshData, Model, TargetCamera};

    /// A rig with a lit cube in front of the camera.
    fn cube() -> Option<Rig> {
        let mut rig = Rig::new()?;
        let mesh = rig.mesh(
            &MeshData::cube(0.5),
            MaterialDesc {
                color: Color::WHITE,
                ..Default::default()
            },
        );
        rig.ctx.world.spawn((Transform3D::default(), Model::new(mesh)));
        rig.ctx.scene3d.camera.position = vec3(0.0, 1.0, 4.0);
        rig.ctx.scene3d.camera.look_at(Vec3::ZERO);
        Some(rig)
    }

    #[test]
    fn the_depth_survives_the_opaque_pass_for_the_occlusion_grid() {
        // Without a solid prepass the occlusion grid is reduced after the
        // opaque pass; ambient occlusion alone must not let that pass
        // discard the depth it reads.
        let Some(mut rig) = cube() else { return };
        // Both need compute passes.
        if rig.device.limits().max_storage_buffers_per_shader_stage == 0 {
            return;
        }
        rig.renderer.solid_prepass = false;
        rig.ctx.scene3d.post.ambient_occlusion.enabled = true;
        rig.ctx.scene3d.camera.occlusion_culling = true;
        for _ in 0..3 {
            rig.draw();
        }
        let features = last_features().unwrap();
        assert!(features.contains(Features::OCCLUSION.with(Features::AO)), "{features}");
        assert!(!features.contains(Features::SOLID_PREPASS));
        assert_eq!(depth_after_opaque(&rig.renderer), Some(wgpu::StoreOp::Store));
    }

    #[test]
    fn the_depth_is_still_discarded_when_nothing_reads_it_after_the_opaque_pass() {
        // The same frame without the occlusion readback: nothing reads the
        // depth after the opaque pass, so a GPU that draws in tiles doesn't
        // write it back to memory.
        let Some(mut rig) = cube() else { return };
        if rig.device.limits().max_storage_buffers_per_shader_stage == 0 {
            return;
        }
        rig.renderer.solid_prepass = false;
        rig.ctx.scene3d.post.ambient_occlusion.enabled = true;
        rig.ctx.scene3d.camera.occlusion_culling = false;
        for _ in 0..3 {
            rig.draw();
        }
        let features = last_features().unwrap();
        assert!(features.contains(Features::AO), "{features}");
        assert!(
            !features.intersects(Features::OCCLUSION.with(Features::SOLID_PREPASS)),
            "{features}"
        );
        assert_eq!(depth_after_opaque(&rig.renderer), Some(wgpu::StoreOp::Discard));
        // With a solid prepass too, with or without the readback.
        let Some(mut rig) = cube() else { return };
        rig.ctx.scene3d.camera.occlusion_culling = true;
        for _ in 0..3 {
            rig.draw();
        }
        assert!(last_features().unwrap().contains(Features::SOLID_PREPASS));
        assert_eq!(depth_after_opaque(&rig.renderer), Some(wgpu::StoreOp::Discard));
    }

    #[test]
    fn a_probe_bake_with_a_face_that_is_not_a_render_target_records_nothing() {
        // The bake's part is in the frame (the frame asks for the bake), but
        // without all six faces nothing is captured: the part records no
        // probe step, and none is expected of it.
        let Some(mut rig) = cube() else { return };
        rig.probe_bakes.push((1, [900, 901, 902, 903, 904, 905]));
        rig.draw();
        let features = last_features().unwrap();
        assert!(features.contains(Features::VIEWS), "{features}");
        assert!(
            !features.intersects(Features::PROBES.with(Features::TARGET_CAMERAS)),
            "{features}"
        );
        // Five of six faces: still nothing.
        let texture = rig.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("probe face test"),
            size: wgpu::Extent3d {
                width: 16,
                height: 16,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        for id in 900..905 {
            rig.renderer.add_render_target(id, &texture);
        }
        rig.draw();
        assert!(!last_features().unwrap().intersects(Features::PROBES));
        // All six: the bake is recorded.
        rig.renderer.add_render_target(905, &texture);
        rig.draw();
        assert!(last_features().unwrap().contains(Features::PROBES));
    }

    #[test]
    fn cut_outs_get_their_coverage_pass_in_the_main_view_and_in_target_cameras() {
        let Some(mut rig) = cube() else { return };
        rig.draw();
        let plain = last_features().unwrap();
        assert!(plain.contains(Features::MSAA.with(Features::SUN_SHADOWS)), "{plain}");
        assert!(!plain.intersects(Features::CUTOUTS.with(Features::VIEWS)), "{plain}");
        let cutout = rig.mesh(
            &MeshData::cube(0.5),
            MaterialDesc {
                blend: crate::three::Blend::Cutout,
                emissive: Color::WHITE,
                ..Default::default()
            },
        );
        rig.ctx
            .world
            .spawn((Transform3D::at(vec3(1.5, 0.0, 0.0)), Model::new(cutout)));
        rig.draw();
        let main = last_features().unwrap();
        assert!(main.contains(Features::CUTOUTS), "{main}");
        assert!(!main.intersects(Features::TARGET_CUTOUTS), "{main}");
        // A target camera that sees it draws its own coverage pass.
        let texture = rig.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("target camera test"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        rig.ctx.set_headless(true);
        let handle = rig.ctx.gfx.create_render_target(64, 64);
        rig.renderer.add_render_target(handle.id, &texture);
        let camera = rig.ctx.scene3d.camera;
        rig.ctx
            .scene3d
            .passes
            .target_cameras
            .push(TargetCamera::new(camera, handle));
        rig.draw();
        let both = last_features().unwrap();
        let expected = Features::CUTOUTS
            .with(Features::TARGET_CUTOUTS)
            .with(Features::TARGET_CAMERAS)
            .with(Features::VIEWS);
        assert!(both.contains(expected), "{both}");
    }
}
