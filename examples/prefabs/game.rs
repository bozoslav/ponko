//! The `prefabs` example's game: carts spawned from a prefab with children
//! (`assets/prefabs/cart.json`). Each cart is a body with two wheels (the
//! `wheel.json` prefab, included twice), a driver attached to the body and
//! a hat attached to the driver. Shared by `main.rs` and the headless test
//! in `tests/scenes.rs`.

use ponko::d2::Transform;
use ponko::prelude::*;

/// The cart prefab.
pub const CART: &str = "prefabs/cart.json";
/// How far the carts roll each simulation tick, in pixels.
pub const SPEED: f32 = 2.0;
/// The tick at which the first cart is taken away, with its parts.
pub const SCRAP_AT: u64 = 60;

/// Three carts in a row; returns their roots.
pub fn setup(ctx: &mut Ctx) -> Vec<Entity> {
    (0..3)
        .map(|i| {
            ctx.spawn_prefab(CART, vec2(120.0 + i as f32 * 200.0, 300.0))
                .expect("the cart prefab loads")
        })
        .collect()
}

/// Rolls every cart (each root with prefab parts) to the right. Wheels,
/// drivers and hats follow by their attachments.
pub fn roll(ctx: &mut Ctx) {
    ctx.world
        .query::<(&mut Transform, &PrefabParts)>()
        .for_each(|(t, _)| t.position.x += SPEED);
}

/// At [`SCRAP_AT`], despawns the leftmost cart and everything that came
/// with it.
pub fn scrap(ctx: &mut Ctx) {
    if ctx.time.sim_tick() != SCRAP_AT {
        return;
    }
    let first = ctx
        .world
        .query::<(Entity, &Transform, &PrefabParts)>()
        .iter()
        .min_by(|a, b| a.1.position.x.total_cmp(&b.1.position.x))
        .map(|(e, _, _)| e);
    if let Some(cart) = first {
        ctx.despawn_prefab(cart);
    }
}

/// The game with its systems: roll, scrap, then place the attached parts.
pub fn game() -> Game {
    Game::new("prefabs", 800, 600)
        .sim_system(roll)
        .sim_system(scrap)
        .sim_system(systems::follow_attachments)
}
