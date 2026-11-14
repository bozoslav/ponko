//! Loading a saved world replaces every entity. Queries the archetype
//! index answered before the load must see the loaded world after it.

mod common;

use common::TempAsset;
use ponko::ecs_core::QueryCache;
use ponko::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
struct Height(u32);
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug, Component)]
struct Tag<const N: usize>;

fn reflect(world: &mut World) {
    world.reflect::<Height>();
    world.reflect::<Tag<0>>();
    world.reflect::<Tag<1>>();
    world.reflect::<Tag<2>>();
    world.reflect::<Tag<3>>();
    world.reflect::<Tag<4>>();
    world.reflect::<Tag<5>>();
    world.reflect::<Tag<6>>();
}

/// One entity per subset of seven tags whose number passes `keep`: up to
/// 128 archetypes.
fn fill(world: &mut World, keep: impl Fn(u32) -> bool) {
    for bits in (0..128).filter(|b| keep(*b)) {
        let e = world.spawn((Height(bits),));
        macro_rules! tag {
            ($($n:literal)*) => {$(
                if bits & (1 << $n) != 0 {
                    world.insert(e, Tag::<$n>);
                }
            )*};
        }
        tag!(0 1 2 3 4 5 6);
    }
}

/// The same three queries with the index (asked for, and left to the
/// world) and without it.
fn answers(world: &mut World) -> [Vec<(Entity, u32)>; 3] {
    let rows = |hint: QueryCache, world: &mut World| -> Vec<(Entity, u32)> {
        let q = world
            .query::<(Entity, &Height)>()
            .filter::<With<Tag<0>>>()
            .filter::<Without<Tag<1>>>();
        let mut q = match hint {
            QueryCache::Always => q.cached(),
            QueryCache::Never => q.uncached(),
            _ => q,
        };
        // Sorted: query order follows archetype slots, which a world that
        // reclaims empty archetypes reuses in its own order, and isn't part
        // of the API (STABILITY.md).
        let mut rows: Vec<(Entity, u32)> = q.iter().map(|(e, h)| (e, h.0)).collect();
        rows.sort_unstable();
        rows
    };
    [
        rows(QueryCache::Always, world),
        rows(QueryCache::Auto, world),
        rows(QueryCache::Never, world),
    ]
}

#[test]
fn indexed_queries_see_the_loaded_world() {
    let file = TempAsset::new("test-scene-query-index.json");
    // The saved world: some of the archetypes.
    let mut saved = Vec::new();
    Game::new("scene index", 1, 1).run_headless(0, |ctx| {
        reflect(&mut ctx.world);
        fill(&mut ctx.world, |bits| bits % 2 == 1 && bits % 3 != 0);
        ctx.assets.save_scene(&ctx.world, file.name()).unwrap();
        saved = answers(&mut ctx.world)[2].clone();
    });
    assert!(!saved.is_empty());

    Game::new("scene index", 1, 1).run_headless(0, |ctx| {
        reflect(&mut ctx.world);
        // Another world, with every archetype, queried until it is indexed.
        fill(&mut ctx.world, |_| true);
        for _ in 0..3 {
            let [hinted, auto, scanned] = answers(&mut ctx.world);
            assert_eq!(hinted, scanned);
            assert_eq!(auto, scanned);
            assert_eq!(scanned.len(), 32);
        }
        assert!(ctx.world.query_cache_stats().indexed >= 1);

        ctx.load_scene(file.name()).unwrap();
        let [hinted, auto, scanned] = answers(&mut ctx.world);
        assert_eq!(scanned, saved, "the load kept ids and components");
        assert_eq!(hinted, saved);
        assert_eq!(auto, saved);

        // And it keeps following the world after the load.
        let late = ctx.world.spawn((Height(999), Tag::<0>, Tag::<5>));
        let [hinted, auto, scanned] = answers(&mut ctx.world);
        assert_eq!(scanned.len(), saved.len() + 1);
        assert!(scanned.contains(&(late, 999)));
        assert_eq!(hinted, scanned);
        assert_eq!(auto, scanned);
    });
}
