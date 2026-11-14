//! Retained UI on surfaces in the 3D world: screens, terminals, signs and
//! menus on things. Each panel has its own [`Ui`], drawn every frame into a
//! render target that a material shows on a quad; the mouse uses it where
//! the camera ray hits that quad.

use glam::{Mat4, Vec2, Vec3};

use super::retained::Ui;
use crate::d3::Transform as Transform3D;
use crate::gfx::Graphics;
use crate::render::batch::PanelDraw;
use crate::{ctx::Ctx, input::MouseButton, texture::Texture};
use ponko_ecs::{Component, Entity};

/// Retained UI shown on a quad in the world (Rust's [`Ui`] on a surface):
/// a computer screen, a shop terminal, a sign with buttons. Build `ui` like
/// [`Ctx::ui`]; its widgets are laid out in `texture`'s pixels.
///
/// The panel is the quad `size` meters wide and high in its entity's XY
/// plane, centered on it, facing +Z (what [`mesh`](Self::mesh) makes). The
/// mouse uses the nearest panel the camera ray through it hits, unless the
/// screen's own UI has it; the panel clicked last also gets the keyboard.
/// [`Graphics::create_ui_panel`](crate::gfx::Graphics::create_ui_panel) makes the texture, material and mesh at once.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     // A 1.2 x 0.8 m terminal, 600 x 400 pixels.
///     let (mut panel, model) = ctx.gfx.create_ui_panel(600, 400, vec2(1.2, 0.8));
///     let buy = panel.ui.add(ponko::ui::UiWidget::Button, ponko::ui::UiLayout::auto());
///     panel.ui.set_text(buy, "Buy");
///     ctx.world.spawn((ponko::d3::Transform::at(vec3(0.0, 1.5, -2.0)), model, panel));
/// });
/// ```
#[derive(Component)]
pub struct UiPanel3D {
    /// Its widgets, events, focus and screens.
    pub ui: Ui,
    /// What it's drawn into each frame: a render target
    /// ([`Graphics::create_render_target`](crate::gfx::Graphics::create_render_target)) whose size is the panel's in pixels.
    pub texture: Texture,
    /// Its size in the world, in meters.
    pub size: Vec2,
    /// Whether the mouse and keyboard can use it. Default true.
    pub interactive: bool,
    /// Where the mouse is on it this frame, in its pixels.
    pointer: Option<Vec2>,
}

impl std::fmt::Debug for UiPanel3D {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UiPanel3D")
            .field("texture", &self.texture)
            .field("size", &self.size)
            .field("interactive", &self.interactive)
            .field("pointer", &self.pointer)
            .finish_non_exhaustive()
    }
}

impl UiPanel3D {
    /// A panel drawn into `texture` (a render target), `size` meters in the
    /// world. Show `texture` on [`mesh`](Self::mesh) with a material.
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let screen = ctx.gfx.create_render_target(512, 256);
    ///     let panel = ponko::d3::UiPanel::new(screen, vec2(1.0, 0.5));
    ///     assert!(panel.interactive && panel.pointer().is_none());
    /// });
    /// ```
    #[must_use]
    pub fn new(texture: Texture, size: Vec2) -> Self {
        Self {
            ui: Ui::default(),
            texture,
            size,
            interactive: true,
            pointer: None,
        }
    }

    /// The quad it's shown on: `size` in the XY plane, centered, facing +Z,
    /// with the texture's top-left corner at the top left.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(screen: Texture) {
    /// let panel = ponko::d3::UiPanel::new(screen, vec2(2.0, 1.0));
    /// let quad = panel.mesh();
    /// assert_eq!(quad.positions[0], vec3(-1.0, 0.5, 0.0));
    /// assert_eq!(quad.uvs[0], Vec2::ZERO);
    /// # }
    /// ```
    #[must_use]
    pub fn mesh(&self) -> crate::render::MeshData {
        let (x, y) = (self.size.x * 0.5, self.size.y * 0.5);
        crate::render::MeshData {
            positions: vec![
                Vec3::new(-x, y, 0.0),
                Vec3::new(x, y, 0.0),
                Vec3::new(x, -y, 0.0),
                Vec3::new(-x, -y, 0.0),
            ],
            normals: vec![Vec3::Z; 4],
            uvs: vec![Vec2::ZERO, Vec2::X, Vec2::ONE, Vec2::Y],
            indices: vec![0, 2, 1, 0, 3, 2],
            ..Default::default()
        }
    }

    /// Where the mouse is on it this frame, in its pixels; `None` when it
    /// isn't over it.
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(panel: &ponko::d3::UiPanel) {
    /// if let Some(at) = panel.pointer() {
    ///     println!("hovering at {at}");
    /// }
    /// # }
    /// ```
    #[must_use]
    pub fn pointer(&self) -> Option<Vec2> {
        self.pointer
    }

    /// Where the ray from `origin` along `direction` hits the panel placed
    /// by `matrix`: its distance along the ray and the pixel.
    fn hit(&self, matrix: Mat4, origin: Vec3, direction: Vec3) -> Option<(f32, Vec2)> {
        let to_local = matrix.inverse();
        if !to_local.is_finite() {
            return None;
        }
        let (o, d) = (to_local.transform_point3(origin), to_local.transform_vector3(direction));
        if d.z.abs() < 1e-9 {
            return None;
        }
        let k = -o.z / d.z;
        let at = o + d * k;
        let half = self.size * 0.5;
        if k <= 0.0 || at.x.abs() > half.x || at.y.abs() > half.y {
            return None;
        }
        let distance = (matrix.transform_point3(at) - origin).length();
        let pixels = Vec2::new(self.texture.width() as f32, self.texture.height() as f32);
        let uv = Vec2::new(at.x / self.size.x + 0.5, 0.5 - at.y / self.size.y);
        Some((distance, uv * pixels))
    }
}

/// Panels' state that lives in the context.
#[derive(Default)]
pub(crate) struct Panels {
    /// The panel clicked last: it gets the keyboard.
    active: Option<Entity>,
    /// Scratch: this frame's panels.
    entities: Vec<Entity>,
    /// This frame's drawing, one per panel (lists reused).
    pub draws: Vec<PanelDraw>,
    used: usize,
}

impl Panels {
    /// Forgets which panel has the keyboard (after a scene load: its entity
    /// id may name another panel now).
    pub fn forget_focus(&mut self) {
        self.active = None;
    }

    /// This frame's drawing.
    pub fn frame_draws(&mut self) -> &mut [PanelDraw] {
        &mut self.draws[..self.used]
    }
}

impl Graphics {
    /// A [`UiPanel3D`] `width` x `height` pixels, `size` meters in the
    /// world, and the [`Model`](crate::render::Model) that shows it: a quad with a
    /// glowing, alpha-blended material, so it reads in the dark and its
    /// clear parts let the world through. Spawn both on one entity.
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let (panel, model) = ctx.gfx.create_ui_panel(256, 256, vec2(0.5, 0.5));
    ///     assert_eq!(panel.texture.width(), 256);
    ///     ctx.world.spawn((ponko::d3::Transform::default(), model, panel));
    /// });
    /// ```
    ///
    /// # Panics
    ///
    /// If a pixel dimension is zero, the world size is not positive and finite,
    /// or the GPU can't make a texture that large.
    pub fn create_ui_panel(&mut self, width: u32, height: u32, size: Vec2) -> (UiPanel3D, crate::render::Model) {
        self.try_create_ui_panel(width, height, size)
            .unwrap_or_else(|message| panic!("{message}"))
    }

    /// [`create_ui_panel`](Self::create_ui_panel) with errors as values:
    /// [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) for a size that
    /// isn't positive and finite, and the errors of
    /// [`try_create_render_target`](Self::try_create_render_target).
    pub fn try_create_ui_panel(
        &mut self,
        width: u32,
        height: u32,
        size: Vec2,
    ) -> Result<(UiPanel3D, crate::render::Model), crate::error::Error> {
        if !size.is_finite() || size.x <= 0.0 || size.y <= 0.0 {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::Invalid,
                "panel size must be positive and finite",
            ));
        }
        let texture = self.try_create_render_target(width, height)?;
        let panel = UiPanel3D::new(texture, size);
        let material = self.create_material(crate::render::MaterialDesc {
            color: crate::color::Color::BLACK,
            texture: Some(texture),
            emissive: crate::color::Color::WHITE,
            emissive_map: Some(texture),
            blend: crate::render::Blend::Transparent,
            ..Default::default()
        });
        let mesh = self.create_mesh(&panel.mesh(), material);
        Ok((panel, crate::render::Model::new(mesh)))
    }
}

impl Ctx {
    /// Gives every [`UiPanel3D`] this frame's input: the mouse to the
    /// nearest one it's over, the keyboard to the one clicked last.
    pub(crate) fn update_ui_panels(&mut self) {
        let mut entities = std::mem::take(&mut self.ui_state.panels.entities);
        entities.clear();
        self.world
            .query::<(Entity, &UiPanel3D, &Transform3D)>()
            .for_each(|(e, _, _)| entities.push(e));
        if entities.is_empty() {
            self.ui_state.panels.entities = entities;
            return;
        }
        let (origin, direction) = self
            .ui
            .pointer_ray
            .unwrap_or_else(|| self.scene3d.camera.screen_ray(self.input.mouse_pos()));
        // The screen's UI comes first.
        let screen_has_it = self.ui.wants_pointer();
        let mut nearest: Option<(Entity, f32, Vec2)> = None;
        for &e in &entities {
            let (Some(panel), Some(at)) = (self.world.get::<UiPanel3D>(e), self.world.get::<Transform3D>(e)) else {
                continue;
            };
            if !panel.interactive || screen_has_it {
                continue;
            }
            if let Some((distance, pixel)) = panel.hit(at.matrix(), origin, direction)
                && nearest.is_none_or(|n| distance < n.1)
            {
                nearest = Some((e, distance, pixel));
            }
        }
        if self.input.mouse_pressed(MouseButton::Left) {
            self.ui_state.panels.active = nearest.map(|n| n.0);
        }
        let active = self.ui_state.panels.active;
        for &e in &entities {
            let Some(mut ui) = self.world.get_mut::<UiPanel3D>(e).map(|p| std::mem::take(&mut p.ui)) else {
                continue;
            };
            let pointer = nearest.filter(|n| n.0 == e).map(|n| n.2);
            let full = self.retained_input(&ui, String::new());
            let mut input = if active == Some(e) {
                full
            } else {
                // Only the pointer: no keys, no typing.
                crate::ui::UiInput {
                    held: full.held,
                    released: full.released,
                    ..Default::default()
                }
            };
            // Off the panel: far away, so nothing is hovered or pressed.
            input.pointer = pointer.unwrap_or(Vec2::splat(-1e9));
            if pointer.is_none() {
                input.pressed = false;
                input.wheel = Vec2::ZERO;
            }
            let Some(panel) = self.world.get_mut::<UiPanel3D>(e) else {
                continue;
            };
            let viewport = Vec2::new(panel.texture.width() as f32, panel.texture.height() as f32);
            panel.pointer = pointer;
            ui.frame_with_text(viewport, &input, &mut self.gfx.text);
            if let Some(panel) = self.world.get_mut::<UiPanel3D>(e) {
                panel.ui = ui;
            }
        }
        self.ui_state.panels.entities = entities;
    }

    /// Draws every [`UiPanel3D`] into its own list for the renderer.
    pub(crate) fn draw_ui_panels(&mut self) {
        self.ui_state.panels.used = 0;
        if self.headless {
            return;
        }
        let mut entities = std::mem::take(&mut self.ui_state.panels.entities);
        entities.clear();
        self.world
            .query::<(Entity, &UiPanel3D)>()
            .for_each(|(e, _)| entities.push(e));
        // Panels' pixels are the texture's: no window scaling.
        let window_scale = std::mem::replace(&mut self.gfx.scale_factor, 1.0);
        for &e in &entities {
            let Some((mut ui, texture)) = self
                .world
                .get_mut::<UiPanel3D>(e)
                .map(|p| (std::mem::take(&mut p.ui), p.texture))
            else {
                continue;
            };
            // A freed render target is drawn into no more.
            if !self.gfx.is_render_target(texture) {
                if let Some(panel) = self.world.get_mut::<UiPanel3D>(e) {
                    panel.ui = ui;
                }
                continue;
            }
            let used = self.ui_state.panels.used;
            if used == self.ui_state.panels.draws.len() {
                self.ui_state.panels.draws.push(PanelDraw::default());
            }
            let mut draw = std::mem::take(&mut self.ui_state.panels.draws[used]);
            draw.texture = texture.id;
            draw.size = [texture.width(), texture.height()];
            draw.list.clear();
            std::mem::swap(&mut self.gfx.ui_list, &mut draw.list);
            self.draw_ui_tree(&mut ui, Vec2::new(texture.width() as f32, texture.height() as f32));
            std::mem::swap(&mut self.gfx.ui_list, &mut draw.list);
            self.ui_state.panels.draws[used] = draw;
            self.ui_state.panels.used += 1;
            if let Some(panel) = self.world.get_mut::<UiPanel3D>(e) {
                panel.ui = ui;
            }
        }
        self.gfx.scale_factor = window_scale;
        self.ui_state.panels.entities = entities;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::UiEvent;
    use crate::ui::UiWidget;
    use glam::{Vec2 as V2, vec3};

    fn panel() -> UiPanel3D {
        UiPanel3D::new(Texture::new(1, 200, 100), V2::new(2.0, 1.0))
    }

    #[test]
    fn rays_hit_panels_at_their_pixels() {
        let p = panel();
        let at = Transform3D::at(vec3(0.0, 0.0, -5.0));
        // Straight at the center.
        let (distance, pixel) = p.hit(at.matrix(), Vec3::ZERO, -Vec3::Z).unwrap();
        assert!((distance - 5.0).abs() < 1e-5);
        assert!(pixel.abs_diff_eq(V2::new(100.0, 50.0), 1e-3));
        // The top-left quarter.
        let (_, pixel) = p.hit(at.matrix(), vec3(-0.5, 0.25, 0.0), -Vec3::Z).unwrap();
        assert!(pixel.abs_diff_eq(V2::new(50.0, 25.0), 1e-3), "{pixel}");
        // Missing it, going away from it, along it.
        assert!(p.hit(at.matrix(), vec3(3.0, 0.0, 0.0), -Vec3::Z).is_none());
        assert!(p.hit(at.matrix(), Vec3::ZERO, Vec3::Z).is_none());
        assert!(p.hit(at.matrix(), Vec3::ZERO, Vec3::X).is_none());
    }

    #[test]
    fn turned_panels_are_hit_on_their_own_plane() {
        let p = panel();
        // Turned to face +X, 3 m along X.
        let mut at = Transform3D::at(vec3(3.0, 0.0, 0.0));
        at.rotation = glam::Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let (distance, pixel) = p.hit(at.matrix(), vec3(6.0, 0.0, 0.0), -Vec3::X).unwrap();
        assert!((distance - 3.0).abs() < 1e-5);
        assert!(pixel.abs_diff_eq(V2::new(100.0, 50.0), 1e-3));
    }

    #[test]
    fn the_mouse_clicks_buttons_on_the_nearest_panel() {
        let mut ctx = Ctx::new(1.0 / 60.0, V2::new(800.0, 600.0));
        ctx.scene3d.camera = crate::d3::Camera::new(V2::new(800.0, 600.0));
        ctx.scene3d.camera.position = Vec3::ZERO;
        ctx.scene3d.camera.look_at(-Vec3::Z);
        let mut spawn = |z: f32| {
            let mut p = UiPanel3D::new(Texture::new(1, 200, 100), V2::new(2.0, 1.0));
            let style = taffy::Style {
                size: taffy::Size::from_percent(1.0, 1.0),
                ..Default::default()
            };
            let button = p.ui.add(UiWidget::Button, crate::ui::UiLayout::custom(style));
            p.ui.push_screen(button, false);
            (ctx.world.spawn((Transform3D::at(vec3(0.0, 0.0, z)), p)), button)
        };
        let (near, near_button) = spawn(-3.0);
        let (far, _) = spawn(-6.0);
        let mut ctx = ctx;
        // Lay out once, then click in the middle of the screen.
        ctx.update_ui_panels();
        ctx.input.set_mouse_pos(V2::new(400.0, 300.0));
        ctx.input.set_mouse_button(MouseButton::Left, true);
        ctx.update_ui_panels();
        ctx.input.end_frame();
        ctx.input.set_mouse_button(MouseButton::Left, false);
        ctx.update_ui_panels();
        let events: Vec<UiEvent> = ctx.world.get_mut::<UiPanel3D>(near).unwrap().ui.events().collect();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, UiEvent::Clicked(id) if *id == near_button)),
            "{events:?}"
        );
        let far_events = ctx.world.get_mut::<UiPanel3D>(far).unwrap().ui.events().count();
        assert_eq!(far_events, 0, "the far panel is behind the near one");
        assert!(ctx.world.get::<UiPanel3D>(near).unwrap().pointer().is_some());
        assert!(ctx.world.get::<UiPanel3D>(far).unwrap().pointer().is_none());
        assert_eq!(
            ctx.ui_state.panels.active,
            Some(near),
            "clicked last: it gets the keyboard"
        );
        // A set ray goes elsewhere: nothing is under it.
        ctx.ui.set_pointer_ray(Some((Vec3::ZERO, Vec3::X)));
        ctx.update_ui_panels();
        assert!(ctx.world.get::<UiPanel3D>(near).unwrap().pointer().is_none());
    }

    #[test]
    fn a_scene_load_forgets_which_panel_has_the_keyboard() {
        let mut ctx = Ctx::new(1.0 / 60.0, V2::new(800.0, 600.0));
        let e = ctx.world.spawn((Transform3D::default(), panel()));
        ctx.ui_state.panels.active = Some(e); // clicked last
        // A scene with the same ids (transforms only: panels aren't saved).
        let save = crate::reflect::WorldReflect::save(&ctx.world);
        ctx.replace_world(&save).unwrap();
        assert!(ctx.world.is_alive(e), "the scene has the same id");
        assert_eq!(ctx.ui_state.panels.active, None, "the old panel's focus is gone");
        // The game puts a new panel on it: no keyboard until it's clicked.
        ctx.world.insert(e, panel());
        ctx.update_ui_panels();
        assert_eq!(ctx.ui_state.panels.active, None);
        assert!(
            ctx.gfx.frame3d.motion.reset,
            "same-id models have no motion from the old scene"
        );
    }
}
