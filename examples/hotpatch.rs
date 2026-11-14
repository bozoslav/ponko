//! Hot-patching game code: change a system while the game runs, and the
//! running game picks up the new code without losing any state.
//!
//! ```sh
//! cargo install dioxus-cli@0.7.10   # once; installs `dx`
//! dx serve --hot-patch --example hotpatch --features hotpatch
//! ```
//!
//! Then edit `spin` or `wobble` below and save: the crabs change behavior
//! within a second, mid-spin, where they were.

#[allow(unused_imports)]
use ponko::d2::*;
use ponko::prelude::*;

/// Every crab turns. Try a different speed, or `-=` to turn the other way.
// ANCHOR: spin
fn spin(ctx: &mut Ctx) {
    let dt = ctx.time.dt();
    for t in ctx.world.query::<&mut d2::Transform>().iter() {
        t.rotation += -6.0 * dt;
    }
}
// ANCHOR_END: spin

/// Crabs breathe. Try a bigger amplitude, or a different shape.
fn wobble(ctx: &mut Ctx) {
    let time = ctx.time.sim_time() as f32;
    for (t, crab) in ctx.world.query::<(&mut d2::Transform, &Crab)>().iter() {
        let s = 1.0 + 0.15 * (time * 3.0 + crab.0).sin();
        t.scale = vec2(s, s);
    }
}

/// Prints the first crab's rotation once a second, so you can see that
/// state carries over a patch.
fn report(ctx: &mut Ctx) {
    let tick = ctx.time.sim_tick();
    if tick.is_multiple_of(60)
        && let Some(t) = ctx.world.query::<&d2::Transform>().iter().next()
    {
        println!("tick {tick:>5}  rotation {:>7.2}", t.rotation);
    }
}

/// A phase, so crabs don't all move together.
#[derive(Component)]
struct Crab(f32);

fn main() {
    Game::new("Ponko: hot patching", 1280, 720)
        .sim_system(spin)
        .sim_system(wobble)
        .sim_system(report)
        .setup(|ctx| {
            let crab = ctx.gfx.load_texture("crab.png");
            for i in 0..12 {
                let at = vec2(160.0 + (i % 6) as f32 * 190.0, 220.0 + (i / 6) as f32 * 280.0);
                ctx.world
                    .spawn((d2::Transform::at(at), Sprite::new(crab), Crab(i as f32 * 0.7)));
            }
        })
        .run();
}
