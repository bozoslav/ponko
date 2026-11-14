//! Retained UI on a screen in the 3D world: `cargo run --features 3d --example ui_panel`.
use ponko::render::{MaterialDesc, MeshData};
use ponko::{d3::*, prelude::*, ui::UiLayout};
pub fn setup(ctx: &mut Ctx) {
    ctx.scene3d.camera.position = vec3(0.0, 1.6, 1.0);
    ctx.scene3d.camera.look_at(vec3(0.0, 1.5, -2.0));
    let floor_material = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x6B7B58)));
    let floor = ctx.gfx.create_mesh(&MeshData::plane(12.0), floor_material);
    ctx.world.spawn((Transform::default(), Model::new(floor)));
    // ANCHOR: panel_create
    // A 1.2 x 0.8 m terminal, 600 x 400 pixels, with a button.
    let (mut panel, model) = ctx.gfx.create_ui_panel(600, 400, vec2(1.2, 0.8));
    let ui = &mut panel.ui;
    let root = ui.root().layout(UiLayout::fill().padding(20.0).gap(12.0)).id();
    ui.button(root, "Buy").layout(UiLayout::fixed(200.0, 60.0)).name("buy");
    ui.push_screen(root, false);
    ctx.world.spawn((Transform::at(vec3(0.0, 1.5, -2.0)), model, panel));
    // ANCHOR_END: panel_create
}

// ANCHOR: panel_events
pub fn terminal(ctx: &mut Ctx) {
    // Aim with the middle of the screen, not the mouse (a locked cursor, a controller in VR).
    let (eye, forward) = (ctx.scene3d.camera.position, ctx.scene3d.camera.forward());
    ctx.ui.set_pointer_ray(Some((eye, forward)));
    ctx.world.query::<&mut crate::d3::UiPanel>().for_each(|panel| {
        let clicked = panel.ui.events().filter_map(ponko::ui::UiEvent::clicked);
        if let Some(buy) = clicked.last().filter(|&id| panel.ui.name(id) == Some("buy")) {
            panel.ui.set_text(buy, "Bought!");
        }
    });
}
// ANCHOR_END: panel_events
fn main() {
    Game::new("UI panel", 960, 640).system(terminal).setup(setup).run();
}
