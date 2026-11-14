//! Tilemap drawing: `draw_tilemaps` on a 1000 × 1000 map (a million tiles),
//! zoomed out so about 360 000 of them are visible, timed inside headless
//! frames (the draw list is built as in a windowed game, then discarded).
//! `-animated` gives a tenth of the tile kinds a 4-frame animation.
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use ponko::d2::Transform as Transform2D;
use ponko::game::{Phase, Systems};
use ponko::prelude::{Ctx, Game, SpriteSheet, vec2};
use ponko::tilemap::TileMap;

use crate::{Bench, report};

const SIDE: u32 = 1000;
const FRAMES: u64 = 60;
const WARMUP: usize = 5;

fn map(ctx: &mut Ctx, animated: bool) -> TileMap {
    let tex = ctx.gfx.create_texture(16 * 40, 16, &vec![255; 16 * 40 * 16 * 4]);
    let mut map = TileMap::new(SpriteSheet::new(tex, 16, 16), SIDE, SIDE);
    for y in 0..SIDE {
        for x in 0..SIDE {
            map.set(x, y, Some(((x * 7 + y * 13) % 40) as u16));
        }
    }
    if animated {
        animate(&mut map);
    }
    map
}

fn animate(map: &mut TileMap) {
    for tile in (0..40).step_by(10) {
        map.animate(tile, &[(tile, 0.1), (tile + 1, 0.1), (tile + 2, 0.1), (tile + 3, 0.1)]);
    }
}

pub fn run(b: &mut Bench) {
    for animated in [false, true] {
        let name = if animated {
            "tilemap/draw-1m-animated"
        } else {
            "tilemap/draw-1m"
        };
        if b.skips(name) {
            continue;
        }
        let times = Rc::new(RefCell::new(Vec::<Duration>::new()));
        let log = times.clone();
        Game::new("bench", 1280, 720)
            .headless_frames(true)
            .remove(Systems::DRAW_TILEMAPS)
            .add(Phase::Draw, move |ctx: &mut Ctx| {
                let t = Instant::now();
                ponko::systems::draw_tilemaps(ctx);
                log.borrow_mut().push(t.elapsed());
                std::hint::black_box(ctx.gfx.sprite_count());
            })
            .run_headless(FRAMES + WARMUP as u64, move |ctx| {
                let m = map(ctx, animated);
                ctx.world.spawn((Transform2D::default(), m));
                ctx.camera.zoom = 0.1;
                ctx.camera.look_at(vec2(SIDE as f32 * 8.0, SIDE as f32 * 8.0));
            });
        let mut times = times.borrow_mut().split_off(WARMUP);
        b.report(report::Record::new(name, report::Kind::Steady, WARMUP, &mut times));
    }
}
