//! Sprite sheets, animation and tilemaps, checked headless. The built-in
//! drawing systems are plain functions, so tests call them directly.

#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
use ponko::prelude::*;
use ponko::sheet::Animation;

fn in_tick(f: impl FnMut(&mut Ctx) + 'static) {
    Game::new("tiles", 800, 600).sim_system(f).run_headless(1, |_| {});
}

fn sheet(ctx: &mut Ctx) -> SpriteSheet {
    let tex = ctx.gfx.create_texture(64, 32, &vec![255; 64 * 32 * 4]);
    SpriteSheet::new(tex, 16, 16)
}

#[test]
fn animations_show_their_frame_on_the_sprite() {
    in_tick(|ctx| {
        let s = sheet(ctx);
        let e = ctx
            .world
            .spawn((Transform2D::default(), s.sprite(0), Animation::new(s, 4..8, 30.0)));
        // One fixed step at 60 Hz is half a frame at 30 fps: still frame 4.
        systems::animate_sprites(ctx);
        assert_eq!(ctx.world.get::<Sprite>(e).unwrap().region, Some(s.frame(4)));
        systems::animate_sprites(ctx);
        systems::animate_sprites(ctx);
        assert_eq!(ctx.world.get::<Animation>(e).unwrap().current_frame(), 5);
        assert_eq!(ctx.world.get::<Sprite>(e).unwrap().region, Some(s.frame(5)));
    });
}

#[test]
fn tilemaps_draw_only_visible_tiles() {
    in_tick(|ctx| {
        let s = sheet(ctx);
        let mut map = ponko::tilemap::TileMap::new(s, 1000, 1000); // 16 000 × 16 000 world units
        map.tiles.fill(1);
        map.set(0, 0, None);
        ctx.world.spawn((Transform2D::default(), map));

        // 800 × 600 window at zoom 1 from (0, 0): 50 × 38 tiles, minus the empty one.
        systems::draw_tilemaps(ctx);
        assert_eq!(ctx.gfx.sprite_count(), 50 * 38 - 1);

        // Looking past the map's edge: nothing.
        ctx.camera.position = vec2(-5000.0, -5000.0);
        let before = ctx.gfx.sprite_count();
        systems::draw_tilemaps(ctx);
        assert_eq!(ctx.gfx.sprite_count(), before);
    });
}

#[test]
fn tile_lookup_by_world_position() {
    in_tick(|ctx| {
        let s = sheet(ctx);
        let mut map = ponko::tilemap::TileMap::new(s, 10, 5);
        let origin = vec2(100.0, 100.0);
        assert_eq!(map.tile_at(origin, vec2(100.0, 100.0)), Some(UVec2::new(0, 0)));
        assert_eq!(map.tile_at(origin, vec2(259.0, 179.0)), Some(UVec2::new(9, 4)));
        assert_eq!(map.tile_at(origin, vec2(99.0, 100.0)), None);
        assert_eq!(map.tile_at(origin, vec2(260.0, 100.0)), None);
        map.set(9, 4, Some(3));
        assert_eq!(map.get(9, 4), Some(3));
        map.set(99, 99, Some(3)); // ignored
        assert_eq!(map.get(99, 99), None);
    });
}

#[test]
fn sprite_regions_and_sheets_draw_like_sprites() {
    in_tick(|ctx| {
        let s = sheet(ctx);
        ctx.gfx
            .draw_ex(s.texture, Vec2::ZERO, DrawParams::default().with_region(s.frame(1)));
        assert_eq!(ctx.gfx.sprite_count(), 1);
        assert_eq!(s.sprite(2).region, Some(Rect::new(32.0, 0.0, 16.0, 16.0)));
    });
}
