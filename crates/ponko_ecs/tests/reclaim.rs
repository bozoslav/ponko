//! Reclaiming empty archetypes (issue #347): a world that keeps visiting new
//! component sets keeps a bounded number of archetypes, reusing the slots
//! and storage of the ones that stayed empty longest, and entities,
//! queries, views, transition edges and change detection give the same
//! answers as if every archetype were kept.

use ponko_ecs::*;

#[derive(Debug, Clone, Copy, PartialEq, Component)]
struct Value(u64);
/// Eight markers: bit `N` of a mask.
#[derive(Component)]
struct M<const N: usize>;

/// Gives `e` the markers of `mask`'s set bits, one at a time (so it passes
/// through the sets in between too).
fn insert_markers(w: &mut World, e: Entity, mask: u32) {
    macro_rules! each {
        ($($n:literal)*) => { $(if mask >> $n & 1 != 0 { w.insert(e, M::<$n>); })* };
    }
    each!(0 1 2 3 4 5 6 7);
}

/// Takes the markers of `mask`'s set bits from `e`.
fn remove_markers(w: &mut World, e: Entity, mask: u32) {
    macro_rules! each {
        ($($n:literal)*) => { $(if mask >> $n & 1 != 0 { w.remove::<M<$n>>(e); })* };
    }
    each!(0 1 2 3 4 5 6 7);
}

/// Spawns a short-lived entity for each mask and despawns it: the world
/// passes through, and empties, archetypes for all of them.
fn visit(w: &mut World, masks: impl IntoIterator<Item = u32>) {
    for mask in masks {
        let e = w.spawn((Value(u64::from(mask)),));
        insert_markers(w, e, mask);
        assert!(w.despawn(e));
    }
}

fn slots(w: &World) -> usize {
    w.raw_ref().archetypes().len()
}

#[test]
fn a_world_reuses_reclaimed_archetype_slots_for_new_component_sets() {
    let mut w = World::new();
    visit(&mut w, 0..256);
    let after_one_round = slots(&w);
    // 256 marker sets, and the sets passed through on the way to each:
    // without reclaiming, every one would keep an archetype.
    assert!(after_one_round < 256, "{after_one_round} slots");
    for _ in 0..3 {
        visit(&mut w, (0..256).rev());
        visit(&mut w, 0..256);
    }
    assert_eq!(slots(&w), after_one_round, "later rounds reuse the same slots");
    assert!(w.is_empty());
}

#[test]
fn entities_move_to_the_right_archetypes_after_their_neighbours_were_reclaimed() {
    let mut w = World::new();
    let e = w.spawn((Value(7),));
    insert_markers(&mut w, e, 0b1);
    // Builds (and links) archetypes next to `e`'s, then reclaims them.
    visit(&mut w, 0..256);
    visit(&mut w, (0..256).filter(|m| m & 0b1 == 0));
    // Each step follows an edge that may have led to a reclaimed archetype.
    let mut mask = 0b1;
    for step in [0b10, 0b100, 0b1, 0b1000_0000, 0b10, 0b1] {
        if mask & step == 0 {
            insert_markers(&mut w, e, step);
        } else {
            remove_markers(&mut w, e, step);
        }
        mask ^= step;
        visit(&mut w, (0..256).filter(|&m| m != mask));
        assert_eq!(w.get::<Value>(e), Some(&Value(7)));
        let has = [
            w.has::<M<0>>(e),
            w.has::<M<1>>(e),
            w.has::<M<2>>(e),
            w.has::<M<3>>(e),
            w.has::<M<4>>(e),
            w.has::<M<5>>(e),
            w.has::<M<6>>(e),
            w.has::<M<7>>(e),
        ];
        let expected: Vec<bool> = (0..8).map(|n| mask >> n & 1 != 0).collect();
        assert_eq!(has.to_vec(), expected, "after flipping {step:#b}");
        assert_eq!(w.query::<&Value>().count(), 1);
        assert_eq!(
            w.query::<&Value>().filter::<With<M<1>>>().count(),
            usize::from(mask & 0b10 != 0)
        );
    }
}

#[test]
fn indexed_queries_follow_slots_reused_for_other_component_sets() {
    let mut w = World::new();
    let mut policy = QueryCachePolicy::new();
    policy.min_archetypes = 1;
    policy.promote_after = 1;
    policy.max_match_percent = 100;
    w.set_query_cache_policy(policy);
    let kept = w.spawn((Value(1),));
    insert_markers(&mut w, kept, 0b1000);
    for round in 0..4u64 {
        // Index the signature, then let its archetypes be reclaimed and
        // their slots reused by sets with and without `M<3>`.
        let indexed = w.query::<&Value>().filter::<With<M<3>>>().cached().count();
        assert_eq!(indexed, 1);
        visit(&mut w, (0..256).map(|m| (m * 37 + round as u32) % 256));
        let live: Vec<Entity> = (0..256u32)
            .filter(|m| m % 3 == round as u32 % 3)
            .map(|m| {
                let e = w.spawn((Value(u64::from(m) + 100),));
                insert_markers(&mut w, e, m);
                e
            })
            .collect();
        for hint in ["cached", "auto", "uncached"] {
            let query = w.query::<&Value>().filter::<With<M<3>>>();
            let mut query = match hint {
                "cached" => query.cached(),
                "uncached" => query.uncached(),
                _ => query,
            };
            let mut got: Vec<u64> = query.iter().map(|v| v.0).collect();
            got.sort_unstable();
            let mut expected: Vec<u64> = (0..256u64)
                .filter(|m| m % 3 == round % 3 && m & 0b1000 != 0)
                .map(|m| m + 100)
                .chain([1])
                .collect();
            expected.sort_unstable();
            assert_eq!(got, expected, "round {round}, {hint}");
        }
        for e in live {
            w.despawn(e);
        }
    }
    assert!(w.query_cache_stats().indexed > 0);
}

#[test]
fn views_scan_the_archetypes_of_a_world_that_reclaims() {
    let mut w = World::new();
    visit(&mut w, 0..256);
    let a = w.spawn((Value(1),));
    insert_markers(&mut w, a, 0b101);
    let b = w.spawn((Value(2),));
    insert_markers(&mut w, b, 0b1000_0001);
    visit(&mut w, (0..256).rev());
    let mut state = ViewState::new().write::<Value>().read::<M<0>>().read::<M<2>>();
    state.run(&mut w, |mut view| {
        view.query::<&mut Value>().for_each(|v| v.0 *= 10);
        assert_eq!(view.query::<&Value>().filter::<With<M<0>>>().count(), 2);
        assert_eq!(view.query::<&Value>().filter::<With<M<2>>>().count(), 1);
    });
    assert_eq!(w.get::<Value>(a), Some(&Value(10)));
    assert_eq!(w.get::<Value>(b), Some(&Value(20)));
}

#[test]
fn change_detection_is_right_in_rebuilt_archetypes_and_grown_pooled_buffers() {
    let mut w = World::new();
    w.track_changes::<Value>();
    // An archetype with room for 16 rows, emptied and then reclaimed: its
    // buffers wait in the world's pool.
    let first: Vec<Entity> = (0..16)
        .map(|i| {
            let e = w.spawn((Value(i),));
            insert_markers(&mut w, e, 0b1);
            e
        })
        .collect();
    for e in first {
        w.despawn(e);
    }
    // Sets without `M<0>`: that archetype stays empty and is reclaimed.
    visit(&mut w, (2..256).filter(|m| m & 1 == 0));
    // A new archetype with the same `Value` column, a tick later (so stamps
    // left in a pooled buffer would be told apart): 8 rows, a tick, then 8
    // more, which grow it into a pooled buffer and carry the stamps along.
    let before = w.advance_change_tick();
    let early: Vec<Entity> = (0..8)
        .map(|i| {
            let e = w.spawn((Value(100 + i),));
            insert_markers(&mut w, e, 0b10);
            e
        })
        .collect();
    let tick = w.advance_change_tick();
    let late: Vec<Entity> = (0..8)
        .map(|i| {
            let e = w.spawn((Value(200 + i),));
            insert_markers(&mut w, e, 0b10);
            e
        })
        .collect();
    let sorted = |mut v: Vec<Entity>| {
        v.sort();
        v
    };
    let added = w
        .query::<Entity>()
        .filter::<(Added<Value>, With<M<1>>)>()
        .since(tick)
        .iter()
        .collect();
    assert_eq!(sorted(added), sorted(late.clone()));
    let added = w
        .query::<Entity>()
        .filter::<(Added<Value>, With<M<1>>)>()
        .since(before)
        .iter()
        .collect();
    assert_eq!(sorted(added), sorted([&early[..], &late[..]].concat()));
    let tick = w.advance_change_tick();
    w.get_mut::<Value>(early[3]).unwrap().0 += 1;
    let changed: Vec<Entity> = w
        .query::<Entity>()
        .filter::<Changed<Value>>()
        .since(tick)
        .iter()
        .collect();
    assert_eq!(changed, [early[3]]);
    let values: u64 = w.query::<&Value>().iter().map(|v| v.0).sum();
    assert_eq!(values, (100..108).sum::<u64>() + 1 + (200..208).sum::<u64>());
    // Removals are logged by entity, whatever archetype they left.
    let tick = w.advance_change_tick();
    w.despawn(late[0]);
    visit(&mut w, 0..256);
    let mut removed = w.removed_since::<Value>(tick).to_vec();
    removed.retain(|&e| e == late[0]);
    assert_eq!(removed, [late[0]]);
}

#[test]
fn bundle_archetypes_are_never_reclaimed() {
    let mut w = World::new();
    let ids = {
        let mut ids = vec![w.register::<Value>(), w.register::<M<5>>()];
        ids.sort();
        ids
    };
    let e = w.spawn((Value(1), M::<5>));
    let slot = w
        .raw_ref()
        .archetypes()
        .iter()
        .position(|a| a.component_ids() == ids)
        .unwrap();
    w.despawn(e);
    visit(&mut w, 0..256);
    visit(&mut w, (0..256).rev());
    assert_eq!(w.raw_ref().archetypes()[slot].component_ids(), ids);
    let e = w.spawn((Value(2), M::<5>));
    assert_eq!(w.get::<Value>(e), Some(&Value(2)));
    assert!(w.has::<M<5>>(e));
}
