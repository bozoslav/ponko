//! Text in the 3D world: signs, labels, name tags and damage numbers, sharp
//! at any size and distance. Glyphs are signed distance fields made once
//! per font and character, so one atlas serves every size, and outlines
//! and soft edges come from the same glyphs.

use glam::{Vec2, Vec3};

use super::{Camera3D, Transform3D, linear};
use crate::text::{SDF_PX, SDF_SPREAD, Text};
use crate::{color::Color, ctx::Ctx, text::Font, texture::Texture};
use ponko_ecs::Component;

/// How the lines of a [`Text3D`] line up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum TextAlign3D {
    /// Along the block's left edge.
    Left = 0,
    /// Each line centered. The default.
    #[default]
    Center = 1,
    /// Along the block's right edge.
    Right = 2,
}

/// Text in the world, drawn on its entity's [`d3::Transform`](crate::d3::Transform): a shop sign,
/// a name over a head, a damage number. It stays sharp at any size and
/// distance, and can have an outline and a soft edge (for glows and
/// shadows). Any loaded [`Font`] works, and one entity's text can't mix
/// fonts; use several entities for that.
///
/// The text block is centered on the entity, in its XY plane, read from
/// +Z (like [`UiPanel3D`](crate::d3::UiPanel)), and scaled by its transform.
/// With `billboard` it turns to face the camera instead, keeping upright.
/// It's see-through where there's no text and is drawn with the particles,
/// after solid models; it casts no shadow.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let label = ponko::d3::Text {
///         size: 0.3,
///         outline: Color::BLACK,
///         outline_width: 0.06,
///         billboard: true,
///         ..ponko::d3::Text::new("Shop")
///     };
///     ctx.world.spawn((ponko::d3::Transform::at(vec3(0.0, 2.5, 0.0)), label));
/// });
/// ```
#[derive(Debug, Clone, PartialEq, Component)]
pub struct Text3D {
    /// What it says; `\n` starts a new line.
    pub text: String,
    /// Default [`Font::DEFAULT`].
    pub font: Font,
    /// Height of a line, in meters. Default 0.25.
    pub size: f32,
    /// Default white.
    pub color: Color,
    /// The outline's color. Default transparent (no outline).
    pub outline: Color,
    /// How far the outline reaches out from the letters, in lines (0.05 is
    /// a twentieth of a line); at most 0.12. Default 0.
    pub outline_width: f32,
    /// How soft the edges are, in lines: 0 is sharp; around 0.1 makes a
    /// glow or shadow (use a soft copy behind sharp text). At most 0.12.
    /// Default 0.
    pub softness: f32,
    /// Default [`TextAlign3D::Center`].
    pub align: TextAlign3D,
    /// Turn to face the camera (upright). Default false.
    pub billboard: bool,
}

impl Text3D {
    /// `text` in the default font, a quarter meter per line, white.
    /// ```
    /// # use ponko::prelude::*;
    /// let sign = ponko::d3::Text::new("OPEN");
    /// assert_eq!(sign.size, 0.25);
    /// ```
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            font: Font::DEFAULT,
            size: 0.25,
            color: Color::WHITE,
            outline: Color::TRANSPARENT,
            outline_width: 0.0,
            softness: 0.0,
            align: TextAlign3D::Center,
            billboard: false,
        }
    }
}

/// Most a [`Text3D`]'s outline or softness reaches, in lines: as far as the
/// glyphs record distances.
pub(crate) const MAX_REACH: f32 = (SDF_SPREAD as f32 - 0.5) / SDF_PX;

/// One corner of a glyph quad.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct TextVertex {
    pub position: [f32; 3],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub outline: [f32; 4],
    /// The outline's reach and the softness, in the atlas' distance units
    /// (0.5 is the full spread).
    pub params: [f32; 2],
}

/// This frame's 3D text: glyph quads (a triangle list), farthest text
/// first, and the atlas they sample.
#[derive(Default)]
pub(crate) struct TextFrame {
    pub vertices: Vec<TextVertex>,
    pub atlas: Option<Texture>,
    /// Scratch: (depth, entity) of each text to draw.
    order: Vec<(f32, ponko_ecs::Entity)>,
}

impl TextFrame {
    /// Adds `t`'s glyph quads placed by `at`, laid out in `text` (whose
    /// SDF atlas is `ATLAS` pixels wide).
    pub(crate) fn push(&mut self, t: &Text3D, at: &Transform3D, camera: &Camera3D, text: &mut Text) {
        let block = text.layout_sdf(&t.text, t.font);
        let side = Text::ATLAS_SIZE as f32;
        let meters = t.size.max(0.0) / SDF_PX;
        let widths = text.line_widths();
        let place = |local: Vec2| -> Vec3 {
            let local = local * meters;
            if t.billboard {
                let (right, up) = (camera.rotation * Vec3::X, camera.rotation * Vec3::Y);
                at.position + (right * local.x * at.scale.x) + (up * local.y * at.scale.y)
            } else {
                at.matrix().transform_point3(local.extend(0.0))
            }
        };
        let color = premultiplied(t.color);
        let outline = premultiplied(t.outline);
        let units = |lines: f32| lines.clamp(0.0, MAX_REACH) * SDF_PX / (2.0 * SDF_SPREAD as f32);
        let params = [units(t.outline_width), units(t.softness)];
        for p in text.placed() {
            let Some(atlas_at) = p.glyph.at else { continue };
            let size = Vec2::new(p.glyph.size[0] as f32, p.glyph.size[1] as f32);
            let line = widths.get(p.line as usize).copied().unwrap_or(0.0);
            let shift = match t.align {
                TextAlign3D::Left => -block.x / 2.0,
                TextAlign3D::Center => -line / 2.0,
                TextAlign3D::Right => block.x / 2.0 - line,
            };
            // Pixels (y down from the block's top) to the block's plane
            // (y up from its center).
            let corner =
                |dx: f32, dy: f32| Vec2::new(p.pos.x + dx * size.x + shift, block.y / 2.0 - (p.pos.y + dy * size.y));
            let uv0 = Vec2::new(atlas_at[0] as f32, atlas_at[1] as f32) / side;
            let uv = |dx: f32, dy: f32| (uv0 + Vec2::new(dx, dy) * size / side).to_array();
            let v = |dx: f32, dy: f32| TextVertex {
                position: place(corner(dx, dy)).to_array(),
                uv: uv(dx, dy),
                color,
                outline,
                params,
            };
            let (tl, tr, br, bl) = (v(0.0, 0.0), v(1.0, 0.0), v(1.0, 1.0), v(0.0, 1.0));
            self.vertices.extend([tl, bl, br, tl, br, tr]);
        }
    }
}

fn premultiplied(c: Color) -> [f32; 4] {
    let [r, g, b, a] = linear(c);
    [r * a, g * a, b * a, a]
}

impl Ctx {
    /// Lays out this frame's [`Text3D`]s into glyph quads (farthest first)
    /// and puts new glyphs on the GPU.
    pub(crate) fn collect_text3d(&mut self) {
        let frame = &mut self.gfx.frame3d.particles.texts;
        frame.vertices.clear();
        frame.order.clear();
        let camera = &self.scene3d.camera;
        let (eye, forward) = (camera.position, camera.forward());
        self.world
            .query::<(ponko_ecs::Entity, &Text3D, &Transform3D)>()
            .for_each(|(e, t, at)| {
                if !t.text.is_empty() && t.size > 0.0 {
                    frame.order.push(((at.position - eye).dot(forward), e));
                }
            });
        if frame.order.is_empty() {
            return;
        }
        frame.order.sort_by(|a, b| b.0.total_cmp(&a.0));
        let atlas = match frame.atlas {
            Some(t) => t,
            None => {
                let side = Text::ATLAS_SIZE;
                let t = self.gfx.make_texture(side, side, &vec![0; (side * side * 4) as usize]);
                self.gfx.frame3d.particles.texts.atlas = Some(t);
                t
            }
        };
        let mut frame = std::mem::take(&mut self.gfx.frame3d.particles.texts);
        for &(_, e) in &frame.order.clone() {
            let (Some(t), Some(at)) = (self.world.get::<Text3D>(e), self.world.get::<Transform3D>(e)) else {
                continue;
            };
            frame.push(t, at, &self.scene3d.camera, &mut self.gfx.text);
        }
        if let Some(renderer) = self.gfx.renderer.as_mut() {
            for (at, size, rgba) in self.gfx.text.sdf.uploads.drain(..) {
                renderer.update_texture_region(atlas.id, at, size, &rgba);
            }
        } else {
            self.gfx.text.sdf.uploads.clear();
        }
        self.gfx.frame3d.particles.texts = frame;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Quat;

    fn quads(t: &Text3D, at: &Transform3D) -> Vec<TextVertex> {
        let mut frame = TextFrame::default();
        let mut camera = Camera3D::new(Vec2::new(800.0, 600.0));
        // Level, looking down -Z.
        camera.rotation = Quat::IDENTITY;
        frame.push(t, at, &camera, &mut Text::new());
        frame.vertices
    }

    fn bounds(v: &[TextVertex]) -> (Vec3, Vec3) {
        v.iter()
            .fold((Vec3::splat(f32::MAX), Vec3::splat(f32::MIN)), |(lo, hi), v| {
                let p = Vec3::from_array(v.position);
                (lo.min(p), hi.max(p))
            })
    }

    #[test]
    fn text_is_centered_on_its_entity_and_sized_in_meters() {
        let t = Text3D::new("Hello");
        let at = Transform3D::at(Vec3::new(1.0, 2.0, 3.0));
        let v = quads(&t, &at);
        assert_eq!(v.len(), 5 * 6, "a quad per letter");
        let (lo, hi) = bounds(&v);
        // Centered (glyph padding is symmetric give or take a pixel).
        let center = (lo + hi) / 2.0;
        assert!(
            (center.x - 1.0).abs() < 0.05 && (center.y - 2.0).abs() < 0.08,
            "{center}"
        );
        assert!(lo.z == 3.0 && hi.z == 3.0, "flat in its XY plane");
        // A line is about `size` tall (plus the distance padding).
        let pad = 2.0 * SDF_SPREAD as f32 / SDF_PX * t.size;
        assert!(hi.y - lo.y < t.size + pad, "{}", hi.y - lo.y);
        // Twice the size, twice as big.
        let (lo2, hi2) = bounds(&quads(&Text3D { size: 0.5, ..t.clone() }, &at));
        assert!(((hi2.x - lo2.x) / (hi.x - lo.x) - 2.0).abs() < 1e-3);
    }

    #[test]
    fn lines_line_up_as_asked() {
        let at = Transform3D::default();
        let text = "a\nwide line";
        let left_of_first = |align| {
            let v = quads(
                &Text3D {
                    align,
                    ..Text3D::new(text)
                },
                &at,
            );
            // The first letter's quad comes first.
            v[..6].iter().map(|v| v.position[0]).fold(f32::MAX, f32::min)
        };
        let (l, c, r) = (
            left_of_first(TextAlign3D::Left),
            left_of_first(TextAlign3D::Center),
            left_of_first(TextAlign3D::Right),
        );
        assert!(l < c && c < r, "{l} {c} {r}");
        // The widest line is the same whatever the alignment.
        let wide = |align| {
            bounds(&quads(
                &Text3D {
                    align,
                    ..Text3D::new(text)
                },
                &at,
            ))
        };
        assert!((wide(TextAlign3D::Left).0.x - wide(TextAlign3D::Right).0.x).abs() < 1e-4);
    }

    #[test]
    fn turned_text_follows_its_transform_and_billboards_face_the_camera() {
        let turned = Transform3D::default().rotated(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let (lo, hi) = bounds(&quads(&Text3D::new("Hi"), &turned));
        assert!(hi.x - lo.x < 1e-4, "turned to face +X, it's flat in YZ");
        assert!(hi.z - lo.z > 0.05);
        // The camera looks down -Z: a billboard is flat in XY,
        // whatever its entity's rotation.
        let t = Text3D {
            billboard: true,
            ..Text3D::new("Hi")
        };
        let (lo, hi) = bounds(&quads(&t, &turned));
        assert!(hi.z - lo.z < 1e-4 && hi.x - lo.x > 0.05);
    }

    #[test]
    fn outlines_and_softness_are_kept_within_the_glyphs_reach() {
        let t = Text3D {
            outline: Color::BLACK,
            outline_width: 1.0,
            softness: 0.05,
            ..Text3D::new("x")
        };
        let v = quads(&t, &Transform3D::default());
        let [outline, soft] = v[0].params;
        assert!(outline < 0.5 && outline > 0.4, "clamped below the spread: {outline}");
        assert!((soft - 0.05 * SDF_PX / (2.0 * SDF_SPREAD as f32)).abs() < 1e-6);
        assert_eq!(v[0].outline, [0.0, 0.0, 0.0, 1.0]);
        // Spaces take room but draw nothing; empty text draws nothing.
        assert_eq!(quads(&Text3D::new("a b"), &Transform3D::default()).len(), 12);
        assert!(quads(&Text3D::new(""), &Transform3D::default()).is_empty());
    }
}
