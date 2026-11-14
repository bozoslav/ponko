//! 2D lights, shadows and normal-mapped sprites: the components behind
//! `d2::{PointLight, SpotLight, DirectionalLight, Occluder}` (their one
//! path; the module docs of [`d2`](crate::d2) describe them) and the
//! ambient light and normal maps on [`Graphics`].

use bytemuck::{Pod, Zeroable};
use glam::Vec2;
use ponko_ecs::{Component, World};

use crate::camera::Rect;
use crate::color::Color;
use crate::ecs::Transform2D;
use crate::gfx::Graphics;
use crate::texture::Texture;
use crate::tilemap::TileMap;

/// A light shining from its entity's [`d2::Transform`](crate::d2::Transform) position in every
/// direction, out to `radius` world units. Rotation and scale are ignored.
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::d2::PointLight;
/// # fn f(ctx: &mut Ctx) {
/// let torch = PointLight { color: Color::rgb(1.0, 0.6, 0.3), radius: 120.0, ..Default::default() };
/// ctx.world.spawn((ponko::d2::Transform::at(vec2(64.0, 64.0)), torch));
/// # }
/// ```
#[repr(C)] // shared with C/C++ (`PonkoPointLight2D`)
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[serde(default)]
pub struct PointLight2D {
    /// sRGB color (alpha ignored). Default white.
    pub color: Color,
    /// Brightness multiplier. Default 1. The lit scene saturates at twice
    /// a pixel's own color, so lights can brighten as well as reveal.
    pub intensity: f32,
    /// Reach in world units: no light beyond it. Default 200.
    pub radius: f32,
    /// How fast the light fades toward `radius`: the light is
    /// `(1 - distance / radius) ^ falloff`. 1 fades evenly, 2 (the default)
    /// keeps a bright core.
    pub falloff: f32,
    /// How high above the scene the light hangs, in world units. Only
    /// normal-mapped sprites and tiles see it: low lights graze their
    /// bumps, high lights light them flat. Default 64.
    pub height: f32,
    /// Whether [`Occluder`](crate::d2::Occluder)s in reach cast shadows from this light.
    /// Default false.
    pub shadows: bool,
}

impl Default for PointLight2D {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 1.0,
            radius: 200.0,
            falloff: 2.0,
            height: 64.0,
            shadows: false,
        }
    }
}

/// A cone of light from its entity's [`d2::Transform`](crate::d2::Transform) position, pointing
/// along its rotation (rotation 0 points right, along +x; positive turns
/// clockwise on screen, as sprites do). Fades like a [`PointLight`](crate::d2::PointLight).
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::d2::SpotLight;
/// # fn f(ctx: &mut Ctx) {
/// let lamp = SpotLight { angle: 0.4, radius: 300.0, shadows: true, ..Default::default() };
/// let mut at = ponko::d2::Transform::at(vec2(100.0, 50.0));
/// at.rotation = std::f32::consts::FRAC_PI_2; // shining down
/// ctx.world.spawn((at, lamp));
/// # }
/// ```
#[repr(C)] // shared with C/C++ (`PonkoSpotLight2D`)
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[serde(default)]
pub struct SpotLight2D {
    /// sRGB color (alpha ignored). Default white.
    pub color: Color,
    /// Brightness multiplier. Default 1.
    pub intensity: f32,
    /// Reach in world units. Default 300.
    pub radius: f32,
    /// As [`PointLight::falloff`](crate::d2::PointLight::falloff). Default 2.
    pub falloff: f32,
    /// As [`PointLight::height`](crate::d2::PointLight::height). Default 64.
    pub height: f32,
    /// Half the cone's width, in radians (0 to π). Default π/6 (a 60°
    /// cone).
    pub angle: f32,
    /// How much of the cone's edge fades out instead of ending sharply,
    /// 0 (hard) to 1 (fades from the center line). Default 0.2.
    pub softness: f32,
    /// Whether [`Occluder`](crate::d2::Occluder)s in reach cast shadows. Default false.
    pub shadows: bool,
}

impl Default for SpotLight2D {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 1.0,
            radius: 300.0,
            falloff: 2.0,
            height: 64.0,
            angle: std::f32::consts::FRAC_PI_6,
            softness: 0.2,
            shadows: false,
        }
    }
}

/// Light from far away falling evenly on the whole scene, such as sun- or
/// moonlight, travelling along `direction`. It needs no transform. Its
/// shadows are endless strips behind each occluder.
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::d2::DirectionalLight;
/// # fn f(ctx: &mut Ctx) {
/// ctx.world.spawn((DirectionalLight {
///     color: Color::rgb(0.6, 0.7, 1.0),
///     intensity: 0.5,
///     direction: vec2(1.0, 1.0), // toward the bottom right
///     ..Default::default()
/// },));
/// # }
/// ```
#[repr(C)] // shared with C/C++ (`PonkoDirectionalLight2D`)
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[serde(default)]
pub struct DirectionalLight2D {
    /// sRGB color (alpha ignored). Default white.
    pub color: Color,
    /// Brightness multiplier. Default 1.
    pub intensity: f32,
    /// Where the light travels, in world units (any length; y is down).
    /// Default `(0, 1)`: from the top of the screen downward.
    pub direction: Vec2,
    /// The light's angle above the scene in radians, for normal maps: near
    /// 0 grazes, π/2 shines straight down at the screen. Default π/4.
    pub elevation: f32,
    /// Whether [`Occluder`](crate::d2::Occluder)s cast shadows. Default false.
    pub shadows: bool,
}

impl Default for DirectionalLight2D {
    fn default() -> Self {
        Self {
            color: Color::WHITE,
            intensity: 1.0,
            direction: Vec2::Y,
            elevation: std::f32::consts::FRAC_PI_4,
            shadows: false,
        }
    }
}

/// A shape that blocks 2D light: lights with `shadows` on cast a shadow
/// behind it. Put it on an entity with a [`d2::Transform`](crate::d2::Transform) (position,
/// rotation and scale apply); it can share the entity with the sprite it
/// shades.
///
/// The shape itself stays lit: only its edges facing away from a light
/// cast, so a wall is lit on the near side and throws its shadow on the
/// far side. Tile maps make one from their solid tiles with
/// [`TileMap::occluder`].
///
/// ```
/// # use ponko::prelude::*;
/// # use ponko::d2::Occluder;
/// let crate_box = Occluder::rect(vec2(32.0, 32.0));
/// assert_eq!(crate_box.edges().len(), 4);
/// let pillar = Occluder::circle(10.0);
/// let ramp = Occluder::polygon(&[vec2(0.0, 0.0), vec2(40.0, 20.0), vec2(0.0, 20.0)]);
/// let fence = Occluder::line(vec2(-50.0, 0.0), vec2(50.0, 0.0)); // casts on both sides
/// # let _ = (pillar, ramp, fence);
/// ```
///
/// Saved and reflected as `{"edges": [[[x, y], [x, y]], ...]}`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Component)]
#[serde(from = "OccluderEdges")]
pub struct Occluder2D {
    /// Edges in the entity's local space, each `[from, to]` with the
    /// outside on its left on screen (y down): outward normal
    /// `(d.y, -d.x)` for `d = to - from`.
    edges: Vec<[Vec2; 2]>,
    /// The farthest any edge point is from the local origin, worked out
    /// from the edges (never saved, so an edited occluder can't keep a
    /// stale one).
    #[serde(skip)]
    reach: f32,
}

/// What an [`Occluder`](crate::d2::Occluder) loads from.
#[derive(serde::Deserialize)]
struct OccluderEdges {
    edges: Vec<[Vec2; 2]>,
}

impl From<OccluderEdges> for Occluder2D {
    fn from(saved: OccluderEdges) -> Self {
        Self::from_edges(saved.edges)
    }
}

impl Occluder2D {
    /// Sides of [`circle`](Self::circle)s.
    const CIRCLE_SIDES: usize = 16;

    /// A rectangle `size` wide and tall, centered on the entity.
    #[must_use]
    pub fn rect(size: Vec2) -> Self {
        let h = size.abs() * 0.5;
        Self::polygon(&[
            Vec2::new(-h.x, -h.y),
            Vec2::new(h.x, -h.y),
            Vec2::new(h.x, h.y),
            Vec2::new(-h.x, h.y),
        ])
    }

    /// A circle of `radius`, centered on the entity (a 16-sided polygon).
    #[must_use]
    pub fn circle(radius: f32) -> Self {
        let r = radius.abs();
        let points: Vec<Vec2> = (0..Self::CIRCLE_SIDES)
            .map(|i| Vec2::from_angle(i as f32 * std::f32::consts::TAU / Self::CIRCLE_SIDES as f32) * r)
            .collect();
        Self::polygon(&points)
    }

    /// A closed polygon through `points`, relative to the entity, in either
    /// winding order. It may be concave; it must not cross itself.
    #[must_use]
    pub fn polygon(points: &[Vec2]) -> Self {
        let n = points.len();
        if n < 2 {
            return Self::from_edges(Vec::new());
        }
        // Positive shoelace sum: clockwise on a y-down screen, so the
        // outside is on each edge's left.
        let area: f32 = (0..n).map(|i| points[i].perp_dot(points[(i + 1) % n])).sum();
        let edges = (0..n)
            .map(|i| {
                let (a, b) = (points[i], points[(i + 1) % n]);
                if area >= 0.0 { [a, b] } else { [b, a] }
            })
            .filter(|[a, b]| a != b)
            .collect();
        Self::from_edges(edges)
    }

    /// A thin wall from `a` to `b` (relative to the entity), casting
    /// shadows toward both sides.
    #[must_use]
    pub fn line(a: Vec2, b: Vec2) -> Self {
        Self::from_edges(vec![[a, b], [b, a]])
    }

    /// The shape's edges in the entity's local space, each `[from, to]` with
    /// the outside on its left on screen (the outward normal is
    /// `(d.y, -d.x)` for `d = to - from`). An edge casts a shadow from a
    /// light behind it.
    #[must_use]
    pub fn edges(&self) -> &[[Vec2; 2]] {
        &self.edges
    }

    fn from_edges(edges: Vec<[Vec2; 2]>) -> Self {
        let reach = edges.iter().flatten().map(|p| p.length()).fold(0.0, f32::max);
        Self { edges, reach }
    }
}

impl TileMap {
    /// An [`Occluder`](crate::d2::Occluder) outlining the tiles `solid` picks (by tile value):
    /// one edge along every side where a solid tile meets an empty one or
    /// the map's border, merged into long runs. Put it on the map's entity,
    /// so lights with shadows cast them from the walls; the walls
    /// themselves stay lit. Like [`solid_rects`](Self::solid_rects), it is
    /// relative to the map's top-left corner and doesn't follow
    /// [`parallax`](Self::parallax). Make it again after changing the
    /// walls.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, map: ponko::tilemap::TileMap) {
    /// const WALL: u16 = 3;
    /// let shadows = map.occluder(|tile| tile == WALL);
    /// ctx.world.spawn((ponko::d2::Transform::at(vec2(0.0, 0.0)), map, shadows));
    /// # }
    /// ```
    #[must_use]
    pub fn occluder(&self, solid: impl Fn(u16) -> bool) -> Occluder2D {
        let (w, h) = (self.size.x as i64, self.size.y as i64);
        let is = |x: i64, y: i64| {
            x >= 0 && y >= 0 && x < w && y < h && {
                let t = self.tiles[(y * w + x) as usize];
                t != TileMap::EMPTY && solid(t)
            }
        };
        let ts = self.tile_size;
        let at = |x: i64, y: i64| Vec2::new(x as f32 * ts.x, y as f32 * ts.y);
        let mut edges = Vec::new();
        // Horizontal sides: tops run left to right, bottoms right to left
        // (outside on the left of each edge).
        for y in 0..=h {
            let mut run: Option<(i64, bool)> = None;
            for x in 0..=w {
                // At the row boundary `y`: the cell below solid and above
                // empty is a top side; the reverse a bottom side.
                let side = if x < w {
                    match (is(x, y - 1), is(x, y)) {
                        (false, true) => Some(true),
                        (true, false) => Some(false),
                        _ => None,
                    }
                } else {
                    None
                };
                if run.map(|(_, top)| Some(top)) != Some(side) {
                    if let Some((start, top)) = run.take() {
                        let (a, b) = (at(start, y), at(x, y));
                        edges.push(if top { [a, b] } else { [b, a] });
                    }
                    run = side.map(|top| (x, top));
                }
            }
        }
        // Vertical sides: left sides run upward, right sides downward.
        for x in 0..=w {
            let mut run: Option<(i64, bool)> = None;
            for y in 0..=h {
                let side = if y < h {
                    match (is(x - 1, y), is(x, y)) {
                        (false, true) => Some(true),
                        (true, false) => Some(false),
                        _ => None,
                    }
                } else {
                    None
                };
                if run.map(|(_, left)| Some(left)) != Some(side) {
                    if let Some((start, left)) = run.take() {
                        let (a, b) = (at(x, start), at(x, y));
                        edges.push(if left { [b, a] } else { [a, b] });
                    }
                    run = side.map(|left| (y, left));
                }
            }
        }
        Occluder2D::from_edges(edges)
    }
}

/// The game's 2D lighting settings (`ctx.gfx.lights2d`).
#[derive(Default)]
pub(crate) struct Lighting2D {
    pub(crate) ambient: Option<Color>,
    /// The normal map of each texture, by texture id.
    pub(crate) normal_maps: Vec<Option<Texture>>,
    /// What this frame draws, filled just before it is drawn.
    pub(crate) frame: LightFrame2D,
}

impl Graphics {
    /// Sets the light every place gets before the 2D lights add theirs
    /// (sRGB, alpha ignored), or `None` (the default). A color here turns
    /// 2D lighting on even without lights, which darkens the whole world
    /// layer to it: night, caves, a dim room. Without one, the ambient is
    /// black whenever there are 2D lights, so only lit places show.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     ctx.gfx.set_ambient_light2d(Some(Color::rgb(0.2, 0.2, 0.3)));
    ///     assert_eq!(ctx.gfx.ambient_light2d(), Some(Color::rgb(0.2, 0.2, 0.3)));
    /// });
    /// ```
    pub fn set_ambient_light2d(&mut self, ambient: Option<Color>) {
        self.lights2d.ambient = ambient;
    }

    /// The ambient light set with
    /// [`set_ambient_light2d`](Self::set_ambient_light2d).
    #[must_use]
    pub fn ambient_light2d(&self) -> Option<Color> {
        self.lights2d.ambient
    }

    /// Gives every sprite, tile and particle drawn with `texture` a normal
    /// map (`None` takes it away), so 2D lights shade its bumps and edges.
    /// The normal map lines up with `texture` texel for texel (same size,
    /// same atlas layout), tangent-space with green pointing up the image,
    /// as most tools export it. Rotated and flipped sprites shade
    /// correctly. Sprites without one are lit flat.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn setup(ctx: &mut Ctx) {
    /// let bricks = ctx.gfx.load_texture("bricks.png");
    /// let bumps = ctx.gfx.load_texture("bricks_normal.png");
    /// ctx.gfx.set_normal_map(bricks, Some(bumps));
    /// # }
    /// ```
    pub fn set_normal_map(&mut self, texture: Texture, normal_map: Option<Texture>) {
        let index = texture.id as usize;
        let maps = &mut self.lights2d.normal_maps;
        if maps.len() <= index {
            if normal_map.is_none() {
                return;
            }
            maps.resize(index + 1, None);
        }
        maps[index] = normal_map;
    }

    /// The normal map given to `texture` with
    /// [`set_normal_map`](Self::set_normal_map).
    #[must_use]
    pub fn normal_map(&self, texture: Texture) -> Option<Texture> {
        self.lights2d.normal_maps.get(texture.id as usize).copied().flatten()
    }
}

/// One light as the light pass reads it (an instance): four `vec4`s.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct LightInstance {
    /// Position (world), radius (0: directional) and height (normal maps).
    pub place: [f32; 4],
    /// Linear color × intensity × ½ (the light map holds half), and falloff.
    pub color: [f32; 4],
    /// Unit direction (spot, directional) and the cone's cosines: outer
    /// edge, inner edge (-2 and -1 for no cone).
    pub cone: [f32; 4],
    /// Directional only: the unit light vector toward the light, `xyz`.
    pub toward: [f32; 4],
}

/// What 2D lighting draws this frame, gathered from the world.
#[derive(Debug, Default)]
pub(crate) struct LightFrame2D {
    /// Lighting is on: there are 2D lights or an ambient light.
    pub on: bool,
    /// The ambient light, linear and halved, as the light map is cleared.
    pub ambient: [f32; 4],
    /// The lights in view: `free` without shadows first, then those with.
    pub lights: Vec<LightInstance>,
    /// How many of `lights` come first and cast no shadows.
    pub free: u32,
    /// For each shadowed light (`lights[free + i]`), its shadow volumes'
    /// triangles in `vertices`.
    pub shadows: Vec<std::ops::Range<u32>>,
    /// Shadow volume triangles, world units.
    pub vertices: Vec<[f32; 2]>,
    /// Scratch: occluder edges in world space, and the shadowed lights
    /// before they go after the free ones.
    edges: Vec<[Vec2; 2]>,
    shadowed: Vec<LightInstance>,
    casting: Vec<LightInstance>,
}

/// How far point and spot light shadows reach, in radii: the volume's far
/// side is two corners out at this distance, so it stays outside the light.
const SHADOW_REACH: f32 = 2.0;
/// How far directional shadows reach, in world units: past any screen.
const ENDLESS: f32 = 1.0e6;

fn linear(c: f32) -> f32 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// `color` in linear light × `scale`, as the light map adds it.
fn light_color(color: Color, scale: f32) -> [f32; 3] {
    [linear(color.r), linear(color.g), linear(color.b)].map(|c| c * scale)
}

/// Whether the circle at `center` of `radius` overlaps `view`.
fn circle_in(view: Rect, center: Vec2, radius: f32) -> bool {
    let nearest = center.clamp(view.min, view.max);
    nearest.distance_squared(center) <= radius * radius
}

/// The distance from `p` to the segment `a`-`b`.
fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let d = b - a;
    let t = ((p - a).dot(d) / d.length_squared().max(f32::MIN_POSITIVE)).clamp(0.0, 1.0);
    p.distance(a + d * t)
}

impl LightFrame2D {
    /// Gathers this frame's lights in `view` and their shadows from
    /// `world`, with `ambient` (sRGB). Reuses its buffers: steady frames
    /// don't allocate. Off, and nearly free, when the world has no 2D
    /// lights and there is no ambient light.
    pub(crate) fn collect(&mut self, world: &mut World, view: Rect, ambient: Option<Color>) {
        self.lights.clear();
        self.shadows.clear();
        self.vertices.clear();
        self.edges.clear();
        self.shadowed.clear();
        self.free = 0;
        let any = world.query::<(&Transform2D, &PointLight2D)>().count()
            + world.query::<(&Transform2D, &SpotLight2D)>().count()
            + world.query::<&DirectionalLight2D>().count();
        self.on = any > 0 || ambient.is_some();
        if !self.on {
            return;
        }
        let a = light_color(ambient.unwrap_or(Color::BLACK), 0.5);
        self.ambient = [a[0], a[1], a[2], 1.0];
        let (lights, shadowed) = (&mut self.lights, &mut self.shadowed);
        let mut push = |light: LightInstance, shadows: bool| {
            if shadows {
                shadowed.push(light)
            } else {
                lights.push(light)
            }
        };
        world.query::<(&Transform2D, &PointLight2D)>().for_each(|(t, l)| {
            if !(l.intensity > 0.0 && l.radius > 0.0) || !circle_in(view, t.position, l.radius) {
                return;
            }
            let c = light_color(l.color, l.intensity * 0.5);
            push(
                LightInstance {
                    place: [t.position.x, t.position.y, l.radius, l.height.max(0.0)],
                    color: [c[0], c[1], c[2], l.falloff.max(0.0)],
                    cone: [1.0, 0.0, -2.0, -1.0],
                    toward: [0.0; 4],
                },
                l.shadows,
            );
        });
        world.query::<(&Transform2D, &SpotLight2D)>().for_each(|(t, l)| {
            if !(l.intensity > 0.0 && l.radius > 0.0 && l.angle > 0.0) || !circle_in(view, t.position, l.radius) {
                return;
            }
            let c = light_color(l.color, l.intensity * 0.5);
            let dir = Vec2::from_angle(t.rotation);
            let outer = l.angle.min(std::f32::consts::PI);
            let inner = outer * (1.0 - l.softness.clamp(0.0, 1.0));
            // A hard edge still needs the inner cosine above the outer.
            let (cos_outer, cos_inner) = (outer.cos(), inner.cos().max(outer.cos() + 1e-4));
            push(
                LightInstance {
                    place: [t.position.x, t.position.y, l.radius, l.height.max(0.0)],
                    color: [c[0], c[1], c[2], l.falloff.max(0.0)],
                    cone: [dir.x, dir.y, cos_outer, cos_inner],
                    toward: [0.0; 4],
                },
                l.shadows,
            );
        });
        world.query::<&DirectionalLight2D>().for_each(|l| {
            let Some(dir) = l.direction.try_normalize() else {
                return;
            };
            if l.intensity <= 0.0 {
                return;
            }
            let c = light_color(l.color, l.intensity * 0.5);
            let (sin, cos) = l.elevation.clamp(0.0, std::f32::consts::FRAC_PI_2).sin_cos();
            let toward = (-dir * cos).extend(sin);
            push(
                LightInstance {
                    place: [0.0, 0.0, 0.0, 0.0],
                    color: [c[0], c[1], c[2], 0.0],
                    cone: [dir.x, dir.y, -2.0, -1.0],
                    toward: [toward.x, toward.y, toward.z, 0.0],
                },
                l.shadows,
            );
        });
        if !self.shadowed.is_empty() {
            self.collect_edges(world, view);
            self.build_shadows();
        }
        self.free = self.lights.len() as u32 - self.shadows.len() as u32;
    }

    /// The world-space edges of every occluder a shadowed light can reach.
    fn collect_edges(&mut self, world: &mut World, view: Rect) {
        let directional = self.shadowed.iter().any(|l| l.place[2] == 0.0);
        let shadowed = &self.shadowed;
        let edges = &mut self.edges;
        world.query::<(&Transform2D, &Occluder2D)>().for_each(|(t, o)| {
            let scale = t.scale;
            let reach = o.reach * scale.abs().max_element();
            let near = |l: &LightInstance| {
                let at = Vec2::new(l.place[0], l.place[1]);
                l.place[2] == 0.0 || at.distance(t.position) <= l.place[2] + reach
            };
            // Directional shadows reach the view from anywhere; others
            // only from occluders within a shadowed light's reach.
            let wanted = if directional {
                shadowed.iter().any(near) || circle_in(grow(view, ENDLESS), t.position, reach)
            } else {
                shadowed.iter().any(near)
            };
            if !wanted {
                return;
            }
            let (sin, cos) = t.rotation.sin_cos();
            let place = |p: Vec2| {
                let p = p * scale;
                t.position + Vec2::new(p.x * cos - p.y * sin, p.x * sin + p.y * cos)
            };
            // A mirror (one negative scale) turns the outside to the
            // right: swap the ends back.
            let mirrored = scale.x * scale.y < 0.0;
            edges.extend(o.edges.iter().map(|&[a, b]| {
                let (a, b) = (place(a), place(b));
                if mirrored { [b, a] } else { [a, b] }
            }));
        });
    }

    /// Each shadowed light's volumes, from the edges facing away from it.
    /// A light none of whose edges cast joins the free lights.
    fn build_shadows(&mut self) {
        let Self {
            lights,
            shadows,
            vertices,
            edges,
            shadowed,
            casting,
            ..
        } = self;
        casting.clear();
        for &light in shadowed.iter() {
            let start = vertices.len() as u32;
            let radius = light.place[2];
            if radius == 0.0 {
                let dir = Vec2::new(light.cone[0], light.cone[1]);
                let far = dir * ENDLESS;
                for &[a, b] in edges.iter() {
                    let d = b - a;
                    // Outward normal (d.y, -d.x) along the light: lit from inside.
                    if Vec2::new(d.y, -d.x).dot(dir) <= 0.0 {
                        continue;
                    }
                    vertices.extend([a, b, b + far, a, b + far, a + far].map(|p| p.to_array()));
                }
            } else {
                let at = Vec2::new(light.place[0], light.place[1]);
                let reach = radius * SHADOW_REACH;
                for &[a, b] in edges.iter() {
                    let d = b - a;
                    if Vec2::new(d.y, -d.x).dot(a - at) <= 0.0 || segment_distance(at, a, b) >= radius {
                        continue;
                    }
                    let (Some(da), Some(db)) = ((a - at).try_normalize(), (b - at).try_normalize()) else {
                        continue;
                    };
                    let mid = (da + db).try_normalize().unwrap_or(Vec2::new(da.y, -da.x));
                    let (fa, fb, fm) = (at + da * reach, at + db * reach, at + mid * reach);
                    vertices.extend([a, b, fb, a, fb, fm, a, fm, fa].map(|p| p.to_array()));
                }
            }
            let end = vertices.len() as u32;
            if end == start {
                lights.push(light);
            } else {
                casting.push(light);
                shadows.push(start..end);
            }
        }
        lights.extend_from_slice(casting);
    }
}

/// `r` grown by `by` on every side.
fn grow(r: Rect, by: f32) -> Rect {
    Rect {
        min: r.min - Vec2::splat(by),
        max: r.max + Vec2::splat(by),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Rect {
        Rect {
            min: Vec2::ZERO,
            max: Vec2::new(400.0, 300.0),
        }
    }

    /// Whether `p` lies inside any triangle of `tris`.
    fn shadowed(tris: &[[f32; 2]], p: Vec2) -> bool {
        tris.as_chunks::<3>().0.iter().any(|t| {
            let [a, b, c] = t.map(Vec2::from);
            let s = |u: Vec2, v: Vec2| (v - u).perp_dot(p - u);
            let (d1, d2, d3) = (s(a, b), s(b, c), s(c, a));
            let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
            let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
            !(neg && pos)
        })
    }

    fn frame(world: &mut World, ambient: Option<Color>) -> LightFrame2D {
        let mut f = LightFrame2D::default();
        f.collect(world, view(), ambient);
        f
    }

    #[test]
    fn a_world_without_lights_or_ambient_stays_unlit() {
        let mut world = World::new();
        world.spawn((Transform2D::at(Vec2::new(10.0, 10.0)), Occluder2D::rect(Vec2::ONE)));
        let f = frame(&mut world, None);
        assert!(!f.on);
        assert!(f.lights.is_empty() && f.vertices.is_empty());
        // An ambient light alone turns lighting on, with no lights to draw.
        let f = frame(&mut world, Some(Color::WHITE));
        assert!(f.on && f.lights.is_empty());
        assert_eq!(f.ambient, [0.5, 0.5, 0.5, 1.0]);
    }

    #[test]
    fn lights_out_of_view_or_switched_off_are_left_out_but_keep_lighting_on() {
        let mut world = World::new();
        let far = PointLight2D {
            radius: 50.0,
            ..Default::default()
        };
        world.spawn((Transform2D::at(Vec2::new(-100.0, 100.0)), far));
        let dark = PointLight2D {
            intensity: 0.0,
            ..Default::default()
        };
        world.spawn((Transform2D::at(Vec2::new(100.0, 100.0)), dark));
        let f = frame(&mut world, None);
        assert!(f.on, "lights exist, so the world is dark around them");
        assert!(f.lights.is_empty());
        assert_eq!(f.ambient, [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn a_box_shadows_the_far_side_from_a_point_light_and_stays_lit_itself() {
        let mut world = World::new();
        world.spawn((
            Transform2D::at(Vec2::new(100.0, 150.0)),
            PointLight2D {
                radius: 250.0,
                shadows: true,
                ..Default::default()
            },
        ));
        world.spawn((
            Transform2D::at(Vec2::new(200.0, 150.0)),
            Occluder2D::rect(Vec2::new(20.0, 40.0)),
        ));
        let f = frame(&mut world, None);
        assert_eq!((f.lights.len(), f.free, f.shadows.len()), (1, 0, 1));
        let tris = &f.vertices[f.shadows[0].start as usize..f.shadows[0].end as usize];
        assert!(shadowed(tris, Vec2::new(260.0, 150.0)), "behind the box");
        assert!(
            !shadowed(tris, Vec2::new(150.0, 150.0)),
            "between the light and the box"
        );
        assert!(!shadowed(tris, Vec2::new(195.0, 150.0)), "inside the box: lit");
        assert!(!shadowed(tris, Vec2::new(260.0, 60.0)), "beside the shadow");
        // The control: without `shadows`, the same light casts nothing.
        world.query::<&mut PointLight2D>().for_each(|l| l.shadows = false);
        let f = frame(&mut world, None);
        assert_eq!((f.lights.len(), f.free), (1, 1));
        assert!(f.vertices.is_empty());
    }

    #[test]
    fn rotated_and_mirrored_occluders_cast_from_their_far_side() {
        for scale in [Vec2::ONE, Vec2::new(-1.0, 1.0), Vec2::new(2.0, -0.5)] {
            let mut world = World::new();
            world.spawn((
                Transform2D::at(Vec2::new(100.0, 150.0)),
                PointLight2D {
                    radius: 300.0,
                    shadows: true,
                    ..Default::default()
                },
            ));
            let mut t = Transform2D::at(Vec2::new(200.0, 150.0));
            t.rotation = 0.7;
            t.scale = scale;
            world.spawn((
                t,
                Occluder2D::polygon(&[Vec2::new(-10.0, -10.0), Vec2::new(10.0, -10.0), Vec2::new(0.0, 12.0)]),
            ));
            let f = frame(&mut world, None);
            assert!(shadowed(&f.vertices, Vec2::new(280.0, 150.0)), "{scale}");
            assert!(
                !shadowed(&f.vertices, Vec2::new(200.0, 150.0)),
                "{scale}: the occluder stays lit"
            );
            assert!(!shadowed(&f.vertices, Vec2::new(150.0, 150.0)), "{scale}");
        }
    }

    #[test]
    fn a_light_with_no_occluder_in_reach_draws_without_a_shadow_pass() {
        let mut world = World::new();
        world.spawn((
            Transform2D::at(Vec2::new(50.0, 50.0)),
            PointLight2D {
                radius: 40.0,
                shadows: true,
                ..Default::default()
            },
        ));
        world.spawn((Transform2D::at(Vec2::new(300.0, 250.0)), Occluder2D::circle(10.0)));
        let f = frame(&mut world, None);
        assert_eq!((f.lights.len(), f.free, f.shadows.len()), (1, 1, 0));
    }

    #[test]
    fn directional_shadows_run_endlessly_along_the_light() {
        let mut world = World::new();
        world.spawn((DirectionalLight2D {
            direction: Vec2::new(1.0, 0.0),
            shadows: true,
            ..Default::default()
        },));
        world.spawn((
            Transform2D::at(Vec2::new(100.0, 150.0)),
            Occluder2D::rect(Vec2::new(10.0, 10.0)),
        ));
        let f = frame(&mut world, None);
        assert_eq!(f.shadows.len(), 1);
        assert!(shadowed(&f.vertices, Vec2::new(390.0, 150.0)));
        assert!(!shadowed(&f.vertices, Vec2::new(50.0, 150.0)), "before the box");
        assert!(!shadowed(&f.vertices, Vec2::new(390.0, 170.0)), "beside the strip");
        let toward = f.lights[0].toward;
        assert!(
            toward[0] < 0.0 && toward[2] > 0.0,
            "{toward:?}: back toward the light, up"
        );
    }

    #[test]
    fn spot_lights_carry_their_cone_and_turn_with_their_transform() {
        let mut world = World::new();
        let mut t = Transform2D::at(Vec2::new(100.0, 100.0));
        t.rotation = std::f32::consts::FRAC_PI_2;
        world.spawn((
            t,
            SpotLight2D {
                angle: 0.5,
                softness: 0.0,
                ..Default::default()
            },
        ));
        let f = frame(&mut world, None);
        let cone = f.lights[0].cone;
        assert!(
            cone[0].abs() < 1e-6 && (cone[1] - 1.0).abs() < 1e-6,
            "{cone:?}: points down"
        );
        assert!((cone[2] - 0.5f32.cos()).abs() < 1e-6 && cone[3] > cone[2]);
    }

    #[test]
    fn a_tile_room_outlines_its_walls_once_facing_out() {
        let tex = Texture::new(1, 16, 16);
        let mut map = TileMap::new(crate::sheet::SpriteSheet::new(tex, 16, 16), 4, 3);
        // A 2 x 1 block at (1, 1): six cells of map, one block of walls.
        map.set(1, 1, Some(0));
        map.set(2, 1, Some(0));
        let o = map.occluder(|_| true);
        assert_eq!(o.edges().len(), 4, "{:?}", o.edges());
        // Every edge's outward normal points away from the block's center.
        let center = Vec2::new(32.0, 24.0);
        for &[a, b] in o.edges() {
            let d = b - a;
            assert!(Vec2::new(d.y, -d.x).dot((a + b) * 0.5 - center) > 0.0, "{a} {b}");
        }
        let perimeter: f32 = o.edges().iter().map(|[a, b]| a.distance(*b)).sum();
        assert_eq!(perimeter, 2.0 * (32.0 + 16.0));
        assert!(map.occluder(|_| false).edges().is_empty());
    }

    #[test]
    fn a_loaded_occluder_works_out_its_own_reach() {
        let wall = Occluder2D::line(Vec2::new(-30.0, 40.0), Vec2::new(30.0, 40.0));
        let json = serde_json::to_value(&wall).unwrap();
        assert!(json.get("reach").is_none(), "derived, never saved: {json}");
        assert_eq!(serde_json::from_value::<Occluder2D>(json).unwrap(), wall);
        // An edited edge list gets a fresh reach, so its shadows aren't culled.
        let edited = serde_json::json!({"edges": [[[0.0, 0.0], [300.0, 400.0]]]});
        let far = serde_json::from_value::<Occluder2D>(edited).unwrap();
        assert_eq!(far.reach, 500.0);
    }

    #[test]
    fn steady_frames_reuse_their_buffers() {
        let mut world = World::new();
        for i in 0..8 {
            world.spawn((
                Transform2D::at(Vec2::new(40.0 * i as f32, 100.0)),
                PointLight2D {
                    shadows: true,
                    ..Default::default()
                },
            ));
            world.spawn((
                Transform2D::at(Vec2::new(40.0 * i as f32 + 20.0, 120.0)),
                Occluder2D::circle(5.0),
            ));
        }
        let mut f = LightFrame2D::default();
        f.collect(&mut world, view(), None);
        let caps = (f.lights.capacity(), f.vertices.capacity(), f.edges.capacity());
        f.collect(&mut world, view(), None);
        assert_eq!(caps, (f.lights.capacity(), f.vertices.capacity(), f.edges.capacity()));
    }
}
