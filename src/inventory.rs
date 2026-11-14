//! What the game holds right now: live textures, meshes, materials,
//! sounds and other asset handles, with estimated bytes ([`Ctx::asset_inventory`]).
//!
//! Counts come from the engine's own handle tables, the same ones that
//! decide whether a handle is live ([`Ctx::is_live`], [`Ctx::live_counts`]),
//! so they also work in headless runs. Every class agrees with
//! [`LiveCounts`](crate::handle::LiveCounts) except [`AssetClass::Texture`], which adds the engine's own
//! textures because it measures memory, not handles.
//! A windowed game adds what the renderer actually holds on the GPU
//! ([`GpuResidency`]), to cross-check the two.

use std::fmt;

use serde::Serialize;

use crate::ctx::Ctx;

/// A class of asset handle in an [`AssetInventory`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssetClass {
    /// Images from [`Graphics::create_texture`](crate::gfx::Graphics::create_texture) and [`Ctx::load_texture`],
    /// freed with [`Graphics::free`](crate::gfx::Graphics::free). Also counts the engine's own textures
    /// (glyph atlases, UI images), which the game can't free, so it can read
    /// more than [`LiveCounts::textures`](crate::handle::LiveCounts::textures).
    Texture,
    /// 3D: textures cameras draw into ([`Graphics::create_render_target`](crate::gfx::Graphics::create_render_target)),
    /// freed with [`Graphics::free`](crate::gfx::Graphics::free).
    RenderTarget,
    /// 3D: [`TextureArray`](crate::terrain::TextureArray)s, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    TextureArray,
    /// 3D: [`Material`](crate::render::Material)s, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    Material,
    /// 3D: material [`Shader`](crate::render::Shader)s, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    Shader,
    /// 3D: [`Mesh`](crate::render::Mesh)es, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    Mesh,
    /// 3D: [`GpuBuffer`](crate::render::GpuBuffer)s, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    GpuBuffer,
    /// 3D: screen [`Effect`](crate::render::Effect)s, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    Effect,
    /// 3D: [`Compute`](crate::render::Compute) passes, freed with [`Graphics::free`](crate::gfx::Graphics::free).
    Compute,
    /// [`Sound`](crate::audio::Sound)s, loaded or made, freed with [`Audio::free`](crate::audio::Audio::free).
    Sound,
}

impl AssetClass {
    /// Its name in reports, like `"render_target"`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::Texture => "texture",
            Self::RenderTarget => "render_target",
            Self::TextureArray => "texture_array",
            Self::Material => "material",
            Self::Shader => "shader",
            Self::Mesh => "mesh",
            Self::GpuBuffer => "gpu_buffer",
            Self::Effect => "effect",
            Self::Compute => "compute",
            Self::Sound => "sound",
        }
    }
}

impl fmt::Display for AssetClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// How many handles of one [`AssetClass`] are live, and about how much
/// memory they hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AssetCount {
    /// What they are.
    pub class: AssetClass,
    /// Handles made and not freed.
    pub live: usize,
    /// Estimated bytes they hold, on the GPU (or in memory, for sounds).
    /// An estimate from what was handed in: base image levels at 4 bytes
    /// a pixel (mipmaps a 3D material adds are not counted), mesh corners
    /// and indices as the renderer lays them out (skinning and morph data
    /// not counted), decoded audio samples (streamed music counts 0). 0
    /// for classes whose size is small or unknown: materials, shaders,
    /// effects and compute passes.
    pub bytes: u64,
}

/// How much one [`AssetClass`] grew between two inventories, from
/// [`AssetInventory::growth_since`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AssetGrowth {
    /// What grew.
    pub class: AssetClass,
    /// More live handles than before (negative: fewer).
    pub live: i64,
    /// More estimated bytes than before (negative: fewer).
    pub bytes: i64,
}

/// What the renderer actually holds on the GPU, counted from its own
/// tables rather than from handles: a handle freed while its GPU memory
/// stays behind shows up here. `None` in headless runs, which have no GPU.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct GpuResidency {
    /// 2D textures, render targets included, and the renderer's own
    /// (such as its 1x1 white texture).
    pub textures: usize,
    /// Their bytes, mipmaps included.
    pub texture_bytes: u64,
    /// 3D meshes with vertex and index buffers.
    pub meshes: usize,
    /// Those buffers' bytes (skinning buffers not counted).
    pub mesh_bytes: u64,
    /// 3D GPU buffers.
    pub gpu_buffers: usize,
    /// Their bytes.
    pub gpu_buffer_bytes: u64,
}

/// A component that still names a freed handle, from
/// [`AssetInventory::dangling`]: it draws with the default (see
/// [`Ctx::free`]) until it is updated or despawned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct DanglingHandle {
    /// The entity holding it, as its bits ([`Entity::to_bits`](ponko_ecs::Entity::to_bits)).
    pub entity: u64,
    /// The component: `"Sprite"`, `"Model"`, `"ModelMaterial"` or `"ShadowMesh"`.
    pub component: &'static str,
    /// What was freed.
    pub class: AssetClass,
    /// The freed handle's raw id, as its `Debug` output shows it: to tell
    /// records apart in reports, not a way back to the handle.
    pub id: u32,
}

/// Live asset handles by [`AssetClass`], from [`Ctx::asset_inventory`]:
/// to see what a long-running game holds, and to find what grows when it
/// shouldn't (a leak). Take one as a baseline, run the game for a while,
/// and compare with [`growth_since`](Self::growth_since).
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let before = ctx.asset_inventory();
///     let _sprite = ctx.gfx.create_texture(2, 2, &[255; 16]);
///     let after = ctx.asset_inventory();
///     assert_eq!(after.get(ponko::inventory::AssetClass::Texture).live, before.get(ponko::inventory::AssetClass::Texture).live + 1);
///     let growth = after.growth_since(&before);
///     assert_eq!(growth[0].class, ponko::inventory::AssetClass::Texture);
///     assert_eq!(growth[0].bytes, 2 * 2 * 4);
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AssetInventory {
    /// Every class this build has, in [`AssetClass`] order, empty ones too.
    pub classes: Vec<AssetCount>,
    /// What the renderer holds, to cross-check the handles against.
    pub gpu: Option<GpuResidency>,
    /// Sprites and models that still show a freed texture, mesh or
    /// material, in entity order; empty when there are none. A freed mesh's
    /// id goes to the next mesh made, so a model is listed only until then.
    pub dangling: Vec<DanglingHandle>,
}

impl AssetInventory {
    /// The count for `class` (zero when this build doesn't have it, such
    /// as meshes without the `3d` feature).
    #[must_use]
    pub fn get(&self, class: AssetClass) -> AssetCount {
        self.classes
            .iter()
            .find(|c| c.class == class)
            .copied()
            .unwrap_or(AssetCount {
                class,
                live: 0,
                bytes: 0,
            })
    }

    /// Estimated bytes of every class together.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.classes.iter().map(|c| c.bytes).sum()
    }

    /// The classes with more live handles or bytes than in `baseline`,
    /// most bytes first: empty when nothing grew. A leak test takes an
    /// inventory, loads and frees in a loop, and expects this empty.
    #[must_use]
    pub fn growth_since(&self, baseline: &Self) -> Vec<AssetGrowth> {
        let mut grown: Vec<AssetGrowth> = self
            .classes
            .iter()
            .map(|now| {
                let then = baseline.get(now.class);
                AssetGrowth {
                    class: now.class,
                    live: now.live as i64 - then.live as i64,
                    bytes: now.bytes as i64 - then.bytes as i64,
                }
            })
            .filter(|g| g.live > 0 || g.bytes > 0)
            .collect();
        grown.sort_by(|a, b| b.bytes.cmp(&a.bytes).then(b.live.cmp(&a.live)));
        grown
    }
}

impl fmt::Display for AssetInventory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{:<14} {:>8} {:>12}", "class", "live", "bytes")?;
        for c in &self.classes {
            writeln!(f, "{:<14} {:>8} {:>12}", c.class.name(), c.live, c.bytes)?;
        }
        if let Some(g) = &self.gpu {
            writeln!(
                f,
                "on the GPU: {} textures ({} bytes), {} meshes ({} bytes), {} buffers ({} bytes)",
                g.textures, g.texture_bytes, g.meshes, g.mesh_bytes, g.gpu_buffers, g.gpu_buffer_bytes
            )?;
        }
        Ok(())
    }
}

/// Classes whose handles have no table of their own to count from (2D
/// textures): what each live one holds; and when each live handle was
/// made and which file loaded it (`Graphics::asset_handles`). Graphics keeps
/// one, audio another for sounds.
#[derive(Default)]
pub(crate) struct Ledger {
    /// Bytes of each 2D texture made and not freed, by texture id.
    pub textures: std::collections::HashMap<u32, u64>,
    /// When each live handle was made, and its file, by class and id.
    births: std::collections::HashMap<(AssetClass, u32), Birth>,
    /// The frame now (set as each frame begins).
    pub now: u64,
}

/// When a handle was made and which file loaded it.
struct Birth {
    frame: u64,
    owner: Option<std::sync::Arc<str>>,
}

impl Ledger {
    /// Handle `id` of `class` was made this frame (in code, until
    /// [`own`](Self::own) names its file).
    pub(crate) fn made(&mut self, class: AssetClass, id: u32) {
        self.births.insert(
            (class, id),
            Birth {
                frame: self.now,
                owner: None,
            },
        );
    }

    /// Handle `id` of `class` was loaded for asset `name`.
    pub(crate) fn own(&mut self, class: AssetClass, id: u32, name: &str) {
        let now = self.now;
        let birth = self.births.entry((class, id)).or_insert(Birth {
            frame: now,
            owner: None,
        });
        if birth.owner.as_deref() != Some(name) {
            birth.owner = Some(name.into());
        }
    }

    /// Handle `id` of `class` was freed.
    pub(crate) fn freed(&mut self, class: AssetClass, id: u32) {
        self.births.remove(&(class, id));
    }

    /// When handle `id` of `class` was made, and its file.
    fn birth(&self, class: AssetClass, id: u32) -> (u64, Option<&str>) {
        self.births
            .get(&(class, id))
            .map_or((0, None), |b| (b.frame, b.owner.as_deref()))
    }
}

/// Who made a handle, in an [`AssetRecord`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum AssetOwner {
    /// Game code (`create_texture`, `create_mesh`, ...).
    Code,
    /// Loading the named asset (`load_texture("crab.png")`, the glTF file
    /// a model's materials and textures came from, ...).
    File(String),
    /// The engine itself: glyph atlases, UI images. The game can't free it.
    Engine,
}

/// One live handle, with what it holds, its age and its owner, from
/// [`Graphics::asset_handles`](crate::gfx::Graphics::asset_handles): to find which handles of a class that
/// [grows](AssetInventory::growth_since) are piling up, and where they
/// came from.
///
/// ```
/// # use ponko::prelude::*;
/// use ponko::inventory::{AssetClass, AssetOwner};
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let _made = ctx.gfx.create_texture(2, 2, &[255; 16]);
///     let _loaded = ctx.gfx.load_texture("crab.png");
///     let records = ctx.gfx.asset_handles(AssetClass::Texture);
///     let made = records.iter().find(|r| r.owner == AssetOwner::Code).unwrap();
///     assert_eq!(made.bytes, 16);
///     let loaded = records.iter().find(|r| matches!(r.owner, AssetOwner::File(_))).unwrap();
///     assert_eq!(loaded.owner, AssetOwner::File("crab.png".into()));
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct AssetRecord {
    /// What it is.
    pub class: AssetClass,
    /// Its raw id, as the handle's `Debug` output shows it: to tell
    /// records apart in reports (ids are per class), not a way back to the
    /// handle.
    pub id: u32,
    /// Estimated bytes it holds, as [`AssetCount::bytes`] counts them.
    pub bytes: u64,
    /// The frame it was made on ([`Time::frame`](crate::time::Time::frame)).
    pub made_frame: u64,
    /// Frames since then.
    pub age_frames: u64,
    /// Who made it.
    pub owner: AssetOwner,
}

/// Bytes of an RGBA8 image.
pub(crate) fn rgba8_bytes(width: u32, height: u32) -> u64 {
    u64::from(width) * u64::from(height) * 4
}

/// Bytes of an image with every mipmap level down to 1x1.
#[cfg_attr(not(feature = "3d"), allow(dead_code))]
pub(crate) fn mip_chain_bytes(width: u32, height: u32, bytes_per_pixel: u64) -> u64 {
    let (mut w, mut h, mut total) = (u64::from(width.max(1)), u64::from(height.max(1)), 0);
    loop {
        total += w * h * bytes_per_pixel;
        if w == 1 && h == 1 {
            return total;
        }
        w = (w / 2).max(1);
        h = (h / 2).max(1);
    }
}

impl Ledger {
    /// The record of live handle `id` of `class`, `now` being this frame.
    fn record(&self, class: AssetClass, id: u32, bytes: u64, engine: bool) -> AssetRecord {
        let (made_frame, owner) = self.birth(class, id);
        AssetRecord {
            class,
            id,
            bytes,
            made_frame,
            age_frames: self.now.saturating_sub(made_frame),
            owner: match owner {
                _ if engine => AssetOwner::Engine,
                Some(name) => AssetOwner::File(name.to_string()),
                None => AssetOwner::Code,
            },
        }
    }
}

/// Oldest first, then by id.
fn oldest_first(mut records: Vec<AssetRecord>) -> Vec<AssetRecord> {
    records.sort_by_key(|r| (r.made_frame, r.id));
    records
}

impl crate::gfx::Graphics {
    /// Every live handle of `class`, with its bytes, age and owner, oldest
    /// first (see [`AssetRecord`]); empty for [`AssetClass::Sound`], which
    /// [`Audio::asset_handles`](crate::audio::Audio::asset_handles) lists.
    /// It allocates: for diagnostics and tests, after
    /// [`Ctx::asset_inventory`] says which class to look at.
    #[must_use]
    pub fn asset_handles(&self, class: AssetClass) -> Vec<AssetRecord> {
        let ledger = &self.asset_ledger;
        let mut out = Vec::new();
        match class {
            AssetClass::Texture => {
                for (&id, &bytes) in &ledger.textures {
                    out.push(ledger.record(class, id, bytes, !self.textures_live.was_made(id)));
                }
            }
            AssetClass::Sound => {}
            #[cfg(feature = "3d")]
            _ => self.assets3d.records(class, &self.textures_live, |id, bytes| {
                out.push(ledger.record(class, id, bytes, false));
            }),
            #[cfg(not(feature = "3d"))]
            _ => {}
        }
        oldest_first(out)
    }
}

impl crate::audio::Audio {
    /// Every live sound, with its bytes, age and owner, oldest first (see
    /// [`AssetRecord`]): [`Graphics::asset_handles`](crate::gfx::Graphics::asset_handles)
    /// for sounds.
    #[must_use]
    pub fn asset_handles(&self) -> Vec<AssetRecord> {
        let records = self
            .sounds_live
            .ids()
            .map(|id| {
                self.ledger
                    .record(AssetClass::Sound, id, self.backend.sound_bytes(id), false)
            })
            .collect();
        oldest_first(records)
    }
}

impl Ctx {
    /// Live asset handles by class, with estimated bytes: see
    /// [`AssetInventory`]. Cheap enough to call every frame, but meant for
    /// diagnostics and tests. The control endpoint's `perf.get` includes it
    /// as `assets`.
    #[must_use]
    pub fn asset_inventory(&self) -> AssetInventory {
        let mut classes = vec![AssetCount {
            class: AssetClass::Texture,
            live: self.gfx.asset_ledger.textures.len(),
            bytes: self.gfx.asset_ledger.textures.values().sum(),
        }];
        #[cfg(feature = "3d")]
        classes.extend(self.gfx.assets3d.inventory(&self.gfx.textures_live));
        let (sounds, sound_bytes) = self.audio.backend.inventory();
        classes.push(AssetCount {
            class: AssetClass::Sound,
            live: sounds,
            bytes: sound_bytes,
        });
        let gpu = if self.headless {
            None
        } else {
            self.gfx.renderer.as_ref().and_then(crate::render::Renderer::residency)
        };
        AssetInventory {
            classes,
            gpu,
            dangling: self.dangling_handles(),
        }
    }

    /// Components that name a freed handle (see [`AssetInventory::dangling`]).
    fn dangling_handles(&self) -> Vec<DanglingHandle> {
        let mut out = Vec::new();
        let world = &self.world;
        // `was_made` keeps `NONE` and the engine's own textures out.
        let textures = &self.gfx.textures_live;
        each::<crate::d2::Sprite>(world, |e, s| {
            if textures.was_made(s.texture.id) && !textures.contains(s.texture.id) {
                out.push(DanglingHandle::new(e, "Sprite", AssetClass::Texture, s.texture.id));
            }
        });
        #[cfg(feature = "3d")]
        {
            let gfx = &self.gfx;
            let freed_mesh = |m: crate::render::Mesh| m != crate::render::Mesh::NONE && !gfx.is_live(m);
            each::<crate::render::Model>(world, |e, m| {
                if freed_mesh(m.mesh) {
                    out.push(DanglingHandle::new(e, "Model", AssetClass::Mesh, m.mesh.id));
                }
            });
            each::<crate::render::ShadowMesh>(world, |e, m| {
                if let Some(mesh) = m.0.filter(|&m| freed_mesh(m)) {
                    out.push(DanglingHandle::new(e, "ShadowMesh", AssetClass::Mesh, mesh.id));
                }
            });
            let materials = &gfx.assets3d.materials_live;
            each::<crate::render::ModelMaterial>(world, |e, m| {
                if materials.was_made(m.0.id) && !materials.contains(m.0.id) {
                    out.push(DanglingHandle::new(e, "ModelMaterial", AssetClass::Material, m.0.id));
                }
            });
        }
        out.sort_by_key(|d| d.entity);
        out
    }
}

/// Calls `f` with every entity that has a `T`, without a mutable borrow of
/// the world.
fn each<T: ponko_ecs::Component>(world: &ponko_ecs::World, mut f: impl FnMut(ponko_ecs::Entity, &T)) {
    let Some(id) = world.component_id::<T>() else {
        return;
    };
    for arch in world.raw_ref().archetypes().iter().filter(|a| a.has(id)) {
        for &e in arch.entities() {
            if let Some(c) = world.get::<T>(e) {
                f(e, c);
            }
        }
    }
}

impl DanglingHandle {
    fn new(e: ponko_ecs::Entity, component: &'static str, class: AssetClass, id: u32) -> Self {
        DanglingHandle {
            entity: e.to_bits(),
            component,
            class,
            id,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mip_chains_add_every_level() {
        assert_eq!(mip_chain_bytes(1, 1, 4), 4);
        // 4x2, 2x1, 1x1.
        assert_eq!(mip_chain_bytes(4, 2, 4), (8 + 2 + 1) * 4);
    }

    #[test]
    fn growth_lists_only_classes_that_grew_biggest_first() {
        let count = |class, live, bytes| AssetCount { class, live, bytes };
        let before = AssetInventory {
            classes: vec![count(AssetClass::Texture, 2, 100), count(AssetClass::Sound, 1, 10)],
            gpu: None,
            dangling: Vec::new(),
        };
        let after = AssetInventory {
            classes: vec![
                count(AssetClass::Texture, 3, 150),
                count(AssetClass::Sound, 1, 10),
                count(AssetClass::Mesh, 1, 900),
            ],
            gpu: None,
            dangling: Vec::new(),
        };
        let growth = after.growth_since(&before);
        assert_eq!(growth.len(), 2);
        assert_eq!(
            (growth[0].class, growth[0].live, growth[0].bytes),
            (AssetClass::Mesh, 1, 900)
        );
        assert_eq!(
            (growth[1].class, growth[1].live, growth[1].bytes),
            (AssetClass::Texture, 1, 50)
        );
        assert!(before.growth_since(&after).is_empty(), "shrinking is not growth");
        assert!(after.to_string().contains("mesh"));
    }
}
