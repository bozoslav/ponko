//! [`LodGroup`]: levels of detail for any object. Near the camera it shows
//! the detailed meshes; as it shrinks on screen it switches to cheaper
//! ones, and past the last level it is not drawn at all.

use glam::Vec3;

use super::{Camera3D, InstanceLevel, Mesh, Transform3D};
use crate::color::Color;
use ponko_ecs::Component;

/// One mesh of a [`LodLevel`], placed relative to the [`LodGroup`]'s
/// transform.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let trunk = ctx.gfx.load_mesh("trunk.glb");
/// // A mesh 2 m up the group's own Y axis, tinted.
/// let part = ponko::render::LodRenderer::new(trunk)
///     .with_offset(ponko::d3::Transform::at(vec3(0.0, 2.0, 0.0)))
///     .with_color(Color::hex(0xCCBBAA));
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LodRenderer {
    /// What to draw: a mesh with its materials.
    pub mesh: Mesh,
    /// Where it sits in the group's space. Default: at the group's origin.
    pub offset: Transform3D,
    /// Multiplied with the materials' colors, as [`Model::color`](crate::render::Model::color).
    /// Default white.
    pub color: Color,
}

impl LodRenderer {
    /// `mesh` at the group's origin.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let renderer = ponko::render::LodRenderer::new(ctx.gfx.load_mesh("tree.glb"));
    /// # }
    /// ```
    #[must_use]
    pub fn new(mesh: Mesh) -> Self {
        Self {
            mesh,
            offset: Transform3D::default(),
            color: Color::WHITE,
        }
    }

    /// The same, placed at `offset` in the group's space.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(mesh: ponko::render::Mesh) {
    /// let high = ponko::render::LodRenderer::new(mesh).with_offset(ponko::d3::Transform::at(vec3(0.0, 1.0, 0.0)));
    /// # }
    /// ```
    #[must_use]
    pub fn with_offset(self, offset: Transform3D) -> Self {
        Self { offset, ..self }
    }

    /// The same, tinted with `color`.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(mesh: ponko::render::Mesh) {
    /// let red = ponko::render::LodRenderer::new(mesh).with_color(Color::rgb(1.0, 0.2, 0.2));
    /// # }
    /// ```
    #[must_use]
    pub fn with_color(self, color: Color) -> Self {
        Self { color, ..self }
    }
}

/// What a [`LodLevel`] casts into the shadow maps.
#[derive(Debug, Clone, PartialEq, Default)]
pub enum LodShadow {
    /// Its own meshes cast the shadow (those with shadows turned on, see
    /// [`Graphics::set_mesh_shadows`](crate::gfx::Graphics::set_mesh_shadows)).
    #[default]
    Same,
    /// These meshes cast the shadow instead and are not drawn for the
    /// camera: a cheap stand-in (a simplified copy, a billboard), or the
    /// level's detailed meshes when a lower one is shown. An empty list
    /// casts no shadow.
    Casters(Vec<LodRenderer>),
}

/// One level of a [`LodGroup`]: the meshes drawn while the group is at
/// least `screen_height` tall on screen.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(near: ponko::render::Mesh, far: ponko::render::Mesh) {
/// let levels = [
///     ponko::render::LodLevel::new(0.30, vec![ponko::render::LodRenderer::new(near)]),
///     ponko::render::LodLevel::new(0.05, vec![ponko::render::LodRenderer::new(far)]),
/// ];
/// # }
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct LodLevel {
    /// The level is used while the group's height covers at least this
    /// fraction of the screen's height (0 to 1, after the
    /// [bias](LodSettings::bias)). Levels go from the most to the least
    /// detailed, so these go down. Below the last level's, nothing is drawn.
    pub screen_height: f32,
    /// How much of the way up to the previous level's threshold (or to a
    /// full screen, for the first level) this level crossfades into the
    /// next one, from 0 (switch at once) to 1. The last level fades out to
    /// nothing the same way. Default 0.
    pub fade_width: f32,
    /// The meshes drawn.
    pub renderers: Vec<LodRenderer>,
    /// What casts the shadow while this level is shown.
    pub shadow: LodShadow,
}

impl LodLevel {
    /// A level for `renderers`, used from `screen_height` up, switching
    /// at once to the next, casting its own shadow.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let level = ponko::render::LodLevel::new(0.25, vec![]);
    /// assert_eq!(level.fade_width, 0.0);
    /// ```
    #[must_use]
    pub fn new(screen_height: f32, renderers: Vec<LodRenderer>) -> Self {
        Self {
            screen_height,
            fade_width: 0.0,
            renderers,
            shadow: LodShadow::Same,
        }
    }

    /// A level used out to `distance` from a camera with vertical field of
    /// view `fov_y`, for a group of world size `size` (see
    /// [`LodGroup::size`]): the same as [`new`](Self::new) with the screen
    /// height the group has at that distance. For setting levels by
    /// distance, as many games do.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// // Switches to the next level at 40 m, seen with a 60 degree lens.
    /// let level = ponko::render::LodLevel::at_distance(40.0, 8.0, 60f32.to_radians(), vec![]);
    /// assert!((level.screen_height - 0.1732).abs() < 1e-3);
    /// ```
    #[must_use]
    pub fn at_distance(distance: f32, size: f32, fov_y: f32, renderers: Vec<LodRenderer>) -> Self {
        Self::new(LodGroup::screen_height_at(size, distance, fov_y), renderers)
    }

    /// The same, crossfading into the next level over `width` (see
    /// [`fade_width`](Self::fade_width)).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let level = ponko::render::LodLevel::new(0.25, vec![]).with_fade(0.3);
    /// assert_eq!(level.fade_width, 0.3);
    /// ```
    #[must_use]
    pub fn with_fade(self, width: f32) -> Self {
        Self {
            fade_width: width,
            ..self
        }
    }

    /// The same, with `shadow` casting the shadow.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let level = ponko::render::LodLevel::new(0.05, vec![]).with_shadow(ponko::render::LodShadow::Casters(vec![]));
    /// assert_eq!(level.shadow, ponko::render::LodShadow::Casters(vec![]));
    /// ```
    #[must_use]
    pub fn with_shadow(self, shadow: LodShadow) -> Self {
        Self { shadow, ..self }
    }
}

/// Global settings for [`LodGroup`]s, at
/// [`SceneSettings::lod`](crate::render::SceneSettings::lod)
/// (`ctx.scene3d.lod`): what a game's detail setting changes.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// ctx.scene3d.lod.bias = 0.5; // low detail: switch to cheaper levels twice as soon
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct LodSettings {
    /// Multiplies every group's screen height before it is compared with
    /// the levels' thresholds, so 2 keeps detailed levels up to twice as
    /// far away and 0.5 drops to cheaper ones at half the distance. 0
    /// culls every group. Default 1.
    pub bias: f32,
    /// Whether levels with a [`fade_width`](LodLevel::fade_width) crossfade;
    /// off switches them all at once, which costs less. Default on.
    pub crossfade: bool,
}

impl Default for LodSettings {
    fn default() -> Self {
        Self {
            bias: 1.0,
            crossfade: true,
        }
    }
}

/// Which level or levels of a [`LodGroup`] are drawn, from
/// [`LodGroup::pick`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LodPick {
    /// Smaller than the last level's threshold: nothing is drawn.
    Culled,
    /// Level `n` (an index into [`LodGroup::levels`]).
    Level(usize),
    /// Crossfading from level `from` to level `to` (`None`: to nothing,
    /// past the last level). Both are drawn with complementary dither
    /// patterns; `blend` is how far along it is, from 0 (all `from`) to 1
    /// (all `to`).
    Fade {
        /// The level fading out.
        from: usize,
        /// The level fading in.
        to: Option<usize>,
        /// 0 to 1.
        blend: f32,
    },
}

/// Hysteresis for the [`LodGroup`] on the same entity: once a level is
/// shown, it stays until the group's screen height moves this fraction past
/// the level's own range (0 to 1). A camera resting near a threshold then
/// keeps one level instead of switching every frame. Levels that
/// [crossfade](LodLevel::fade_width) already change smoothly; this matters
/// most for those that switch at once. Without the component, levels switch
/// exactly at their thresholds.
///
/// Each camera (the main one, and each render target's) holds its own
/// choice.
///
/// In scene files it's `"LodHysteresis": 0.1`.
///
/// ```
/// # use ponko::prelude::*;
/// let mut world = World::new();
/// let group = ponko::render::LodGroup::new(4.0, vec![ponko::render::LodLevel::new(0.2, vec![])]);
/// // Hold each level until the group is 10% past its threshold.
/// world.spawn((ponko::d3::Transform::default(), group, ponko::render::LodHysteresis(0.1)));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize, Component)]
pub struct LodHysteresis(pub f32);

/// Levels of detail: the meshes this entity draws depend on how much of the
/// screen it covers. Goes on an entity with a [`d3::Transform`](crate::d3::Transform) (and no
/// [`Model`](crate::render::Model): the group draws instead).
///
/// How tall the group looks is the fraction of the screen's height its
/// [`size`](Self::size) covers, which follows its distance, its scale and
/// the camera's field of view, and is the same at any window size (as in
/// Unity). The first level whose
/// [`screen_height`](LodLevel::screen_height) it reaches is drawn, after
/// [`SceneSettings::lod`](crate::render::SceneSettings::lod)'s bias.
///
/// Levels can crossfade: both are drawn for a short stretch, each through
/// a dither pattern that fills in what the other leaves out, so the swap
/// is not seen. The shadow maps use the main camera's choice (and fade
/// the same way). Cameras drawing into render targets choose for
/// themselves. Levels that switch at once can be held near their
/// thresholds with a [`LodHysteresis`], so a camera resting at one doesn't
/// flicker between two.
///
/// In scene files it's `"LodGroup": { "size": 8, "levels": [{ "screen_height": 0.3,
/// "renderers": [{ "mesh": "tree_high.glb" }] }, ...] }`.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let high = ctx.gfx.load_mesh("tree_high.glb");
/// let low = ctx.gfx.load_mesh("tree_low.glb");
/// let billboard = ctx.gfx.load_mesh("tree_billboard.glb");
/// let group = ponko::render::LodGroup::new(
///     6.0, // about 6 m tall
///     vec![
///         ponko::render::LodLevel::new(0.30, vec![ponko::render::LodRenderer::new(high)]).with_fade(0.25),
///         ponko::render::LodLevel::new(0.08, vec![ponko::render::LodRenderer::new(low)]),
///         ponko::render::LodLevel::new(0.02, vec![ponko::render::LodRenderer::new(billboard)]), // gone below 2%
///     ],
/// );
/// ctx.world.spawn((ponko::d3::Transform::at(vec3(10.0, 0.0, -20.0)), group));
/// # }
/// ```
#[derive(Debug, Clone, PartialEq, Component)]
pub struct LodGroup {
    /// The levels, most detailed first, their thresholds going down.
    pub levels: Vec<LodLevel>,
    /// How big the group is, in its own space: the height that counts
    /// towards the screen height (its largest dimension). The transform's
    /// largest scale applies on top. Default 1.
    pub size: f32,
    /// The point in the group's space the distance to the camera is
    /// measured to. Default the origin.
    pub center: Vec3,
}

impl LodGroup {
    /// A group of world `size` with `levels`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let group = ponko::render::LodGroup::new(4.0, vec![ponko::render::LodLevel::new(0.1, vec![])]);
    /// assert_eq!(group.levels.len(), 1);
    /// ```
    #[must_use]
    pub fn new(size: f32, levels: Vec<LodLevel>) -> Self {
        Self {
            levels,
            size,
            center: Vec3::ZERO,
        }
    }

    /// The same, measuring distance to `center` (in the group's space).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let group = ponko::render::LodGroup::new(4.0, vec![]).with_center(vec3(0.0, 2.0, 0.0));
    /// assert_eq!(group.center.y, 2.0);
    /// ```
    #[must_use]
    pub fn with_center(self, center: Vec3) -> Self {
        Self { center, ..self }
    }

    /// The fraction of the screen's height that something of world size
    /// `size` covers at `distance` from a camera with vertical field of
    /// view `fov_y` (radians): `size / (2 · distance · tan(fov_y / 2))`.
    /// To turn distances into thresholds.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// // At 10 m a 60 degree lens sees 11.5 m of height: 2 m is 17% of it.
    /// let h = ponko::render::LodGroup::screen_height_at(2.0, 10.0, 60f32.to_radians());
    /// assert!((h - 0.1732).abs() < 1e-3);
    /// ```
    #[must_use]
    pub fn screen_height_at(size: f32, distance: f32, fov_y: f32) -> f32 {
        size / (2.0 * distance.max(1e-4) * (fov_y * 0.5).tan().max(1e-4))
    }

    /// The distance at which something of world `size` covers `height` of
    /// the screen: the inverse of [`screen_height_at`](Self::screen_height_at).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let fov = 60f32.to_radians();
    /// let d = ponko::render::LodGroup::distance_at(2.0, 0.1732, fov);
    /// assert!((d - 10.0).abs() < 0.01);
    /// ```
    #[must_use]
    pub fn distance_at(size: f32, height: f32, fov_y: f32) -> f32 {
        size / (2.0 * height.max(1e-6) * (fov_y * 0.5).tan().max(1e-4))
    }

    /// The fraction of the screen's height this group covers, seen from
    /// `camera` with the group at `transform`, before any bias.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let group = ponko::render::LodGroup::new(2.0, vec![]);
    ///     ctx.scene3d.camera.position = vec3(0.0, 0.0, 10.0);
    ///     ctx.scene3d.camera.look_at(Vec3::ZERO);
    ///     let h = group.screen_height(&ponko::d3::Transform::default(), &ctx.scene3d.camera);
    ///     assert!((h - 0.1732).abs() < 1e-3);
    /// });
    /// ```
    #[must_use]
    pub fn screen_height(&self, transform: &Transform3D, camera: &Camera3D) -> f32 {
        let center = transform.matrix().transform_point3(self.center);
        let size = self.size * transform.scale.abs().max_element();
        Self::screen_height_at(size, center.distance(camera.position), camera.fov_y)
    }

    /// Which level a group covering `height` of the screen shows, under
    /// `settings`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let group = ponko::render::LodGroup::new(1.0, vec![ponko::render::LodLevel::new(0.5, vec![]), ponko::render::LodLevel::new(0.1, vec![])]);
    /// let settings = ponko::render::LodSettings::default();
    /// assert_eq!(group.pick(0.7, &settings), ponko::render::LodPick::Level(0));
    /// assert_eq!(group.pick(0.2, &settings), ponko::render::LodPick::Level(1));
    /// assert_eq!(group.pick(0.05, &settings), ponko::render::LodPick::Culled);
    /// ```
    #[must_use]
    pub fn pick(&self, height: f32, settings: &LodSettings) -> LodPick {
        /// Blends this close to either end count as the end: not worth
        /// drawing two levels for.
        const SNAP: f32 = 1.0 / 128.0;
        let height = height * settings.bias.max(0.0);
        let Some(i) = self.levels.iter().position(|l| height >= l.screen_height) else {
            return LodPick::Culled;
        };
        let level = &self.levels[i];
        let width = level.fade_width.clamp(0.0, 1.0);
        if settings.crossfade && width > 0.0 {
            let lower = level.screen_height;
            let upper = if i == 0 { 1.0 } else { self.levels[i - 1].screen_height }.max(lower);
            let top = lower + width * (upper - lower);
            if height < top && top > lower {
                let to = (i + 1 < self.levels.len()).then_some(i + 1);
                let blend = 1.0 - (height - lower) / (top - lower);
                return if blend < SNAP {
                    LodPick::Level(i)
                } else if blend > 1.0 - SNAP {
                    to.map_or(LodPick::Culled, LodPick::Level)
                } else {
                    LodPick::Fade { from: i, to, blend }
                };
            }
        }
        LodPick::Level(i)
    }

    /// [`pick`](Self::pick) for this group at `transform`, seen from `camera`.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let group = ponko::render::LodGroup::new(2.0, vec![ponko::render::LodLevel::new(0.1, vec![])]);
    ///     // The camera starts 8 m from the origin, looking at it.
    ///     let pick = group.pick_for(&ponko::d3::Transform::default(), &ctx.scene3d.camera, &ctx.scene3d.lod);
    ///     assert_eq!(pick, ponko::render::LodPick::Level(0));
    /// });
    /// ```
    #[must_use]
    pub fn pick_for(&self, transform: &Transform3D, camera: &Camera3D, settings: &LodSettings) -> LodPick {
        self.pick(self.screen_height(transform, camera), settings)
    }

    /// The group as the distance levels of a static instance batch, for
    /// many copies of it that never move: pass them with the copies'
    /// transforms to
    /// [`Graphics::create_instance_batch_levels`](crate::gfx::Graphics::create_instance_batch_levels).
    /// The batch then picks each copy's level where it is culled (in the
    /// GPU's culling pass on the GPU-driven path, see
    /// [`BatchCulling`](crate::render::BatchCulling)) instead of the CPU
    /// picking it for each entity every frame. A device without compute
    /// culling culls and picks on the CPU, cell by cell, with the same bands.
    ///
    /// The copies' largest scale is `scale` (give copies of other scales
    /// batches of their own); the camera's vertical field of view is
    /// `fov_y` (radians) and the bias is `settings.bias`. Level `i` is drawn
    /// from the distance where the group covers the previous levels'
    /// thresholds to where it covers its own: the distances
    /// [`pick`](Self::pick) switches at, up to rounding there. Its
    /// renderers draw for the camera and cast shadows as the level's
    /// [`shadow`](LodLevel::shadow) says.
    ///
    /// What a batch doesn't do, so a group needing it stays an entity:
    /// crossfades (levels switch at once, as with
    /// [`LodSettings::crossfade`] off), [`LodHysteresis`], a level per
    /// render-target camera, a light's shadow picking by the main camera
    /// (a batch measures a light's shadow from the light), and following
    /// changes to the field of view or bias (make the batch again).
    ///
    /// `None` when the group can't be a batch: its [`center`](Self::center)
    /// is away from the origin (a batch measures to each copy's origin), a
    /// renderer has an offset or a color other than white (a batch has one
    /// transform and color per copy), or nothing would ever be drawn: no
    /// renderers, or a bias of 0 or less (or NaN) and no level with a
    /// threshold of 0 or less. Such a bias is taken as `pick` takes it: the
    /// group covers none of the screen, so only the first level whose
    /// threshold is 0 or less is drawn, at every distance. As everywhere, a
    /// batch doesn't crossfade: with [`LodSettings::crossfade`] on and a
    /// fade width on that level, `pick` shows it fully faded into the next
    /// level (into nothing past the last), where the batch draws it.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # let (near, far) = (ponko::render::Mesh::NONE, ponko::render::Mesh::NONE);
    /// let group = ponko::render::LodGroup::new(
    ///     2.0,
    ///     vec![
    ///         ponko::render::LodLevel::new(0.2, vec![ponko::render::LodRenderer::new(near)]),
    ///         ponko::render::LodLevel::new(0.05, vec![ponko::render::LodRenderer::new(far)]),
    ///     ],
    /// );
    /// let fov = 60f32.to_radians();
    /// let levels = group.instance_levels(1.0, fov, &ponko::render::LodSettings::default()).unwrap();
    /// // The near mesh until the group is 20% of the screen tall, then the far one.
    /// let switch = ponko::render::LodGroup::distance_at(2.0, 0.2, fov);
    /// assert_eq!((levels[0].min_distance, levels[0].max_distance), (0.0, switch));
    /// assert_eq!(levels[1].min_distance, switch);
    /// // Then `ctx.gfx.create_instance_batch_levels(&levels, &transforms, &[], options)`.
    /// ```
    #[must_use]
    pub fn instance_levels(&self, scale: f32, fov_y: f32, settings: &LodSettings) -> Option<Vec<InstanceLevel>> {
        if self.center != Vec3::ZERO {
            return None;
        }
        let plain = |r: &LodRenderer| r.offset == Transform3D::default() && r.color == Color::WHITE;
        fn casters(l: &LodLevel) -> &[LodRenderer] {
            match &l.shadow {
                LodShadow::Casters(c) => c,
                LodShadow::Same => &[],
            }
        }
        if !self
            .levels
            .iter()
            .all(|l| l.renderers.iter().all(plain) && casters(l).iter().all(plain))
        {
            return None;
        }
        // Where the group covers `height` of the screen after the bias
        // (clamped as `pick` clamps it: NaN and below 0 count as 0, and a
        // group of size 0 reaches only thresholds of 0 or less).
        let size = self.size * scale.abs() * settings.bias.max(0.0);
        let reach = |height: f32| {
            if height > 0.0 {
                Self::distance_at(size, height, fov_y)
            } else {
                f32::INFINITY
            }
        };
        let mut levels = Vec::new();
        // A level is drawn past the distance of every earlier threshold
        // (it is picked only where none of them is reached).
        let mut near = 0.0f32;
        for level in &self.levels {
            let far = reach(level.screen_height);
            if far > near {
                let band = |mesh, camera, shadows| InstanceLevel {
                    mesh,
                    min_distance: near,
                    max_distance: far,
                    camera,
                    shadows,
                };
                let own = matches!(level.shadow, LodShadow::Same);
                levels.extend(level.renderers.iter().map(|r| band(r.mesh, true, own)));
                levels.extend(casters(level).iter().map(|r| band(r.mesh, false, true)));
            }
            near = near.max(far);
        }
        (!levels.is_empty()).then_some(levels)
    }

    /// [`pick`](Self::pick), but holding `previous` (last frame's pick)
    /// while `height` (before bias) is within `band` (a fraction, see
    /// [`LodHysteresis`]) of that level's range. Crossfades pass through:
    /// they are continuous already, and they meet the levels they connect
    /// at their ends, so switching into or out of one never jumps.
    pub(crate) fn pick_held(&self, height: f32, settings: &LodSettings, band: f32, previous: LodPick) -> LodPick {
        let raw = self.pick(height, settings);
        let n = self.levels.len();
        let index = |p: LodPick| match p {
            LodPick::Level(i) if i < n => Some(i),
            LodPick::Culled => Some(n),
            _ => None,
        };
        let (Some(now), Some(held)) = (index(raw), index(previous)) else {
            return raw;
        };
        let band = band.clamp(0.0, 1.0);
        if now == held || band == 0.0 {
            return raw;
        }
        let height = height * settings.bias.max(0.0);
        // Level `held` is used from its threshold (0 when culled) up to the
        // previous level's (no limit for the first).
        let lower = self.levels.get(held).map_or(0.0, |l| l.screen_height);
        let upper = held
            .checked_sub(1)
            .map_or(f32::INFINITY, |i| self.levels[i].screen_height);
        if height >= lower * (1.0 - band) && height < upper * (1.0 + band) {
            previous
        } else {
            raw
        }
    }

    /// The levels drawn for `pick`, with each one's dither: 0 for none,
    /// else the share of pixels kept, negative for the pattern that is
    /// the complement of the positive one (see `Instance3D::with_fade`).
    pub(crate) fn drawn(&self, pick: LodPick) -> [Option<(usize, &LodLevel, f32)>; 2] {
        let level = |i: usize, fade: f32| self.levels.get(i).map(|l| (i, l, fade));
        match pick {
            LodPick::Culled => [None, None],
            LodPick::Level(i) => [level(i, 0.0), None],
            LodPick::Fade { from, to, blend } => [level(from, -(1.0 - blend)), to.and_then(|t| level(t, blend))],
        }
    }

    /// Where level `level`'s renderers start when every level's renderers
    /// are counted in order, each level's own followed by its separate
    /// shadow casters: what tells the renderers of a group apart.
    pub(crate) fn first_renderer(&self, level: usize) -> u32 {
        self.levels[..level.min(self.levels.len())]
            .iter()
            .map(|l| {
                let casters = match &l.shadow {
                    LodShadow::Casters(c) => c.len(),
                    LodShadow::Same => 0,
                };
                (l.renderers.len() + casters) as u32
            })
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn group(fade: f32) -> LodGroup {
        let level = |h, f| LodLevel::new(h, vec![]).with_fade(f);
        LodGroup::new(2.0, vec![level(0.5, fade), level(0.2, fade), level(0.05, fade)])
    }

    #[test]
    fn levels_switch_at_their_thresholds() {
        let g = group(0.0);
        let s = LodSettings::default();
        assert_eq!(g.pick(1.0, &s), LodPick::Level(0));
        assert_eq!(g.pick(0.5, &s), LodPick::Level(0));
        assert_eq!(g.pick(0.4999, &s), LodPick::Level(1));
        assert_eq!(g.pick(0.2, &s), LodPick::Level(1));
        assert_eq!(g.pick(0.0501, &s), LodPick::Level(2));
        assert_eq!(g.pick(0.0499, &s), LodPick::Culled);
        assert_eq!(g.pick(0.0, &s), LodPick::Culled);
    }

    #[test]
    fn no_levels_cull_the_group() {
        assert_eq!(
            LodGroup::new(1.0, vec![]).pick(1.0, &LodSettings::default()),
            LodPick::Culled
        );
    }

    #[test]
    fn bias_scales_the_height() {
        let g = group(0.0);
        let at = |bias| {
            g.pick(
                0.3,
                &LodSettings {
                    bias,
                    ..Default::default()
                },
            )
        };
        assert_eq!(at(1.0), LodPick::Level(1));
        assert_eq!(at(2.0), LodPick::Level(0)); // 0.6: detail held longer
        assert_eq!(at(0.5), LodPick::Level(2)); // 0.15: below level 1's 0.2
    }

    #[test]
    fn a_smaller_bias_drops_detail_sooner() {
        let g = group(0.0);
        let low = LodSettings {
            bias: 0.5,
            ..Default::default()
        };
        assert_eq!(g.pick(0.3, &low), LodPick::Level(2)); // 0.15
        assert_eq!(g.pick(0.08, &low), LodPick::Culled); // 0.04
        let off = LodSettings {
            bias: 0.0,
            ..Default::default()
        };
        assert_eq!(g.pick(5.0, &off), LodPick::Culled);
    }

    #[test]
    fn screen_height_follows_distance_fov_and_scale() {
        let g = LodGroup::new(2.0, vec![]);
        let mut camera = Camera3D::new(glam::Vec2::new(800.0, 600.0));
        camera.look_at(Vec3::ZERO);
        let height = |g: &LodGroup, t: &Transform3D, camera: &Camera3D| g.screen_height(t, camera);
        let here = Transform3D::default();
        camera.position = Vec3::new(0.0, 0.0, 10.0);
        camera.fov_y = 60f32.to_radians();
        let base = height(&g, &here, &camera);
        assert!((base - 2.0 / (2.0 * 10.0 * 30f32.to_radians().tan())).abs() < 1e-5);
        // Twice as far: half as tall.
        camera.position = Vec3::new(0.0, 0.0, 20.0);
        assert!((height(&g, &here, &camera) - base / 2.0).abs() < 1e-5);
        // A narrower lens zooms in: taller.
        camera.position = Vec3::new(0.0, 0.0, 10.0);
        camera.fov_y = 30f32.to_radians();
        assert!(height(&g, &here, &camera) > base * 1.9);
        // The transform's scale counts, and so does the center.
        camera.fov_y = 60f32.to_radians();
        assert!((height(&g, &Transform3D::default().scaled(3.0), &camera) - base * 3.0).abs() < 1e-5);
        let high = LodGroup::new(2.0, vec![]).with_center(Vec3::new(0.0, 0.0, 5.0));
        assert!((height(&high, &here, &camera) - base * 2.0).abs() < 1e-5); // 5 m away instead of 10
    }

    #[test]
    fn the_window_size_does_not_change_the_choice() {
        // Screen-relative: the same fraction of the screen at any resolution.
        let g = LodGroup::new(2.0, vec![LodLevel::new(0.1, vec![])]);
        let mut camera = Camera3D::new(glam::Vec2::new(800.0, 600.0));
        let before = g.screen_height(&Transform3D::default(), &camera);
        camera.set_viewport(glam::Vec2::new(3840.0, 2160.0));
        assert_eq!(g.screen_height(&Transform3D::default(), &camera), before);
        camera.set_viewport(glam::Vec2::new(320.0, 240.0));
        assert_eq!(g.screen_height(&Transform3D::default(), &camera), before);
    }

    #[test]
    fn distances_and_heights_convert_both_ways() {
        let fov = 50f32.to_radians();
        for d in [1.0, 7.5, 120.0] {
            let h = LodGroup::screen_height_at(3.0, d, fov);
            assert!((LodGroup::distance_at(3.0, h, fov) - d).abs() < d * 1e-4);
        }
        let level = LodLevel::at_distance(40.0, 8.0, fov, vec![]);
        assert!((LodGroup::distance_at(8.0, level.screen_height, fov) - 40.0).abs() < 0.01);
    }

    #[test]
    fn crossfade_runs_over_the_band_above_the_threshold() {
        // Level 0 is 0.5 and up; level 1 starts at 0.2 and its previous
        // threshold is 0.5. Width 0.5 of that 0.3: the band is 0.2 to 0.35.
        let g = group(0.5);
        let s = LodSettings::default();
        let blend = |h| match g.pick(h, &s) {
            LodPick::Fade { from, to, blend } => Some((from, to, blend)),
            _ => None,
        };
        // Level 1 fades into level 2 from 0.2 to 0.35.
        assert_eq!(blend(0.36), None);
        let (from, to, b) = blend(0.3).unwrap();
        assert_eq!((from, to), (1, Some(2)));
        assert!((b - (1.0 - 0.1 / 0.15)).abs() < 1e-5, "{b}");
        let (_, _, near_top) = blend(0.348).unwrap();
        assert!(near_top < 0.02);
        let (_, _, near_bottom) = blend(0.2015).unwrap();
        assert!(near_bottom > 0.98);
        // The blend never goes backwards as the group shrinks.
        let mut last = 0.0;
        for k in 0..100 {
            let h = 0.35 - 0.15 * k as f32 / 100.0;
            if let Some((_, _, b)) = blend(h) {
                assert!(b >= last, "{h}");
                last = b;
            }
        }
        // Below the band the next level is alone (past its own band).
        assert_eq!(g.pick(0.19, &s), LodPick::Level(2));
    }

    #[test]
    fn the_first_level_fades_within_the_top_of_the_screen() {
        let g = group(0.4);
        let s = LodSettings::default();
        // Level 0: 0.5 to 1.0, band 0.5 to 0.7.
        assert_eq!(g.pick(0.75, &s), LodPick::Level(0));
        let LodPick::Fade { from, to, blend } = g.pick(0.6, &s) else {
            panic!("fading");
        };
        assert_eq!((from, to), (0, Some(1)));
        assert!((blend - 0.5).abs() < 1e-5);
    }

    #[test]
    fn the_last_level_fades_out_to_nothing() {
        let g = group(0.5);
        let s = LodSettings::default();
        // Level 2: 0.05 up to 0.2, band 0.05 to 0.125.
        let LodPick::Fade { from, to, blend } = g.pick(0.08, &s) else {
            panic!("fading");
        };
        assert_eq!((from, to), (2, None));
        assert!(blend > 0.0 && blend < 1.0);
        assert_eq!(g.pick(0.049, &s), LodPick::Culled);
    }

    #[test]
    fn crossfade_off_switches_at_once() {
        let g = group(0.5);
        let s = LodSettings {
            crossfade: false,
            ..Default::default()
        };
        for k in 0..200 {
            let h = k as f32 / 150.0;
            assert!(!matches!(g.pick(h, &s), LodPick::Fade { .. }), "{h}");
        }
    }

    #[test]
    fn a_fade_draws_both_levels_with_complementary_dither() {
        let g = group(0.5);
        let LodPick::Fade { from, to, blend } = g.pick(0.3, &LodSettings::default()) else {
            panic!("fading");
        };
        let [a, b] = g.drawn(LodPick::Fade { from, to, blend });
        let (ia, _, fa) = a.unwrap();
        let (ib, _, fb) = b.unwrap();
        assert_eq!((ia, ib), (from, to.unwrap()));
        assert!(fa < 0.0 && fb > 0.0);
        // The outgoing level keeps what the incoming one doesn't.
        assert!((fa.abs() + fb - 1.0).abs() < 1e-6);
        assert_eq!(g.drawn(LodPick::Level(1))[0].unwrap().2, 0.0);
        assert!(g.drawn(LodPick::Culled)[0].is_none());
    }

    #[test]
    fn hysteresis_holds_a_level_near_its_threshold() {
        let g = group(0.0);
        let s = LodSettings::default();
        // Level 1 is 0.2 to 0.5. Held with a 10% band, it stays from 0.18
        // up to 0.55.
        let held = |h: f32, prev: LodPick| g.pick_held(h, &s, 0.1, prev);
        assert_eq!(held(0.52, LodPick::Level(1)), LodPick::Level(1));
        assert_eq!(held(0.56, LodPick::Level(1)), LodPick::Level(0));
        assert_eq!(held(0.19, LodPick::Level(1)), LodPick::Level(1));
        assert_eq!(held(0.17, LodPick::Level(1)), LodPick::Level(2));
        // Coming from level 0 the other way: it stays down to 0.45.
        assert_eq!(held(0.46, LodPick::Level(0)), LodPick::Level(0));
        assert_eq!(held(0.44, LodPick::Level(0)), LodPick::Level(1));
        // Culled stays culled until 10% above the last threshold.
        assert_eq!(held(0.054, LodPick::Culled), LodPick::Culled);
        assert_eq!(held(0.056, LodPick::Culled), LodPick::Level(2));
        // A jump far past the band switches at once.
        assert_eq!(held(0.9, LodPick::Level(2)), LodPick::Level(0));
        // No band: the plain pick.
        assert_eq!(g.pick_held(0.52, &s, 0.0, LodPick::Level(1)), LodPick::Level(0));
    }

    #[test]
    fn hysteresis_stops_a_camera_wobbling_at_a_threshold_from_switching() {
        let g = group(0.0);
        let s = LodSettings::default();
        let (mut plain, mut held) = (0, 0);
        let (mut last_plain, mut last_held) = (g.pick(0.5, &s), g.pick(0.5, &s));
        for frame in 0..100 {
            // 0.5 plus or minus 1%: across level 0's threshold every frame.
            let h = 0.5 * (1.0 + 0.01 * if frame % 2 == 0 { 1.0 } else { -1.0 });
            let p = g.pick(h, &s);
            plain += usize::from(p != last_plain);
            last_plain = p;
            let p = g.pick_held(h, &s, 0.05, last_held);
            held += usize::from(p != last_held);
            last_held = p;
        }
        assert!(plain >= 99, "without hysteresis it switches every frame ({plain})");
        assert!(held <= 1, "with it, at most once ({held})");
    }

    #[test]
    fn hysteresis_lets_crossfades_through_and_follows_the_bias() {
        let g = group(0.5);
        let s = LodSettings::default();
        // 0.3 is in level 1's crossfade into level 2 (0.2 to 0.35).
        assert!(matches!(
            g.pick_held(0.3, &s, 0.5, LodPick::Level(0)),
            LodPick::Fade { from: 1, .. }
        ));
        // The bias scales the height before the held range is checked.
        let biased = LodSettings { bias: 2.0, ..s };
        let g = group(0.0);
        assert_eq!(g.pick_held(0.27, &biased, 0.1, LodPick::Level(1)), LodPick::Level(1));
        assert_eq!(g.pick_held(0.29, &biased, 0.1, LodPick::Level(1)), LodPick::Level(0));
        // A stale pick from a group that had more levels is ignored.
        assert_eq!(g.pick_held(0.3, &s, 0.1, LodPick::Level(7)), LodPick::Level(1));
    }

    #[test]
    fn moving_away_never_goes_back_to_a_finer_level() {
        // Dollying out in small steps: the picks only ever move towards
        // cheaper levels, through each crossfade, and the group is only
        // culled past the last threshold (no missing frames on the way).
        for fade in [0.0, 0.3, 1.0] {
            let g = group(fade);
            let s = LodSettings::default();
            let rank = |p: LodPick| match p {
                LodPick::Level(i) => 2 * i,
                LodPick::Fade { from, .. } => 2 * from + 1,
                LodPick::Culled => 2 * g.levels.len(),
            };
            // (rank, blend): never going down as the group shrinks.
            let mut last = (0, 0.0);
            for step in 0..4000 {
                let h = 1.2 * (1.0 - step as f32 / 4000.0);
                let p = g.pick(h, &s);
                let now = match p {
                    LodPick::Fade { blend, .. } => (rank(p), blend),
                    _ => (rank(p), 0.0),
                };
                assert!(now >= last, "fade {fade}: {p:?} after {last:?} at {h}");
                last = now;
                // (Within a blend of 1/128 of its end, a fade to nothing
                // counts as done.)
                if h < 0.05 {
                    assert_eq!(p, LodPick::Culled, "fade {fade}: past the last level at {h}");
                } else if h > 0.052 {
                    assert_ne!(p, LodPick::Culled, "fade {fade}: culled too soon at {h}");
                }
            }
        }
    }

    #[test]
    fn turning_the_camera_does_not_change_the_level() {
        let g = group(0.3);
        let s = LodSettings::default();
        let t = Transform3D::at(Vec3::new(3.0, 0.0, -20.0));
        let mut camera = Camera3D::new(glam::Vec2::new(1280.0, 720.0));
        camera.position = Vec3::new(0.0, 1.0, 0.0);
        camera.look_at(t.position);
        let ahead = g.pick_for(&t, &camera, &s);
        for target in [
            Vec3::new(50.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 50.0),
            Vec3::new(0.0, -40.0, -1.0),
        ] {
            camera.look_at(target);
            assert_eq!(g.pick_for(&t, &camera, &s), ahead, "looking at {target}");
        }
    }

    /// The level whose band holds `distance`, among `levels` (one renderer
    /// each, meshes numbered by level), or `None` past them all.
    fn band_level(levels: &[InstanceLevel], distance: f32) -> Option<u32> {
        let mut found = levels
            .iter()
            .filter(|l| l.camera && distance >= l.min_distance && distance < l.max_distance)
            .map(|l| l.mesh.id);
        let first = found.next();
        assert!(found.next().is_none(), "bands overlap at {distance}");
        first
    }

    fn numbered(thresholds: &[f32]) -> LodGroup {
        LodGroup::new(
            2.0,
            thresholds
                .iter()
                .enumerate()
                .map(|(i, &h)| LodLevel::new(h, vec![LodRenderer::new(Mesh { id: i as u32 })]))
                .collect(),
        )
    }

    #[test]
    fn instance_levels_switch_at_the_distances_pick_switches_at() {
        let fov = 50f32.to_radians();
        for (scale, bias) in [(1.0, 1.0), (3.0, 1.0), (1.0, 0.5), (0.4, 2.0)] {
            let g = numbered(&[0.4, 0.1, 0.02]);
            let settings = LodSettings { bias, crossfade: false };
            let levels = g.instance_levels(scale, fov, &settings).unwrap();
            assert_eq!(levels.len(), 3);
            // Away from the switch distances (a hair either side of them
            // rounds either way), every distance gets the picked level.
            for k in 1..4000 {
                let distance = k as f32 * 0.05 * scale;
                let height = LodGroup::screen_height_at(2.0 * scale, distance, fov);
                let pick = g.pick(height, &settings);
                let near_switch = levels
                    .iter()
                    .any(|l| (distance - l.max_distance).abs() <= 1e-4 * l.max_distance);
                if near_switch {
                    continue;
                }
                let expected = match pick {
                    LodPick::Level(i) => Some(i as u32),
                    LodPick::Culled => None,
                    LodPick::Fade { .. } => panic!("crossfade is off"),
                };
                assert_eq!(
                    band_level(&levels, distance),
                    expected,
                    "scale {scale}, bias {bias}, {distance} m"
                );
            }
        }
    }

    #[test]
    fn instance_levels_follow_the_shadow_setting_of_each_level() {
        let caster = LodRenderer::new(Mesh { id: 9 });
        let g = LodGroup::new(
            1.0,
            vec![
                LodLevel::new(0.3, vec![LodRenderer::new(Mesh { id: 0 })]),
                LodLevel::new(0.1, vec![LodRenderer::new(Mesh { id: 1 })])
                    .with_shadow(LodShadow::Casters(vec![caster])),
                LodLevel::new(0.05, vec![LodRenderer::new(Mesh { id: 2 })]).with_shadow(LodShadow::Casters(vec![])),
            ],
        );
        let levels = g.instance_levels(1.0, 1.0, &LodSettings::default()).unwrap();
        let flags: Vec<_> = levels.iter().map(|l| (l.mesh.id, l.camera, l.shadows)).collect();
        assert_eq!(
            flags,
            [(0, true, true), (1, true, false), (9, false, true), (2, true, false)]
        );
        // A level's stand-in casts over exactly the level's band.
        assert_eq!(
            (levels[1].min_distance, levels[1].max_distance),
            (levels[2].min_distance, levels[2].max_distance)
        );
    }

    #[test]
    fn instance_levels_skip_levels_that_are_never_picked_and_ignore_fades() {
        // The middle threshold is above the first: never picked.
        let mut g = numbered(&[0.2, 0.3, 0.05]);
        g.levels[0].fade_width = 0.5;
        let levels = g.instance_levels(1.0, 1.0, &LodSettings::default()).unwrap();
        let ids: Vec<_> = levels.iter().map(|l| l.mesh.id).collect();
        assert_eq!(ids, [0, 2]);
        assert_eq!(levels[0].max_distance, levels[1].min_distance);
        // A last threshold of 0 never culls.
        let levels = numbered(&[0.2, 0.0])
            .instance_levels(1.0, 1.0, &LodSettings::default())
            .unwrap();
        assert_eq!(levels[1].max_distance, f32::INFINITY);
    }

    #[test]
    fn groups_a_batch_cannot_draw_have_no_instance_levels() {
        let settings = LodSettings::default();
        let plain = numbered(&[0.2]);
        assert!(plain.instance_levels(1.0, 1.0, &settings).is_some());
        assert!(
            plain
                .clone()
                .with_center(Vec3::Y)
                .instance_levels(1.0, 1.0, &settings)
                .is_none()
        );
        let mut offset = plain.clone();
        offset.levels[0].renderers[0].offset = Transform3D::at(Vec3::X);
        assert!(offset.instance_levels(1.0, 1.0, &settings).is_none());
        let mut tinted = plain.clone();
        tinted.levels[0].renderers[0].color = Color::rgb(1.0, 0.0, 0.0);
        assert!(tinted.instance_levels(1.0, 1.0, &settings).is_none());
        assert!(numbered(&[]).instance_levels(1.0, 1.0, &settings).is_none());
    }

    #[test]
    fn instance_levels_take_a_bias_of_zero_or_less_as_pick_does() {
        let fov = 1.0;
        for bias in [0.0, -1.0, f32::NAN] {
            let settings = LodSettings { bias, crossfade: false };
            // No threshold of 0: `pick` culls at every distance, and a
            // batch would draw nothing.
            let culled = numbered(&[0.2, 0.05]);
            assert!(culled.instance_levels(1.0, fov, &settings).is_none(), "bias {bias}");
            // A last level from 0: `pick` draws it at every distance, and
            // so does the batch.
            let always = numbered(&[0.2, 0.05, 0.0]);
            let levels = always
                .instance_levels(1.0, fov, &settings)
                .expect("the last level is drawn");
            assert_eq!(levels.len(), 1, "bias {bias}");
            assert_eq!(
                (levels[0].mesh.id, levels[0].min_distance, levels[0].max_distance),
                (2, 0.0, f32::INFINITY),
                "bias {bias}"
            );
            for k in 0..200 {
                let distance = k as f32 * 0.5;
                let height = LodGroup::screen_height_at(2.0, distance, fov);
                assert_eq!(
                    culled.pick(height, &settings),
                    LodPick::Culled,
                    "bias {bias}, {distance} m"
                );
                assert_eq!(
                    always.pick(height, &settings),
                    LodPick::Level(2),
                    "bias {bias}, {distance} m"
                );
                assert_eq!(band_level(&levels, distance), Some(2), "bias {bias}, {distance} m");
            }
        }
    }
}
