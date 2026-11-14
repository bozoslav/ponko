//! Each frame: the visible [`Model`]s, as GPU instances grouped by mesh, and
//! the models that cast shadows into the sun's shadow map.

use glam::{Mat4, Vec3, Vec4};

use super::lights::MAX_LIGHT_SHADOW_VIEWS;
use super::retained::{self, Lists, OverrideIds, Tracked, Transients};
use super::{
    Assets3D, Camera3D, Lighting, LodGroup, LodHysteresis, LodPick, LodRenderer, LodSettings, LodShadow, Model,
    ModelMaterial, ModelMaterials, ReceivesDecals, ShadowMesh, SkinFrame, TERRAIN_LODS, TerrainTile, Transform3D,
    linear,
};
use ponko_ecs::{Or, With, Without, World};

/// Default width and height of each of the sun's shadow maps, in texels.
pub(crate) const SHADOW_MAP_SIZE: u32 = 2048;
/// Smallest and largest sun shadow map a game may ask for.
pub(crate) const MIN_SHADOW_RESOLUTION: u32 = 256;
pub(crate) const MAX_SHADOW_RESOLUTION: u32 = 4096;

/// The most shadow cascades: shadow maps, each covering a stretch of the
/// view further out than the one before.
pub(crate) const MAX_CASCADES: usize = 4;
/// Shadow views: the sun's cascades in `0..MAX_CASCADES`, then point and
/// spot lights' views.
pub(crate) const MAX_SHADOW_VIEWS: usize = MAX_CASCADES + super::lights::MAX_LIGHT_SHADOW_VIEWS;

/// One drawn model, as the GPU reads it: the model matrix's columns, and the
/// tint in linear color.
///
/// The model matrix is affine, so its fourth row is always (0, 0, 0, 1):
/// its first slot holds the decal layers the model takes, and its second
/// the instance's crossfade (see [`Instance3D::with_fade`]), which costs no
/// more instance data.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct Instance3D {
    pub model: [[f32; 4]; 4],
    pub color: [f32; 4],
}

impl Instance3D {
    /// The same, dithered for a crossfade: `fade` is 0 for none (drawn
    /// whole), else the share of pixels kept (0 to 1): positive for the
    /// level fading in, and negative for the one fading out, which keeps the
    /// pixels the other doesn't (`-(1 - fade)` for a level that is `fade`
    /// of the way to the other). The shader reads it from the second column's `w`.
    pub(crate) fn with_fade(mut self, fade: f32) -> Self {
        self.model[1][3] = fade;
        self
    }

    /// The crossfade set by [`with_fade`](Self::with_fade).
    pub(crate) fn fade(&self) -> f32 {
        self.model[1][3]
    }
}

/// The renderer uploads the instances that changed since the frame before;
/// two of them with up to this many unchanged ones between go in one write.
pub(crate) const INSTANCE_BRIDGE: usize = 15;

/// Set in the mesh id of a run whose instances are crossfading (dithered):
/// their depth and shadow passes run a fragment shader for the dither.
pub(crate) const FADED: u32 = 1 << 31;

/// This frame's 3D drawing. `instances` holds the models each view draws
/// (the camera, then every shadow view), each mesh's in a block of their
/// own; `runs` and `shadow_runs` say where each mesh's instances are. Kept
/// between frames: a model keeps its place in `instances` while a view
/// draws it, and a frame only works out what changed (see
/// [`retained`](super::retained)), without allocating once warm.
#[derive(Default)]
pub(crate) struct Frame3D {
    pub static_shadowless: Vec<u32>,
    pub(crate) instance_batch_revision: u64,
    /// The instances the runs point into. Between the blocks (and past a
    /// block's instances, where it has room to grow) are instances no run
    /// draws.
    pub instances: Vec<Instance3D>,
    pub probes: [super::probe::ProbeUniform; 4],
    pub probe_uploads: Vec<(u32, std::sync::Arc<Vec<u16>>)>,
    pub probe_bakes: Vec<(u32, [u32; 6])>,
    pub particles: super::particles::ParticleFrame,
    pub lights: super::lights::LightGrid,
    /// Projected decals, in the lights' grid.
    pub decals: super::projected::DecalGrid,
    /// The decal texture array's id, when the game set one.
    pub decal_array: Option<u32>,
    /// (mesh id, first instance, count) for the camera, in mesh order.
    pub runs: Vec<(u32, u32, u32)>,
    /// The same for each shadow view (see [`MAX_SHADOW_VIEWS`]); empty
    /// without shadows.
    pub shadow_runs: [Vec<(u32, u32, u32)>; MAX_SHADOW_VIEWS],
    pub view_proj: Mat4,
    /// How many cascades are drawn: 0 without shadows.
    pub cascades: usize,
    /// The sun's shadow map size, the largest light shadow view's and the
    /// lights' shadow atlas's this frame, in texels: what `Lighting` asks
    /// for, within the GPU's limit.
    pub shadow_size: u32,
    pub light_shadow_size: u32,
    pub light_atlas: u32,
    /// How many point and spot light shadow views `Lighting` allows (0 with
    /// shadows off).
    pub light_budget: usize,
    /// The largest texture the GPU makes (0: not known yet).
    pub size_limit: u32,
    /// How many point and spot light shadow views are drawn (their matrices
    /// are `lights.shadow_views`).
    pub light_views: usize,
    /// Each cascade's view-projection, from the sun.
    pub sun_view_proj: [Mat4; MAX_CASCADES],
    /// Each cascade's size, in world units per shadow-map texel.
    pub cascade_texel: [f32; MAX_CASCADES],
    /// Which cascades are drawn this frame, one bit each: the rest keep
    /// what an earlier frame drew into them (see `Lighting::shadow_refresh`).
    pub redrawn: u32,
    /// What each far cascade's shadow map holds, from the frame that drew it.
    cascade_cache: [Option<CachedCascade>; MAX_CASCADES],
    /// Counts frames, for taking turns redrawing the far cascades.
    frame_number: u32,
    /// The screen effects to run, in order: (effect, material).
    pub effects: Vec<(u32, u32)>,
    /// The target effects to run, in order: (effect, target texture, material).
    pub target_effects: Vec<(u32, u32, u32)>,
    /// The compute passes to run, in order: (compute, target texture, material).
    pub computes: Vec<(u32, u32, u32)>,
    /// The scene compute passes to run, in order, after the lit frame is
    /// settled: as `computes`.
    pub scene_computes: Vec<(u32, u32, u32)>,
    /// How many instances the camera draws.
    pub visible: usize,
    /// How many instances the camera and this frame's shadow views draw.
    pub drawn: usize,
    /// How many models some view holds (the camera's, or a shadow view's,
    /// whether this frame draws it again or keeps its map), each once.
    pub models: usize,
    /// An earlier frame's depth to cull hidden models with, when the camera
    /// has [`occlusion_culling`](Camera3D::occlusion_culling) on.
    pub occlusion: Option<super::occlusion::OcclusionGrid>,
    /// How many models (and shadow-only casters) in view were culled as
    /// hidden this frame.
    pub occluded: usize,
    /// What each target camera draws, in `RenderPasses::target_cameras` order (only
    /// the main frame has these).
    pub views: Vec<ViewFrame>,
    /// The light the sky gave last, and what it was worked out from.
    pub sky_light: Option<(Lighting, super::Sky, super::SkyLight)>,
    /// This frame's skinned models (only the main frame has these; target
    /// cameras draw the same). In `runs`, skinned model `k` is mesh id
    /// `assets.bounds.len() + k`.
    pub skins: SkinFrame,
    /// The global LOD settings to select with (set by the engine each frame).
    pub lod: LodSettings,
    /// Which models moved since last frame (when temporal antialiasing is on).
    pub motion: super::motion::Motion,
    /// Global illumination's probes and what they draw, when it's on (only
    /// the main frame has this).
    pub gi: Option<Box<super::gi::GiFrame>>,
    /// The meshes drawn with other materials than their own (a
    /// [`ModelMaterial`], [`ModelMaterials`] or both), each combination
    /// once, in the world's order. In `runs`, entry `k` is mesh id
    /// `override_base + k`. Every frame collected from the same world
    /// numbers them alike, so target cameras and probe views share the main
    /// frame's.
    pub material_overrides: Overrides,
    /// The first mesh id of `material_overrides`: past the meshes and the
    /// skinned models.
    pub override_base: u32,
    /// Each view's instances, and what they were collected from.
    lists: Lists,
    tracked: Tracked,
    transients: Transients,
    /// Per shadow-only caster, which passes see it (see `collect`), kept
    /// between frames.
    flags: Vec<u32>,
    /// Per LOD group: what the camera picked, which passes see any of it,
    /// and how many of its renderers the occlusion grid hid.
    lod_picks: Vec<(LodPick, u32, u32)>,
    /// Per LOD group, in query order: the entity and the pick it showed
    /// last frame, for [`LodHysteresis`]. Each frame (main camera, render
    /// target) keeps its own.
    lod_held: Vec<(Option<ponko_ecs::Entity>, LodPick)>,
}

/// A mesh drawn with other materials than its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct MaterialOverride {
    pub mesh: u32,
    /// The material of every part ([`ModelMaterial`]), or `u32::MAX`: each
    /// part's own.
    pub material: u32,
    /// Its per-part materials ([`ModelMaterials`]), which win over
    /// `material`: where they start in [`Overrides::parts`], and how many.
    pub parts: (u32, u32),
}

impl MaterialOverride {
    /// No override: every part's own material.
    pub const NONE: (u32, (u32, u32)) = (u32::MAX, (0, 0));

    /// The material `part` is drawn with: its entry in `parts` (this
    /// override's slice of the per-part list, sorted by part), else the
    /// whole-mesh `material`, else its `own`. An id that isn't one of the
    /// `materials` made falls through the same way.
    pub fn resolve(material: u32, parts: &[(u32, u32)], part: usize, own: u32, materials: u32) -> u32 {
        let listed = parts
            .binary_search_by_key(&(part as u32), |&(p, _)| p)
            .ok()
            .map(|at| parts[at].1);
        [listed, Some(material)]
            .into_iter()
            .flatten()
            .find(|&m| m < materials)
            .unwrap_or(own)
    }
}

/// A frame's [`MaterialOverride`]s. Keeps its tables between frames, so a
/// warm frame doesn't allocate.
#[derive(Default)]
pub(crate) struct Overrides {
    pub list: Vec<MaterialOverride>,
    /// `(part, material)` of every distinct per-part list, each sorted by
    /// part.
    pub parts: Vec<(u32, u32)>,
    /// Each distinct per-part list's place in `parts`.
    sets: Vec<(u32, u32)>,
    /// A list's hash to its number in `sets` (a clash takes the next hash).
    set_index: std::collections::HashMap<u64, u32>,
    /// (mesh, material, list number) to its place in `list`.
    index: std::collections::HashMap<(u32, u32, u32), u32>,
}

impl Overrides {
    fn clear(&mut self) {
        self.list.clear();
        self.parts.clear();
        self.sets.clear();
        self.set_index.clear();
        self.index.clear();
    }

    pub(crate) fn hash(parts: &[(u32, crate::render::Material)]) -> u64 {
        // FNV-1a: the lists are a few entries long.
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for &(part, material) in parts {
            for v in [part, material.id] {
                h = (h ^ u64::from(v)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        h
    }

    fn same(&self, set: u32, parts: &[(u32, crate::render::Material)]) -> bool {
        let (start, len) = self.sets[set as usize];
        let mine = &self.parts[start as usize..(start + len) as usize];
        mine.len() == parts.len() && mine.iter().zip(parts).all(|(a, b)| *a == (b.0, b.1.id))
    }

    /// The number of per-part list `parts`, if it was added.
    fn set_of(&self, parts: &[(u32, crate::render::Material)]) -> Option<u32> {
        let mut h = Self::hash(parts);
        loop {
            let &set = self.set_index.get(&h)?;
            if self.same(set, parts) {
                return Some(set);
            }
            h = h.wrapping_add(1);
        }
    }

    /// Its number, adding it if it's new.
    fn add_set(&mut self, parts: &[(u32, crate::render::Material)]) -> u32 {
        let mut h = Self::hash(parts);
        while let Some(&set) = self.set_index.get(&h) {
            if self.same(set, parts) {
                return set;
            }
            h = h.wrapping_add(1);
        }
        let set = self.sets.len() as u32;
        self.sets.push((self.parts.len() as u32, parts.len() as u32));
        self.parts.extend(parts.iter().map(|&(part, m)| (part, m.id)));
        self.set_index.insert(h, set);
        set
    }

    /// Notes that `mesh` is drawn with these overrides (at least one).
    fn add(&mut self, mesh: u32, material: Option<&ModelMaterial>, parts: Option<&ModelMaterials>) {
        let parts = parts.map(ModelMaterials::raw).filter(|p| !p.is_empty());
        if material.is_none() && parts.is_none() {
            return;
        }
        let set = parts.map_or(u32::MAX, |p| self.add_set(p));
        let material = material.map_or(u32::MAX, |m| m.0.id);
        let next = self.list.len() as u32;
        if let std::collections::hash_map::Entry::Vacant(slot) = self.index.entry((mesh, material, set)) {
            slot.insert(next);
            self.list.push(MaterialOverride {
                mesh,
                material,
                parts: if set == u32::MAX {
                    (0, 0)
                } else {
                    self.sets[set as usize]
                },
            });
        }
    }

    /// Where `mesh` with these overrides is in `list`, if it has any.
    fn find(&self, mesh: u32, material: Option<&ModelMaterial>, parts: Option<&ModelMaterials>) -> Option<u32> {
        self.find_ids(mesh, material.map_or(u32::MAX, |m| m.0.id), parts)
    }

    /// [`find`](Self::find) with the whole-mesh material by its id
    /// (`u32::MAX`: none).
    pub(crate) fn find_ids(&self, mesh: u32, material: u32, parts: Option<&ModelMaterials>) -> Option<u32> {
        let parts = parts.map(ModelMaterials::raw).filter(|p| !p.is_empty());
        if material == u32::MAX && parts.is_none() {
            return None;
        }
        let set = match parts {
            Some(p) => self.set_of(p)?,
            None => u32::MAX,
        };
        self.index.get(&(mesh, material, set)).copied()
    }

    /// The per-part list of entry `o`.
    #[cfg(test)]
    pub fn parts_of(&self, o: &MaterialOverride) -> &[(u32, u32)] {
        &self.parts[o.parts.0 as usize..(o.parts.0 + o.parts.1) as usize]
    }
}

/// What one target camera draws this frame.
#[derive(Default)]
pub(crate) struct ViewFrame {
    /// The render target's texture id and size.
    pub target: u32,
    pub size: (u32, u32),
    pub camera: Vec3,
    /// What it draws with: the camera's, tilted onto the clip plane.
    pub view_proj: Mat4,
    /// Its models, without shadow casters (it uses the main view's).
    pub frame: Frame3D,
}

impl Frame3D {
    /// Fills the frame with what the camera sees and, with shadows on,
    /// what's inside the sun's shadow box.
    pub fn collect(
        &mut self,
        world: &mut World,
        assets: &Assets3D,
        skins: &SkinFrame,
        camera: &Camera3D,
        lighting: &Lighting,
    ) {
        let batches_changed = self.instance_batch_revision != assets.instance_batch_revision;
        self.instance_batch_revision = assets.instance_batch_revision;
        let static_shadow_changed = self.static_shadowless.iter().copied().ne(assets
            .shadowless
            .iter()
            .enumerate()
            .filter_map(|(i, &off)| off.then_some(i as u32)));
        self.static_shadowless.clear();
        self.static_shadowless.extend(
            assets
                .shadowless
                .iter()
                .enumerate()
                .filter_map(|(i, &off)| off.then_some(i as u32)),
        );
        self.view_proj = camera.view_proj();
        self.particles.collect(world, camera);
        self.shadow_size = lighting.shadow_size(self.size_limit);
        self.light_shadow_size = lighting.light_shadow_size(self.size_limit);
        self.light_atlas = lighting.light_shadow_atlas_size(self.size_limit);
        self.light_budget = if lighting.shadows {
            lighting.light_shadow_budget()
        } else {
            0
        };
        self.lights.collect(
            world,
            camera,
            self.light_budget,
            self.light_shadow_size,
            self.light_atlas,
        );
        self.decals.records.clear();
        self.decals.lists.clear();
        self.decal_array = assets.decal_array.map(|a| a.id);
        if assets.decals_supported {
            self.decals.collect(
                world,
                camera,
                self.lights.depth,
                assets.decal_array.map_or(0, |a| a.layers),
                self.lights.lists.len(),
                self.lights.lights.len(),
            );
        }
        self.light_views = self.lights.shadow_views.len();
        self.cascades = if lighting.shadows && lighting.shadow_distance > 0.0 {
            (lighting.shadow_cascades as usize).clamp(1, MAX_CASCADES)
        } else {
            0
        };
        let splits = cascade_splits(camera.near, lighting.shadow_distance, self.cascades);
        // Each cascade's box is open towards the sun: casters above it
        // still shade what's inside, so its near plane isn't tested.
        // Far cascades can keep an earlier frame's shadow map: each is drawn
        // into a box a little bigger than it needs, and kept while what the
        // camera sees of that stretch stays inside it, the sun stays put and
        // its turn hasn't come (see `Lighting::shadow_refresh`). Kept
        // cascades draw nothing.
        let refresh = lighting.shadow_refresh.clamp(1, MAX_SHADOW_REFRESH);
        let first_far = if refresh > 1 {
            far_cascades(self.cascades)
        } else {
            self.cascades
        };
        self.frame_number = self.frame_number.wrapping_add(1);
        self.redrawn = 0;
        let mut sun_planes = [[Vec4::ZERO; 5]; MAX_CASCADES];
        for i in 0..self.cascades {
            let near = if i == 0 { camera.near } else { splits[i - 1] };
            if i >= first_far {
                let (center, radius) = slice_sphere(camera, near, splits[i]);
                let turn = (self.frame_number + i as u32).is_multiple_of(refresh);
                let kept = self.cascade_cache[i].filter(|c| {
                    !turn
                        && !static_shadow_changed
                        && !batches_changed
                        && c.size == self.shadow_size
                        && c.cascades == self.cascades
                        && c.sun == lighting.sun_direction
                        && center.distance(c.center) + radius <= c.reach
                });
                if let Some(c) = kept {
                    // Its casters stay in its list (culled against the box
                    // its map was drawn with), but no run draws them.
                    self.sun_view_proj[i] = c.view_proj;
                    self.cascade_texel[i] = c.texel;
                } else {
                    let (m, texel, reach) = sun_box(
                        camera,
                        lighting.sun_direction,
                        near,
                        splits[i],
                        self.shadow_size,
                        1.0 + CACHE_MARGIN,
                    );
                    self.cascade_cache[i] = Some(CachedCascade {
                        view_proj: m,
                        texel,
                        center,
                        reach,
                        sun: lighting.sun_direction,
                        size: self.shadow_size,
                        cascades: self.cascades,
                    });
                    self.sun_view_proj[i] = m;
                    self.cascade_texel[i] = texel;
                    self.redrawn |= 1 << i;
                }
            } else {
                self.cascade_cache[i] = None;
                let (m, texel) = sun_view_proj(camera, lighting.sun_direction, near, splits[i], self.shadow_size);
                self.sun_view_proj[i] = m;
                self.cascade_texel[i] = texel;
                self.redrawn |= 1 << i;
            }
            let [l, r, b, t, _, f] = frustum_planes(self.sun_view_proj[i]);
            sun_planes[i] = [l, r, b, t, f];
        }
        for cache in &mut self.cascade_cache[self.cascades..] {
            *cache = None;
        }
        let mut light_planes = [[Vec4::ZERO; 6]; MAX_LIGHT_SHADOW_VIEWS];
        for (planes, m) in light_planes.iter_mut().zip(&self.lights.shadow_views) {
            *planes = frustum_planes(*m);
        }
        // What the sun's shadows can fall on: the view out to the shadow
        // distance. A caster is drawn into the cascades only if its shadow,
        // swept along the sunlight, can reach that (Unity's caster sweep).
        let receivers = frustum_planes(
            Camera3D {
                far: lighting
                    .shadow_distance
                    .clamp(camera.near * 2.0, camera.far.max(camera.near * 2.0)),
                ..*camera
            }
            .view_proj(),
        );
        // Which passes see a model: bit 0 the camera, bit 1 + i shadow view
        // i (cascades, then light views from MAX_CASCADES on).
        let culler = Culler {
            planes: frustum_planes(self.view_proj),
            receivers,
            camera: camera.position,
            sunlight: lighting.sun_direction.try_normalize().unwrap_or(Vec3::NEG_Y),
            sweep: lighting.shadow_distance * 2.0,
            sun_planes,
            cascades: self.cascades,
            texels: self.cascade_texel,
            light_planes,
            light_views: self.light_views,
            // Hidden behind what an earlier frame drew: still casts shadows.
            occlusion: self.occlusion.as_ref().filter(|_| camera.occlusion_culling),
            occluded: std::sync::atomic::AtomicUsize::new(0),
        };
        // `bounds`: the camera mesh's, and the shadow mesh's (`None`: no
        // shadow), as (center, radius) in model space.
        let classify_at = |matrix: Mat4, scale: f32, bounds: (Vec3, f32), shadow_bounds: Option<(Vec3, f32)>| -> u32 {
            culler.classify(
                matrix.transform_point3(bounds.0),
                bounds.1 * scale,
                shadow_bounds.map(|(center, radius)| (matrix.transform_point3(center), radius * scale)),
            )
        };
        let classify = |t: &Transform3D, bounds: (Vec3, f32), shadow_bounds: Option<(Vec3, f32)>| -> u32 {
            classify_at(t.matrix(), t.scale.abs().max_element(), bounds, shadow_bounds)
        };
        // The same, not counted (see `Culler::classify_uncounted`).
        let classify_uncounted = |matrix: Mat4, scale: f32, bounds: (Vec3, f32), shadow_bounds: Option<(Vec3, f32)>| {
            culler.classify_uncounted(
                matrix.transform_point3(bounds.0),
                bounds.1 * scale,
                shadow_bounds.map(|(center, radius)| (matrix.transform_point3(center), radius * scale)),
            )
        };
        // A shadow-only caster's mesh and bounds.
        let shadow_only = |shadow: &ShadowMesh| {
            let mesh = shadow.0.filter(|c| assets.casts_shadows(c.id))?;
            Some((mesh.slot(), assets.live_bounds(mesh.id)?))
        };
        // The matrix's fourth row is always (0, 0, 0, 1): its first three
        // slots carry what the shader needs per model; the first holds the
        // decal layers the model takes.
        let instance = |t: &Transform3D, color: [f32; 4], receives: f32| {
            let mut model = t.matrix().to_cols_array_2d();
            model[0][3] = receives;
            Instance3D { model, color }
        };
        // What the views drew last frame, to bring up to date.
        let mut lists = std::mem::take(&mut self.lists);
        let mut transients = std::mem::take(&mut self.transients);
        let mut arena = std::mem::take(&mut self.instances);
        let mut motion = std::mem::take(&mut self.motion);
        lists.begin(&mut arena);
        transients.begin();
        motion.begin();
        let all_decals = super::projected::receiver_mask(None);
        // Material overrides: each mesh with its overriding materials
        // gets a mesh id of its own past the skinned models, so its models
        // batch together and the rest keep the plain per-mesh path. Only
        // the entities that have an override are visited.
        let real_meshes = assets.bounds.len() as u32;
        // Posed models whose pose changed have motion of their own.
        motion.posed(real_meshes, skins.draws.iter().map(|d| d.posed_motion));
        let override_base = real_meshes + skins.draws.len() as u32;
        self.override_base = override_base;
        let overrides = &mut self.material_overrides;
        overrides.clear();
        world
            .query::<(&Model, &ModelMaterial)>()
            .filter::<Without<ModelMaterials>>()
            .for_each(|(m, o)| overrides.add(m.mesh.slot(), Some(o), None));
        world
            .query::<(&Model, &ModelMaterials, Option<&ModelMaterial>)>()
            .for_each(|(m, parts, o)| overrides.add(m.mesh.slot(), o, Some(parts)));
        world
            .query::<(&LodGroup, Option<&ModelMaterial>, Option<&ModelMaterials>)>()
            .filter::<Or<(With<ModelMaterial>, With<ModelMaterials>)>>()
            .for_each(|(g, o, parts)| {
                for r in g.levels.iter().flat_map(|l| &l.renderers) {
                    overrides.add(r.mesh.slot(), o, parts);
                }
            });
        world
            .query::<(&TerrainTile, Option<&ModelMaterial>, Option<&ModelMaterials>)>()
            .filter::<Or<(With<ModelMaterial>, With<ModelMaterials>)>>()
            .for_each(|(tile, o, parts)| {
                for lod in tile.lods {
                    overrides.add(lod.slot(), o, parts);
                }
            });
        let overrides = &self.material_overrides;
        // A mesh id as the entity draws it: its override's, if it has one.
        let overridden = |mesh: u32, o: Option<&ModelMaterial>, parts: Option<&ModelMaterials>| {
            overrides.find(mesh, o, parts).map_or(mesh, |k| override_base + k)
        };
        // Models: each compared with what it was last frame, culled again
        // when it or the views changed (on all cores), and only then are its
        // instances written again.
        self.tracked.sync(
            world,
            assets,
            &culler,
            &OverrideIds {
                table: overrides,
                base: override_base,
            },
            &mut lists,
            &mut arena,
            &mut motion,
        );
        // The rest is worked out every frame: what a view draws keeps the
        // place it had (found by its key), what none draws any more leaves.
        // `meshes`: what the camera draws, then what each cascade's shadow
        // map draws (the same for models; coarser for distant terrain).
        let mut touch = |key: retained::Key, motion_key: u64, meshes: retained::Meshes, instance: Instance3D, flags| {
            transients.touch(
                &mut lists,
                &mut arena,
                &mut motion,
                key,
                motion_key,
                &meshes,
                flags,
                &instance,
            );
        };
        let flags = &mut self.flags;
        // Shadows with no model: only the cascades that see them.
        world
            .query::<(&Transform3D, &ShadowMesh)>()
            .filter::<Without<Model>>()
            .par_collect_into(flags, |(t, shadow)| {
                // Never drawn for the camera: not counted as hidden from it.
                shadow_only(shadow).map_or(0, |(_, bounds)| {
                    classify_uncounted(t.matrix(), t.scale.abs().max_element(), bounds, Some(bounds)) & !(1 | OCCLUDED)
                })
            });
        let mut at = 0;
        world
            .query::<(ponko_ecs::Entity, &Transform3D, &ShadowMesh)>()
            .filter::<Without<Model>>()
            .for_each(|(e, t, shadow)| {
                let f = flags[at];
                at += 1;
                if f != 0
                    && let Some((mesh, _)) = shadow_only(shadow)
                {
                    touch(
                        (e.to_bits(), retained::SHADOW_ONLY),
                        super::motion::NO_KEY,
                        [mesh; 1 + MAX_CASCADES],
                        instance(t, [1.0; 4], all_decals),
                        f,
                    );
                }
            });
        // Skinned models: each its own mesh, skinned on the GPU this frame.
        for (k, d) in skins.draws.iter().enumerate() {
            // A level-of-detail group's are drawn with the group, below.
            if d.bounds.1 < 0.0 || d.lod != super::skin::NOT_LOD {
                continue;
            }
            let caster = assets.casts_shadows(d.mesh).then_some(d.bounds);
            let f = classify_at(d.matrix, d.scale, d.bounds, caster);
            if f != 0 {
                touch(
                    (d.entity, retained::SKINNED),
                    d.entity,
                    [real_meshes + k as u32; 1 + MAX_CASCADES],
                    d.instance,
                    f,
                );
            }
        }
        // LOD groups: the level (or two, in a crossfade) the camera picks
        // for each, which the shadow maps draw too. Picking and culling the
        // renderers is parallel; the few that are seen are added in order.
        let lod = self.lod;
        let lod_flags = |t: &Transform3D, r: &LodRenderer, drawn: bool, casts: bool| -> Option<(Mat4, u32)> {
            let bounds = assets.live_bounds(r.mesh.id)?;
            let matrix = t.matrix() * r.offset.matrix();
            let scale = t.scale.abs().max_element() * r.offset.scale.abs().max_element();
            let shadow = (casts && assets.casts_shadows(r.mesh.id)).then_some(bounds);
            // Counted once per group, below: a group is classified again
            // after its pick changes or when it is seen.
            let seen = classify_uncounted(matrix, scale, bounds, shadow);
            Some((matrix, if drawn { seen } else { seen & !(1 | OCCLUDED) }))
        };
        // A skinned renderer of a group with a skeleton: its posed draw
        // (see `SkinFrame::lod_draws`), mesh id and flags.
        let skinned = |posed: (u32, &[super::skin::SkinDraw]), ordinal: u32, drawn: bool, casts: bool| {
            let (first, draws) = posed;
            let k = draws.iter().position(|d| d.lod == ordinal)?;
            let d = draws[k];
            if d.bounds.1 < 0.0 {
                return None;
            }
            let shadow = (casts && assets.casts_shadows(d.mesh)).then_some(d.bounds);
            let seen = classify_uncounted(d.matrix, d.scale, d.bounds, shadow);
            Some((
                d,
                real_meshes + first + k as u32,
                if drawn { seen } else { seen & !(1 | OCCLUDED) },
            ))
        };
        let picks = &mut self.lod_picks;
        world
            .query::<(&Transform3D, &LodGroup)>()
            .par_collect_into(picks, |(t, g)| {
                let pick = g.pick_for(t, camera, &lod);
                let (mut seen, mut hidden) = (0, 0);
                lod_draws(g, pick, false, |r, drawn, casts, _, _| {
                    let f = lod_flags(t, r, drawn, casts).map_or(0, |(_, f)| f);
                    seen |= f & !OCCLUDED;
                    hidden += u32::from(f & OCCLUDED != 0);
                });
                (pick, seen, hidden)
            });
        let mut at = 0;
        let held = &mut self.lod_held;
        // Most frames have no posed group: no lookup per group then.
        let any_posed = skins.any_lod_draws();
        held.resize(picks.len(), (None, LodPick::Culled));
        world
            .query::<(
                ponko_ecs::Entity,
                &Transform3D,
                &LodGroup,
                Option<&ReceivesDecals>,
                Option<&LodHysteresis>,
                Option<&ModelMaterial>,
                Option<&ModelMaterials>,
            )>()
            .for_each(|(entity, t, g, decals, hysteresis, material, parts)| {
                // `hidden`: the renderers of the pick the occlusion grid
                // hid from the camera, counted once whatever else sees them.
                let (mut pick, mut seen, mut hidden) = picks[at];
                // Hysteresis: when the pick changes, hold last frame's
                // instead while near its thresholds (the same entity at the
                // same place in the query, else it's new or the query
                // changed: no history this frame).
                if let Some(&LodHysteresis(band)) = hysteresis {
                    let (e, previous) = held[at];
                    if e == Some(entity) && previous != pick {
                        let kept = g.pick_held(g.screen_height(t, camera), &lod, band, previous);
                        if kept != pick {
                            pick = kept;
                            (seen, hidden) = (0, 0);
                            lod_draws(g, pick, false, |r, drawn, casts, _, _| {
                                let f = lod_flags(t, r, drawn, casts).map_or(0, |(_, f)| f);
                                seen |= f & !OCCLUDED;
                                hidden += u32::from(f & OCCLUDED != 0);
                            });
                        }
                    }
                    held[at] = (Some(entity), pick);
                }
                at += 1;
                // Posed renderers move past their rest bounds: test them
                // with their posed bounds, below, whatever `seen` says.
                let posed = if any_posed {
                    skins.lod_draws(entity)
                } else {
                    (0, &[][..])
                };
                if seen != 0 || !posed.1.is_empty() {
                    // Counted again as they are drawn (skinned ones with
                    // their posed bounds).
                    hidden = 0;
                    let receives = super::projected::receiver_mask(decals);
                    // Which of the group's renderers this is, in the order
                    // the pick draws them.
                    let mut renderer = retained::LOD;
                    lod_draws(g, pick, !posed.1.is_empty(), |r, drawn, casts, fade, ordinal| {
                        renderer += 1;
                        if !posed.1.is_empty() && assets.skin(r.mesh.id).is_some() {
                            // Its materials come with the skinned draw.
                            let found = skinned(posed, ordinal, drawn, casts);
                            hidden += found.map_or(0, |(_, _, f)| u32::from(f & OCCLUDED != 0));
                            if let Some((d, mesh, f)) = found.map(|(d, mesh, f)| (d, mesh, f & !OCCLUDED))
                                && f != 0
                            {
                                let mut instance = d.instance;
                                instance.model[0][3] = receives;
                                // Characters move: motion vectors of
                                // their own, as skinned models have.
                                touch(
                                    (entity.to_bits(), renderer),
                                    d.entity,
                                    [mesh; 1 + MAX_CASCADES],
                                    instance.with_fade(fade),
                                    f,
                                );
                            }
                        } else if let Some((matrix, f)) = lod_flags(t, r, drawn, casts).map(|(m, f)| {
                            hidden += u32::from(f & OCCLUDED != 0);
                            (m, f & !OCCLUDED)
                        }) && f != 0
                        {
                            let mut model = matrix.to_cols_array_2d();
                            model[0][3] = receives;
                            let instance = Instance3D {
                                model,
                                color: linear(r.color),
                            };
                            // The group's renderers take its override; a
                            // level's separate shadow casters keep theirs.
                            // (The cheap test first: most groups have no override.)
                            let mesh = if drawn && (material.is_some() || parts.is_some()) {
                                overridden(r.mesh.slot(), material, parts)
                            } else {
                                r.mesh.slot()
                            };
                            // No motion of their own for temporal antialiasing: they
                            // reproject with the camera (scenery stays put).
                            touch(
                                (entity.to_bits(), renderer),
                                super::motion::NO_KEY,
                                [mesh; 1 + MAX_CASCADES],
                                instance.with_fade(fade),
                                f,
                            );
                        }
                    });
                }
                if hidden != 0 {
                    culler.count_occluded(hidden as usize);
                }
            });
        // Terrain: the detail level for how far the camera is from the
        // tile's bounds (the finest level's, which hold every coarser one).
        // Shadow cascade i draws at least level i: its texels are too big
        // for finer detail to show.
        // A `ShadowMesh(None)` on the tile turns its shadow off.
        world
            .query::<(
                ponko_ecs::Entity,
                &Transform3D,
                &TerrainTile,
                Option<&ShadowMesh>,
                Option<&ModelMaterial>,
                Option<&ModelMaterials>,
            )>()
            .for_each(|(e, t, tile, shadow, material, parts)| {
                let Some((center, radius)) = assets.live_bounds(tile.lods[0].id) else {
                    return;
                };
                let scale = t.scale.abs().max_element();
                let away = t.matrix().transform_point3(center).distance(camera.position) - radius * scale;
                let level = tile.lod(away.max(0.0));
                let meshes: [u32; 1 + MAX_CASCADES] = std::array::from_fn(|i| {
                    tile.lods[if i == 0 {
                        level
                    } else {
                        level.max(i - 1).min(TERRAIN_LODS - 1)
                    }]
                    .id
                });
                if meshes.iter().all(|&m| assets.live_bounds(m).is_some()) {
                    let meshes = meshes.map(|m| m & super::SLOT_MASK);
                    let casts = !matches!(shadow, Some(ShadowMesh(None)));
                    let f = classify(t, (center, radius), casts.then_some((center, radius)));
                    if f != 0 {
                        // Most tiles have no override: skip the lookup.
                        let meshes = if material.is_some() || parts.is_some() {
                            meshes.map(|mesh| overridden(mesh, material, parts))
                        } else {
                            meshes
                        };
                        touch(
                            (e.to_bits(), retained::TERRAIN),
                            e.to_bits(),
                            meshes,
                            instance(t, [1.0; 4], all_decals),
                            f,
                        );
                    }
                }
            });
        transients.finish(&mut lists, &mut arena);
        motion.finish();
        lists.finish(&mut arena);
        self.occluded = culler.occluded.into_inner();
        lists.runs(0, &mut self.runs);
        self.visible = lists.count(0);
        self.models = lists.things();
        self.drawn = self.visible;
        for (i, runs) in self.shadow_runs.iter_mut().enumerate() {
            // A kept cascade's casters stay in its list; nothing draws them.
            let drawn = if i < MAX_CASCADES {
                i < self.cascades && self.redrawn & (1 << i) != 0
            } else {
                i - MAX_CASCADES < self.light_views
            };
            if drawn {
                lists.runs(1 + i, runs);
                self.drawn += lists.count(1 + i);
            } else {
                runs.clear();
            }
        }
        self.lists = lists;
        self.transients = transients;
        self.instances = arena;
        self.motion = motion;
        self.collect_gi(world, assets, skins, camera, lighting);
    }
}

impl Frame3D {
    /// How many times the instance array was packed again.
    #[cfg(test)]
    pub(crate) fn repacks(&self) -> u32 {
        self.lists.repacks
    }

    /// Whether the models are worked out without comparing them (see
    /// `Tracked::rush`).
    #[cfg(test)]
    pub(crate) fn rushing(&self) -> bool {
        self.tracked.rushing()
    }

    /// How many plain models (not LOD groups, terrain, skinned models or
    /// static instance batches) were outside the camera's view in the last
    /// frame, tested as the culler tests them, from what is kept per model.
    /// Worked out when asked; `None` while what is kept is stale (see
    /// `Tracked::count_spheres`).
    pub(crate) fn frustum_culled_models(&self) -> Option<usize> {
        let planes = frustum_planes(self.view_proj);
        (self.tracked).count_spheres(|[x, y, z, r]| !sphere_visible(&planes, Vec3::new(x, y, z), r))
    }

    /// Bytes the model collection keeps between frames: its buffers'
    /// capacity (instances, lists and what is remembered per model).
    #[cfg(any(test, feature = "bench-internals"))]
    pub(crate) fn extraction_bytes(&self) -> usize {
        let runs = self.runs.capacity() + self.shadow_runs.iter().map(Vec::capacity).sum::<usize>();
        self.instances.capacity() * size_of::<Instance3D>()
            + self.lists.bytes()
            + self.tracked.bytes()
            + self.transients.bytes()
            + self.flags.capacity() * size_of::<u32>()
            + runs * size_of::<(u32, u32, u32)>()
    }
}

/// What a frame culls with: the camera's and the shadow views' planes.
pub(super) struct Culler<'a> {
    planes: [Vec4; 6],
    /// What the sun's shadows can fall on.
    receivers: [Vec4; 6],
    camera: Vec3,
    sunlight: Vec3,
    /// How far past the camera a caster's shadow is followed.
    sweep: f32,
    /// Each cascade's box, open towards the sun.
    sun_planes: [[Vec4; 5]; MAX_CASCADES],
    cascades: usize,
    texels: [f32; MAX_CASCADES],
    light_planes: [[Vec4; 6]; MAX_LIGHT_SHADOW_VIEWS],
    light_views: usize,
    occlusion: Option<&'a super::occlusion::OcclusionGrid>,
    /// How many spheres in view were culled as hidden.
    occluded: std::sync::atomic::AtomicUsize,
}

/// Set by [`Culler::classify_uncounted`] when the occlusion grid hid the
/// sphere from the camera (past every view bit).
const OCCLUDED: u32 = 1 << 31;
const _: () = assert!(1 + MAX_SHADOW_VIEWS < 31, "the view bits stay clear of OCCLUDED");

impl Culler<'_> {
    /// Which passes see a sphere in the world (bit 0 the camera) and its
    /// shadow caster's (`None`: no shadow; bit 1 + i shadow view i).
    pub fn classify(&self, center: Vec3, radius: f32, caster: Option<(Vec3, f32)>) -> u32 {
        let flags = self.classify_uncounted(center, radius, caster);
        if flags & OCCLUDED != 0 {
            self.count_occluded(1);
        }
        flags & !OCCLUDED
    }

    /// [`classify`](Self::classify) without counting the sphere in
    /// `occluded`: [`OCCLUDED`] is set instead when the occlusion grid hid
    /// it from the camera. For things classified more than once a frame, or
    /// whose camera bit is dropped, which count themselves once.
    pub fn classify_uncounted(&self, center: Vec3, radius: f32, caster: Option<(Vec3, f32)>) -> u32 {
        let mut seen = sphere_visible(&self.planes, center, radius);
        let mut flags = 0;
        if seen
            && let Some(grid) = self.occlusion
            && grid.hides(center - radius, center + radius, self.camera)
        {
            flags = OCCLUDED;
            seen = false;
        }
        flags |= u32::from(seen);
        if let Some((world_center, world_radius)) = caster {
            // How far the shadow can reach: down through the whole view.
            let reach = world_center.distance(self.camera) + self.sweep;
            if capsule_visible(
                &self.receivers,
                world_center,
                world_center + self.sunlight * reach,
                world_radius,
            ) {
                for (i, p) in self.sun_planes[..self.cascades].iter().enumerate() {
                    // Smaller than a texel: its shadow wouldn't show.
                    if world_radius * 2.0 >= self.texels[i] && sphere_visible(p, world_center, world_radius) {
                        flags |= 2 << i;
                    }
                }
            }
            for (i, p) in self.light_planes[..self.light_views].iter().enumerate() {
                if sphere_visible(p, world_center, world_radius) {
                    flags |= 2 << (MAX_CASCADES + i);
                }
            }
        }
        flags
    }

    /// Adds `n` models the occlusion grid hid to this frame's count.
    pub fn count_occluded(&self, n: usize) {
        if n > 0 {
            self.occluded.fetch_add(n, std::sync::atomic::Ordering::Relaxed);
        }
    }

    /// [`classify`](Self::classify) for spheres kept as `xyz` center and
    /// `w` radius; a negative radius is no sphere (the first: nothing is
    /// seen at all).
    pub fn classify_spheres(&self, sphere: [f32; 4], caster: [f32; 4]) -> u32 {
        if sphere[3] < 0.0 {
            return 0;
        }
        let center = |s: [f32; 4]| Vec3::new(s[0], s[1], s[2]);
        self.classify(
            center(sphere),
            sphere[3],
            (caster[3] >= 0.0).then(|| (center(caster), caster[3])),
        )
    }

    /// Whether culling gives other answers than with the views in `key`
    /// (those of the frame before), which become these. An occlusion grid
    /// is new every frame.
    pub fn changed_since(&self, key: &mut Vec<f32>) -> bool {
        let mut changed = self.occlusion.is_some();
        let mut n = 0;
        let mut put = |v: f32| {
            if n == key.len() {
                key.push(v);
                changed = true;
            } else if key[n].to_bits() != v.to_bits() {
                key[n] = v;
                changed = true;
            }
            n += 1;
        };
        put(f32::from(u8::from(self.occlusion.is_some())));
        put(self.cascades as f32);
        put(self.light_views as f32);
        put(self.sweep);
        for v in [self.camera, self.sunlight] {
            v.to_array().into_iter().for_each(&mut put);
        }
        let planes = (self.planes.iter().chain(&self.receivers))
            .chain(self.sun_planes[..self.cascades].iter().flatten())
            .chain(self.light_planes[..self.light_views].iter().flatten());
        for p in planes {
            p.to_array().into_iter().for_each(&mut put);
        }
        self.texels[..self.cascades].iter().copied().for_each(&mut put);
        if n < key.len() {
            key.truncate(n);
            changed = true;
        }
        changed
    }
}

/// The shadow views drawn this frame: the sun's cascades, then the light views.
pub(crate) fn shadow_views(cascades: usize, light_views: usize) -> impl Iterator<Item = usize> + Clone {
    (0..cascades).chain(MAX_CASCADES..MAX_CASCADES + light_views)
}

/// Calls `f(renderer, drawn, casts, fade, ordinal)` for each renderer that
/// `pick` shows: whether the camera draws it, whether it casts a shadow,
/// its dither (see [`Instance3D::with_fade`]) and, with `ordinals`, its
/// place in the group (only posed renderers need it; else counted from 0).
fn lod_draws(group: &LodGroup, pick: LodPick, ordinals: bool, mut f: impl FnMut(&LodRenderer, bool, bool, f32, u32)) {
    for (index, level, fade) in group.drawn(pick).into_iter().flatten() {
        // The renderer's place in the group (`LodGroup::first_renderer`).
        let mut ordinal = if ordinals { group.first_renderer(index) } else { 0 };
        let own_shadow = matches!(level.shadow, LodShadow::Same);
        for r in &level.renderers {
            f(r, true, own_shadow, fade, ordinal);
            ordinal += 1;
        }
        if let LodShadow::Casters(casters) = &level.shadow {
            for r in casters {
                f(r, false, true, fade, ordinal);
                ordinal += 1;
            }
        }
    }
}

/// Where each of `count` cascades ends, as distances from the camera, up to
/// `distance`: a blend of even steps and steps growing by the same factor
/// (the "practical" split), so the near cascades are small and sharp.
pub(crate) fn cascade_splits(near: f32, distance: f32, count: usize) -> [f32; MAX_CASCADES] {
    const LOG_SHARE: f32 = 0.75;
    let near = near.max(1e-3);
    let distance = distance.max(near * 2.0);
    let mut splits = [distance; MAX_CASCADES];
    for (i, split) in splits.iter_mut().enumerate().take(count.saturating_sub(1)) {
        let k = (i + 1) as f32 / count as f32;
        let even = near + (distance - near) * k;
        let log = near * (distance / near).powf(k);
        *split = even + (log - even) * LOG_SHARE;
    }
    splits
}

/// The sun's view-projection for a shadow map covering what the camera sees
/// from `near` to `far` away, and its size in world units per texel: an
/// orthographic box around that part of the view, facing along the
/// sunlight, moved in whole shadow-map texels so shadow edges don't shimmer
/// as the camera moves.
pub(crate) fn sun_view_proj(camera: &Camera3D, sun_direction: Vec3, near: f32, far: f32, size: u32) -> (Mat4, f32) {
    let (m, texel, _) = sun_box(camera, sun_direction, near, far, size, 1.0);
    (m, texel)
}

/// The bounding sphere of what the camera sees from `near` to `far` away:
/// its center and radius. It doesn't change size as the camera turns.
fn slice_sphere(camera: &Camera3D, near: f32, far: f32) -> (Vec3, f32) {
    let far = far.max(camera.near * 2.0);
    // Compute in camera space: inverting a translated view-projection
    // made the radius wobble at rounding boundaries even during tiny moves.
    let near = near.clamp(camera.near, far * 0.5);
    let middle = (near + far) * 0.5;
    let aspect = (camera.viewport.x / camera.viewport.y.max(1.0)).max(1e-3);
    let height = far * (camera.fov_y * 0.5).tan();
    let radius = Vec3::new(height * aspect, height, far - middle).length();
    let center = camera.position + camera.forward() * middle;
    (center, radius)
}

/// [`sun_view_proj`] with the box `grow` times as wide as the view's
/// slice needs, and how far from the slice's center the box reaches
/// (anything within that distance of it is inside the box).
fn sun_box(camera: &Camera3D, sun_direction: Vec3, near: f32, far: f32, size: u32, grow: f32) -> (Mat4, f32, f32) {
    let light = sun_direction.try_normalize().unwrap_or(Vec3::NEG_Y);
    let (center, radius) = slice_sphere(camera, near, far);
    let radius = (radius * grow * 16.0).ceil() / 16.0;

    let up = if light.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
    let rotation = glam::camera::rh::view::look_to_mat4(Vec3::ZERO, light, up);
    // Snap the center to whole texels in the light's view.
    let texel = 2.0 * radius / size.max(1) as f32;
    let snapped = (rotation.transform_point3(center) / texel).round() * texel;
    // Construct the translation directly in light space. A world-space
    // round trip and another look_to introduced non-identical matrices for
    // identical snapped centers (especially along an oblique sun).
    let back = radius * 4.0;
    let view = Mat4::from_translation(-snapped - Vec3::Z * back) * rotation;
    let projection =
        glam::camera::rh::proj::directx::orthographic(-radius, radius, -radius, radius, 0.0, back + radius);
    // Snapping moved the box by up to a texel in each direction.
    (projection * view, texel, radius - texel * 2.0)
}

/// The first of `cascades` cascades that can be kept between frames: the
/// far half (cascades 2 and 3 of 4, 1 of 2).
pub(crate) fn far_cascades(cascades: usize) -> usize {
    cascades.div_ceil(2).max(1)
}

/// What a far cascade's shadow map holds (see `Frame3D::cascade_cache`).
#[derive(Clone, Copy, Debug)]
struct CachedCascade {
    view_proj: Mat4,
    texel: f32,
    /// The center of the view's slice it was drawn for, and how far from
    /// there its box reaches.
    center: Vec3,
    reach: f32,
    /// What it was drawn with: a change to any of them draws it again.
    sun: Vec3,
    size: u32,
    cascades: usize,
}

/// How much bigger a far cascade's box is than it needs to be, so the
/// camera can move a while before it must be drawn again.
const CACHE_MARGIN: f32 = 0.2;

/// The most frames a far cascade may be kept for.
pub(crate) const MAX_SHADOW_REFRESH: u32 = 8;

/// The six planes of a view volume (`xyz` normal pointing in, `w`
/// distance), from a projection-times-view matrix with 0..1 depth: left,
/// right, bottom, top, near, far.
fn frustum_planes(m: Mat4) -> [Vec4; 6] {
    let r = [m.row(0), m.row(1), m.row(2), m.row(3)];
    let planes = [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[2], r[3] - r[2]];
    planes.map(|p| p / p.truncate().length().max(f32::MIN_POSITIVE))
}

fn sphere_visible(planes: &[Vec4], center: Vec3, radius: f32) -> bool {
    planes.iter().all(|p| p.truncate().dot(center) + p.w >= -radius)
}

/// Whether a capsule (a sphere of `radius` swept from `a` to `b`) can be
/// inside the planes: false only when it is wholly outside one of them.
fn capsule_visible(planes: &[Vec4], a: Vec3, b: Vec3, radius: f32) -> bool {
    planes
        .iter()
        .all(|p| (p.truncate().dot(a) + p.w).max(p.truncate().dot(b) + p.w) >= -radius)
}

#[cfg(test)]
mod tests {
    use super::super::TERRAIN_LODS;
    use super::*;
    use crate::color::Color;
    use crate::render::Mesh;
    use glam::Vec2;

    fn camera() -> Camera3D {
        let mut camera = Camera3D::new(Vec2::new(800.0, 800.0));
        camera.position = Vec3::new(0.0, 0.0, 10.0);
        camera.look_at(Vec3::ZERO);
        camera.far = 100.0;
        camera
    }

    #[test]
    fn spheres_outside_the_view_are_culled() {
        let planes = frustum_planes(camera().view_proj());
        let seen = |at: Vec3| sphere_visible(&planes, at, 1.0);
        assert!(seen(Vec3::ZERO));
        assert!(!seen(Vec3::new(0.0, 0.0, 20.0)), "behind");
        assert!(!seen(Vec3::new(0.0, 0.0, -200.0)), "past far");
        assert!(!seen(Vec3::new(50.0, 0.0, 0.0)), "far right");
        assert!(seen(Vec3::new(5.0, 0.0, 0.0)), "just in");
        assert!(seen(Vec3::new(0.0, 0.0, 10.5)), "big enough to reach in");
    }

    #[test]
    fn visible_models_are_grouped_by_mesh() {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0); 3],
            ..Default::default()
        };
        let model = |id| Model::new(Mesh { id }).with_color(Color::WHITE);
        for (i, mesh) in [2, 0, 2, 2, 0].into_iter().enumerate() {
            world.spawn((Transform3D::at(Vec3::new(i as f32, 0.0, 0.0)), model(mesh)));
        }
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, 50.0)), model(1))); // behind the camera
        let mut frame = Frame3D::default();
        let lighting = Lighting {
            shadows: false,
            ..Default::default()
        };
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera(), &lighting);
        assert_eq!(frame.runs, vec![(0, 0, 2), (2, 2, 3)]);
        assert_eq!(frame.cascades, 0);
        assert!(frame.shadow_runs.iter().all(Vec::is_empty));
        let xs: Vec<f32> = frame.instances.iter().map(|i| i.model[3][0]).collect();
        assert_eq!(xs, vec![1.0, 4.0, 0.0, 2.0, 3.0]);
    }

    #[test]
    fn terrain_tiles_draw_coarser_further_away() {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0); TERRAIN_LODS],
            ..Default::default()
        };
        let tile = TerrainTile {
            lods: std::array::from_fn(|id| Mesh { id: id as u32 }),
            lod_distance: 5.0,
        };
        // The camera is at z = 10, looking at the origin.
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, 7.0)), tile)); // 2 away: finest
        world.spawn((Transform3D::at(Vec3::ZERO), tile)); // 9 away: every 2nd height
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, -30.0)), tile)); // 39 away: coarsest
        let lighting = Lighting {
            shadows: false,
            ..Default::default()
        };
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera(), &lighting);
        assert_eq!(frame.runs, vec![(0, 0, 1), (1, 1, 1), (3, 2, 1)]);
    }

    #[test]
    fn shadow_casters_include_models_outside_the_view() {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0)],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 5.0, 10.0);
        camera.look_at(Vec3::ZERO);
        let lighting = Lighting {
            sun_direction: Vec3::new(0.0, -1.0, -0.2),
            shadow_distance: 30.0,
            shadow_cascades: 1,
            ..Default::default()
        };
        let model = Model::new(Mesh { id: 0 });
        world.spawn((Transform3D::at(Vec3::ZERO), model)); // seen
        world.spawn((Transform3D::at(Vec3::new(0.0, 40.0, 2.0)), model)); // high above: casts, unseen
        world.spawn((Transform3D::at(Vec3::new(500.0, 0.0, 0.0)), model)); // far away: neither
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        assert_eq!(frame.visible, 1);
        assert_eq!(frame.runs, vec![(0, 0, 1)]);
        assert_eq!(frame.shadow_runs[0], vec![(0, 1, 2)]);
    }

    #[test]
    fn material_overrides_batch_by_mesh_and_material() {
        use crate::render::Material;
        use crate::render::ModelMaterial;
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0); 2],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 5.0, 10.0);
        camera.look_at(Vec3::ZERO);
        let lighting = Lighting {
            sun_direction: Vec3::new(0.0, -1.0, -0.2),
            shadow_distance: 30.0,
            shadow_cascades: 1,
            ..Default::default()
        };
        let (wet, dry) = (ModelMaterial(Material { id: 7 }), ModelMaterial(Material { id: 3 }));
        let rock = Model::new(Mesh { id: 0 });
        let at = |x: f32| Transform3D::at(Vec3::new(x, 0.0, 0.0));
        world.spawn((at(0.0), rock)); // its own material
        world.spawn((at(1.0), rock, wet));
        world.spawn((at(2.0), rock, dry));
        world.spawn((at(-1.0), rock, wet));
        world.spawn((at(-2.0), Model::new(Mesh { id: 1 }))); // another mesh
        // Its own shadow mesh keeps that mesh's materials.
        world.spawn((at(3.0), rock, wet, ShadowMesh(Some(Mesh { id: 1 }))));
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        // Past the two meshes (no skinned models): one id per pair, in the
        // world's order (archetypes as made: wet, dry, then wet with a shadow mesh).
        assert_eq!(frame.override_base, 2);
        let pair = |mesh, material| MaterialOverride {
            mesh,
            material,
            parts: (0, 0),
        };
        assert_eq!(frame.material_overrides.list, vec![pair(0, 7), pair(0, 3)]);
        assert_eq!(frame.visible, 6);
        // Wet rocks batch into one run, apart from the plain and dry ones.
        assert_eq!(frame.runs, vec![(0, 0, 1), (1, 1, 1), (2, 2, 3), (3, 5, 1)]);
        // Shadows: the plain rock, the two wet and the dry one, plus the
        // two mesh-1 casters (the other mesh and the shadow mesh).
        assert_eq!(frame.shadow_runs[0], vec![(0, 6, 1), (1, 7, 2), (2, 9, 2), (3, 11, 1)]);
        // Another frame of the same world numbers the pairs alike.
        let mut view = Frame3D::default();
        view.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        assert_eq!(
            (view.override_base, &view.material_overrides.list),
            (2, &frame.material_overrides.list)
        );
    }

    #[test]
    fn part_lists_lod_groups_and_terrain_get_override_ids() {
        use crate::render::LodLevel;
        use crate::render::LodRenderer;
        use crate::render::Material;
        use crate::render::ModelMaterial;
        use crate::render::ModelMaterials;
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0); 4],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 5.0, 10.0);
        camera.look_at(Vec3::ZERO);
        let lighting = Lighting {
            shadows: false,
            ..Default::default()
        };
        let m = |id| Material { id };
        let rock = Model::new(Mesh { id: 0 });
        let at = |x: f32| Transform3D::at(Vec3::new(x, 0.0, 0.0));
        // The same list written two ways is one entry; with a whole-mesh
        // material as well it's another.
        world.spawn((at(0.0), rock, ModelMaterials::new(&[(1, m(5)), (0, m(6))])));
        world.spawn((at(1.0), rock, ModelMaterials::new(&[(0, m(6)), (1, m(4)), (1, m(5))])));
        world.spawn((at(2.0), rock, ModelMaterials::new(&[(1, m(5))]), ModelMaterial(m(9))));
        // An empty list is no override.
        world.spawn((at(3.0), rock, ModelMaterials::new(&[])));
        // A group's renderers and a tile's levels take theirs.
        let group = LodGroup::new(1.0, vec![LodLevel::new(0.0, vec![LodRenderer::new(Mesh { id: 1 })])]);
        world.spawn((at(-1.0), group.clone(), ModelMaterial(m(9))));
        world.spawn((at(-2.0), group));
        let tile = TerrainTile {
            lods: [Mesh { id: 2 }, Mesh { id: 3 }, Mesh { id: 3 }, Mesh { id: 3 }],
            lod_distance: 1000.0,
        };
        world.spawn((at(-3.0), tile, ModelMaterials::new(&[(0, m(8))])));
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        let o = &frame.material_overrides;
        let listed: Vec<_> = (o.list.iter())
            .map(|e| (e.mesh, e.material, o.parts_of(e).to_vec()))
            .collect();
        let none = u32::MAX;
        assert_eq!(
            listed,
            vec![
                (0, none, vec![(0, 6), (1, 5)]),
                (0, 9, vec![(1, 5)]),
                (1, 9, vec![]),
                (2, none, vec![(0, 8)]),
                (3, none, vec![(0, 8)]),
            ]
        );
        // Ids 4.. are the entries. The plain rock and the plain group keep
        // their meshes; the tile draws its finest level.
        assert_eq!(frame.override_base, 4);
        let meshes: Vec<_> = frame.runs.iter().map(|&(mesh, _, count)| (mesh, count)).collect();
        assert_eq!(meshes, vec![(0, 1), (1, 1), (4, 2), (5, 1), (6, 1), (7, 1)]);
        // A warm frame keeps its tables: the same entries again.
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        assert_eq!(frame.material_overrides.list.len(), 5);
        // Three distinct lists: the tile's two meshes share theirs.
        assert_eq!(frame.material_overrides.parts.len(), 4);
    }

    #[test]
    fn a_part_resolves_to_its_listed_material_then_the_whole_mesh_one_then_its_own() {
        let parts = [(1, 5), (3, 99)];
        let r = |material, part| MaterialOverride::resolve(material, &parts, part, 2, 10);
        assert_eq!(r(u32::MAX, 0), 2); // not listed, no whole-mesh material
        assert_eq!(r(7, 0), 7); // the whole-mesh material
        assert_eq!(r(7, 1), 5); // listed
        assert_eq!(r(7, 3), 7); // listed with a material that doesn't exist
        assert_eq!(r(50, 3), 2); // neither exists
    }

    #[test]
    fn shadowless_meshes_cast_no_shadow() {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0); 2],
            shadowless: vec![false, true],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 5.0, 10.0);
        camera.look_at(Vec3::ZERO);
        let lighting = Lighting {
            sun_direction: Vec3::new(0.0, -1.0, -0.2),
            shadow_distance: 30.0,
            shadow_cascades: 1,
            ..Default::default()
        };
        world.spawn((Transform3D::at(Vec3::ZERO), Model::new(Mesh { id: 0 })));
        world.spawn((Transform3D::at(Vec3::X), Model::new(Mesh { id: 1 }))); // seen, casts nothing
        world.spawn((Transform3D::at(Vec3::new(0.0, 40.0, 2.0)), Model::new(Mesh { id: 1 }))); // neither
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        assert_eq!(frame.visible, 2);
        assert_eq!(frame.runs, vec![(0, 0, 1), (1, 1, 1)]);
        assert_eq!(frame.shadow_runs[0], vec![(0, 2, 1)]);
    }

    #[test]
    fn farther_shadow_cascades_draw_coarser_terrain() {
        let mut world = World::new();
        // Bounds big enough to reach into every cascade.
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 500.0); TERRAIN_LODS],
            ..Default::default()
        };
        let tile = TerrainTile {
            lods: std::array::from_fn(|id| Mesh { id: id as u32 }),
            lod_distance: 5.0,
        };
        world.spawn((Transform3D::default(), tile));
        let lighting = Lighting {
            shadow_distance: 100.0,
            shadow_cascades: 4,
            ..Default::default()
        };
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera(), &lighting);
        assert_eq!(frame.runs.iter().map(|r| r.0).collect::<Vec<_>>(), vec![0]);
        let meshes: Vec<Vec<u32>> = frame
            .shadow_runs
            .iter()
            .take(MAX_CASCADES)
            .map(|c| c.iter().map(|r| r.0).collect())
            .collect();
        assert_eq!(meshes, vec![vec![0], vec![1], vec![2], vec![3]]);
    }

    #[test]
    fn the_shadow_box_holds_the_near_view_and_snaps_to_texels() {
        let camera = camera();
        let sun_dir = Vec3::new(-0.3, -1.0, -0.2);
        let (sun, _) = sun_view_proj(&camera, sun_dir, camera.near, 20.0, SHADOW_MAP_SIZE);
        let in_box = |p: Vec3| {
            let c = sun * p.extend(1.0);
            let n = c.truncate() / c.w;
            n.x.abs() <= 1.0 && n.y.abs() <= 1.0 && (0.0..=1.0).contains(&n.z)
        };
        assert!(in_box(Vec3::ZERO), "what the camera looks at");
        assert!(in_box(camera.position + camera.forward() * 15.0));
        assert!(!in_box(Vec3::new(0.0, 0.0, -200.0)), "past the shadow distance");
        // A small camera move gives the same box, or one moved by whole texels.
        let mut moved = camera;
        moved.position.x += 0.013;
        let a = sun * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let b = sun_view_proj(&moved, sun_dir, moved.near, 20.0, SHADOW_MAP_SIZE).0 * Vec4::new(0.0, 0.0, 0.0, 1.0);
        let texels = (a - b).truncate().truncate() * SHADOW_MAP_SIZE as f32 / 2.0;
        assert!((texels - texels.round()).length() < 2e-2, "{texels}");
    }

    #[test]
    fn cascades_grow_outwards_and_end_at_the_shadow_distance() {
        let s = cascade_splits(0.1, 900.0, 4);
        assert!(s[0] > 0.1 && s[0] < s[1] && s[1] < s[2] && s[2] < s[3], "{s:?}");
        assert_eq!(s[3], 900.0);
        assert!(
            s[0] < 900.0 / 8.0,
            "the first cascade is much smaller than even steps: {s:?}"
        );
        assert_eq!(cascade_splits(0.1, 60.0, 1)[0], 60.0);
    }

    #[test]
    fn near_models_cast_into_near_cascades_only() {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 0.5)],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 2.0, 0.0);
        camera.look_at(Vec3::new(0.0, 2.0, -1.0));
        camera.near = 0.1;
        camera.far = 1000.0;
        let lighting = Lighting {
            sun_direction: Vec3::new(0.0, -1.0, 0.0),
            shadow_distance: 400.0,
            ..Default::default()
        };
        let model = Model::new(Mesh { id: 0 });
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, -3.0)), model)); // near
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, -350.0)), model)); // far
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        assert_eq!(frame.cascades, 4);
        let counts = frame.shadow_runs.map(|r| r.iter().map(|r| r.2).sum::<u32>());
        assert_eq!(counts[0], 1, "only the near model is in the first cascade");
        assert_eq!(
            counts[3], 2,
            "the last cascade's box reaches the far model, and back to the camera"
        );
        assert!(
            frame.cascade_texel[0] < frame.cascade_texel[3] / 10.0,
            "near texels are much finer"
        );
    }

    #[test]
    fn lod_instances_carry_their_decal_layers_and_fade_in_separate_slots() {
        use crate::render::LodGroup;
        use crate::render::LodLevel;
        use crate::render::LodRenderer;
        use crate::three::projected::ReceivesDecals;
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 0.5), (Vec3::ZERO, 0.5)],
            ..Default::default()
        };
        let camera = camera(); // 10 from the origin, 60 degree lens
        // A group 4 tall covers about 35% of the screen: inside level 0's
        // fade band (0.3 up to 0.65).
        let group = LodGroup::new(
            4.0,
            vec![
                LodLevel::new(0.3, vec![LodRenderer::new(Mesh { id: 0 })]).with_fade(0.5),
                LodLevel::new(0.05, vec![LodRenderer::new(Mesh { id: 1 })]),
            ],
        );
        world.spawn((Transform3D::default(), group, ReceivesDecals(0b101)));
        let mut frame = Frame3D::default();
        let lighting = Lighting {
            shadows: false,
            ..Default::default()
        };
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        let seen = &frame.instances[..frame.visible];
        assert_eq!(seen.len(), 2, "both levels draw during the fade");
        for i in seen {
            assert_eq!(i.model[0][3], 5.0, "the group's decal layers");
            assert!(i.fade() != 0.0 && i.fade().abs() < 1.0, "{}", i.fade());
        }
        assert!(seen[0].fade() * seen[1].fade() < 0.0, "complementary patterns");
    }

    /// A world with one model near the camera and one far, the camera and
    /// lighting for the shadow cache tests.
    fn cache_scene(refresh: u32) -> (World, Assets3D, Camera3D, Lighting) {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 0.5)],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 2.0, 0.0);
        camera.look_at(Vec3::new(0.0, 2.0, -1.0));
        camera.near = 0.1;
        camera.far = 1000.0;
        let lighting = Lighting {
            sun_direction: Vec3::new(0.2, -1.0, 0.1),
            shadow_distance: 400.0,
            shadow_refresh: refresh,
            ..Default::default()
        };
        let model = Model::new(Mesh { id: 0 });
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, -3.0)), model));
        world.spawn((Transform3D::at(Vec3::new(0.0, 0.0, -350.0)), model));
        (world, assets, camera, lighting)
    }

    #[test]
    fn far_cascades_take_turns_and_keep_their_maps_in_between() {
        let (mut world, assets, camera, lighting) = cache_scene(2);
        let mut frame = Frame3D::default();
        let mut collect = |frame: &mut Frame3D, camera: &Camera3D, lighting: &Lighting| {
            frame.collect(&mut world, &assets, &SkinFrame::default(), camera, lighting);
            (frame.redrawn, frame.sun_view_proj, frame.shadow_runs.clone())
        };
        let (first, matrices, _) = collect(&mut frame, &camera, &lighting);
        assert_eq!(first, 0b1111, "the first frame draws every cascade");
        let (second, kept, runs) = collect(&mut frame, &camera, &lighting);
        let (third, _, _) = collect(&mut frame, &camera, &lighting);
        // Near cascades every frame; the far two (2 and 3) alternate.
        assert_eq!(second & 0b11, 0b11);
        assert_eq!(third & 0b11, 0b11);
        assert_eq!((second | third) & 0b1100, 0b1100);
        assert_eq!((second & third) & 0b1100, 0, "{second:b} {third:b}");
        let skipped = if second & 0b100 == 0 { 2 } else { 3 };
        assert_eq!(
            kept[skipped], matrices[skipped],
            "a kept cascade samples the map it drew"
        );
        assert!(runs[skipped].is_empty(), "and draws nothing");
        let drawn = 5 - skipped;
        assert!(!runs[drawn].is_empty(), "the other far cascade draws its casters");
    }

    #[test]
    fn a_kept_cascade_is_drawn_again_when_the_camera_leaves_it_or_the_sun_moves() {
        let (mut world, assets, mut camera, mut lighting) = cache_scene(8);
        let mut frame = Frame3D::default();
        let mut redrawn = |camera: &Camera3D, lighting: &Lighting| {
            frame.collect(&mut world, &assets, &SkinFrame::default(), camera, lighting);
            frame.redrawn
        };
        redrawn(&camera, &lighting);
        // Still: at most one far cascade's turn comes up each frame.
        let still = redrawn(&camera, &lighting);
        assert!((still & 0b1100).count_ones() <= 1, "{still:b}");
        // A small step stays inside the bigger boxes.
        camera.position.x += 1.0;
        let step = redrawn(&camera, &lighting);
        assert!((step & 0b1100).count_ones() <= 1, "{step:b}");
        // A long way: every cascade is drawn again.
        camera.position.x += 500.0;
        assert_eq!(redrawn(&camera, &lighting), 0b1111);
        redrawn(&camera, &lighting);
        // The sun moves: the same.
        lighting.sun_direction.x += 0.01;
        assert_eq!(redrawn(&camera, &lighting), 0b1111);
        // So does a new shadow map size.
        lighting.shadow_resolution = 1024;
        assert_eq!(redrawn(&camera, &lighting), 0b1111);
    }

    #[test]
    fn refresh_1_draws_every_cascade_every_frame_in_tight_boxes() {
        let (mut world, assets, camera, lighting) = cache_scene(1);
        let mut frame = Frame3D::default();
        for _ in 0..3 {
            frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
            assert_eq!(frame.redrawn, 0b1111);
        }
        let splits = cascade_splits(camera.near, lighting.shadow_distance, 4);
        let (tight, _) = sun_view_proj(
            &camera,
            lighting.sun_direction,
            splits[2],
            splits[3],
            lighting.shadow_size(0),
        );
        assert_eq!(frame.sun_view_proj[3], tight);
        // With caching on, the far boxes are a little bigger: coarser texels.
        let (mut world, assets, camera, lighting) = cache_scene(2);
        let mut cached = Frame3D::default();
        cached.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        assert!(cached.cascade_texel[3] > frame.cascade_texel[3] * 1.15);
        assert_eq!(cached.cascade_texel[0], frame.cascade_texel[0]);
    }

    #[test]
    fn the_far_half_of_the_cascades_can_be_kept() {
        assert_eq!((1..=4).map(far_cascades).collect::<Vec<_>>(), [1, 1, 2, 2]);
    }

    /// How many shadow instances each cascade draws, after one collect of
    /// `models` (position, radius of a unit-mesh model scaled to it).
    fn cascade_counts(models: &[(Vec3, f32)], sun: Vec3) -> [u32; MAX_CASCADES] {
        let mut world = World::new();
        let assets = Assets3D {
            bounds: vec![(Vec3::ZERO, 1.0)],
            ..Default::default()
        };
        let mut camera = camera();
        camera.position = Vec3::new(0.0, 2.0, 0.0);
        camera.look_at(Vec3::new(0.0, 2.0, -1.0)); // looking down -z
        camera.near = 0.1;
        camera.far = 1000.0;
        let lighting = Lighting {
            sun_direction: sun,
            shadow_distance: 200.0,
            shadow_refresh: 1,
            ..Default::default()
        };
        for &(at, radius) in models {
            let mut t = Transform3D::at(at);
            t.scale = Vec3::splat(radius);
            world.spawn((t, Model::new(Mesh { id: 0 })));
        }
        let mut frame = Frame3D::default();
        frame.collect(&mut world, &assets, &SkinFrame::default(), &camera, &lighting);
        let mut counts = [0; MAX_CASCADES];
        for (i, runs) in frame.shadow_runs.iter().take(MAX_CASCADES).enumerate() {
            counts[i] = runs.iter().map(|r| r.2).sum();
        }
        counts
    }

    #[test]
    fn casters_whose_shadow_cant_reach_the_view_are_skipped() {
        // A tree 15 m behind the camera. With the sun behind the camera the
        // shadow falls forwards into the view; with the sun in front it falls
        // away, behind the camera, where nothing it shades is seen.
        let tree = [(Vec3::new(0.0, 3.0, 15.0), 3.0)];
        let towards_view = cascade_counts(&tree, Vec3::new(0.0, -0.5, -1.0));
        let away = cascade_counts(&tree, Vec3::new(0.0, -0.5, 1.0));
        assert!(towards_view.iter().sum::<u32>() > 0, "{towards_view:?}");
        assert_eq!(away, [0; MAX_CASCADES]);
    }

    #[test]
    fn casters_smaller_than_a_texel_are_left_out_of_coarse_cascades() {
        let sun = Vec3::new(0.2, -1.0, 0.1);
        // Pebbles 150 m away, in the last cascade (texels of about 10 cm).
        let small = cascade_counts(&[(Vec3::new(0.0, 0.0, -150.0), 0.02)], sun);
        let big = cascade_counts(&[(Vec3::new(0.0, 0.0, -150.0), 0.5)], sun);
        assert_eq!(small[3], 0, "{small:?}");
        assert_eq!(big[3], 1, "{big:?}");
        // Near the camera (in view, at eye height) the same pebble still
        // casts: fine texels.
        let near = cascade_counts(&[(Vec3::new(0.0, 2.0, -3.0), 0.02)], sun);
        assert_eq!(near[0], 1, "{near:?}");
    }
}

#[cfg(test)]
mod stable_shadow_tests {
    use super::*;
    #[test]
    fn translated_and_rotated_cameras_keep_a_bit_stable_sphere_radius() {
        let mut camera = Camera3D::new(glam::Vec2::new(1280.0, 720.0));
        let (_, radius) = slice_sphere(&camera, 0.1, 50.0);
        let sun = Vec3::new(0.3, -0.8, 0.4);
        let mut reused = 0;
        let mut previous = sun_view_proj(&camera, sun, 0.1, 50.0, 1024).0;
        for i in 0..200 {
            camera.position.x += 0.0001;
            assert_eq!(slice_sphere(&camera, 0.1, 50.0).1, radius);
            let current = sun_view_proj(&camera, sun, 0.1, 50.0, 1024).0;
            reused += usize::from(current == previous);
            previous = current;
            camera.rotation = glam::Quat::from_rotation_y(i as f32 * 0.000001) * camera.rotation;
        }
        assert!(
            reused > 150,
            "small camera moves should preserve exact projections: {reused}"
        );
        camera.position = Vec3::splat(100_000.0);
        assert_eq!(slice_sphere(&camera, 0.1, 50.0).1, radius);
    }
}
