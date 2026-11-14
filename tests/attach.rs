//! Attachments: entities that follow another entity.

#[allow(unused_imports)]
use ponko::d2::Transform as Transform2D;
use ponko::prelude::*;

#[derive(Component)]
struct Mover;

fn move_right(ctx: &mut Ctx) {
    ctx.world
        .query::<(&mut Transform2D, &Mover)>()
        .for_each(|(t, _)| t.position.x += 1.0);
}

fn game() -> Game {
    Game::new("attach", 1, 1)
        .sim_system(move_right)
        .sim_system(systems::follow_attachments)
}

fn position(world: &World, e: Entity) -> Vec2 {
    world.get::<Transform2D>(e).unwrap().position
}

#[test]
fn attachments_follow_their_entity_every_tick() {
    let (mut tank, mut bar) = (None, None);
    let world = game().run_headless(5, |ctx| {
        let t = ctx.world.spawn((Transform2D::at(vec2(100.0, 50.0)), Mover));
        bar = Some(ctx.world.spawn((
            Transform2D::default(),
            ponko::attach::Attached::to(t).offset(vec2(0.0, -10.0)),
        )));
        tank = Some(t);
    });
    assert_eq!(position(&world, tank.unwrap()), vec2(105.0, 50.0));
    assert_eq!(position(&world, bar.unwrap()), vec2(105.0, 40.0));
}

#[test]
fn offsets_turn_and_mirror_with_their_entity_but_scale_stays_own() {
    let mut ids = None;
    let world = game().run_headless(1, |ctx| {
        let turned = Transform2D::at(vec2(0.0, 0.0)).rotated(std::f32::consts::PI);
        let flipped = Transform2D::at(vec2(50.0, 0.0)).scaled(vec2(-1.0, 1.0));
        let a = ctx.world.spawn((turned,));
        let b = ctx.world.spawn((flipped,));
        let small = Transform2D::default().scaled(Vec2::splat(0.5));
        let on_a = ctx.world.spawn((
            small,
            ponko::attach::Attached::to(a).offset(vec2(4.0, 0.0)).rotated(0.25),
        ));
        let on_b = ctx.world.spawn((
            Transform2D::default(),
            ponko::attach::Attached::to(b).offset(vec2(4.0, 0.0)),
        ));
        ids = Some((on_a, on_b));
    });
    let (on_a, on_b) = ids.unwrap();
    let t = world.get::<Transform2D>(on_a).unwrap();
    assert!((t.position - vec2(-4.0, 0.0)).length() < 1e-4, "{}", t.position);
    assert!((t.rotation - (std::f32::consts::PI + 0.25)).abs() < 1e-6);
    assert_eq!(t.scale, Vec2::splat(0.5));
    assert_eq!(position(&world, on_b), vec2(46.0, 0.0));
}

#[test]
fn attachments_go_when_their_entity_goes_down_the_whole_chain() {
    let mut ids = None;
    let world = Game::new("attach", 1, 1)
        .sim_system(|ctx: &mut Ctx| {
            if ctx.time.sim_tick() == 1 {
                let doomed = ctx.world.resource::<Doomed>().0;
                ctx.world.despawn(doomed);
            }
        })
        .sim_system(systems::follow_attachments)
        .run_headless(3, |ctx| {
            let unit = ctx.world.spawn((Transform2D::default(),));
            let ring = ctx
                .world
                .spawn((Transform2D::default(), ponko::attach::Attached::to(unit)));
            let label = ctx
                .world
                .spawn((Transform2D::default(), ponko::attach::Attached::to(ring)));
            let other = ctx.world.spawn((Transform2D::default(),));
            let kept = ctx
                .world
                .spawn((Transform2D::default(), ponko::attach::Attached::to(other)));
            ctx.world.insert_resource(Doomed(unit));
            ids = Some((ring, label, kept));
        });
    let (ring, label, kept) = ids.unwrap();
    assert!(!world.is_alive(ring) && !world.is_alive(label));
    assert!(world.is_alive(kept));
}

struct Doomed(Entity);

#[test]
fn many_attachments_are_placed_in_parallel() {
    let world = game().run_headless(3, |ctx| {
        for i in 0..50_000 {
            let base = ctx.world.spawn((Transform2D::at(vec2(i as f32, 0.0)), Mover));
            ctx.world.spawn((
                Transform2D::default(),
                ponko::attach::Attached::to(base).offset(vec2(0.0, 1.0)),
                Child(i),
            ));
        }
    });
    let mut checked = 0;
    let mut world = world;
    world.query::<(&Transform2D, &Child)>().for_each(|(t, c)| {
        assert_eq!(t.position, vec2(c.0 as f32 + 3.0, 1.0));
        checked += 1;
    });
    assert_eq!(checked, 50_000);
}

#[derive(Component)]
struct Child(u32);

// ANCHOR: attach
#[derive(Component)]
struct Tank {
    speed: f32,
}

fn drive(ctx: &mut Ctx) {
    ctx.world
        .query::<(&mut Transform2D, &Tank)>()
        .for_each(|(t, tank)| t.position.x += tank.speed);
}

#[test]
fn turrets_ride_on_tanks() {
    let mut turret = None;
    let world = Game::new("tanks", 800, 600)
        .sim_system(drive)
        .sim_system(systems::follow_attachments) // after the systems that move things
        .run_headless(10, |ctx| {
            let tank = ctx.world.spawn((Transform2D::at(vec2(0.0, 0.0)), Tank { speed: 1.0 }));
            let on_top = ponko::attach::Attached::to(tank).offset(vec2(0.0, -4.0));
            turret = Some(ctx.world.spawn((Transform2D::default(), on_top)));
        });
    assert_eq!(position(&world, turret.unwrap()), vec2(10.0, -4.0));
}
// ANCHOR_END: attach

#[test]
fn attachments_are_reflected_for_tools_and_saves() {
    let mut bar = None;
    let world = game().run_headless(0, |ctx| {
        let unit = ctx.world.spawn((Transform2D::default(),));
        bar = Some(ctx.world.spawn((
            Transform2D::default(),
            ponko::attach::Attached::to(unit).offset(vec2(0.0, -3.0)),
        )));
    });
    let json = world.get_json(bar.unwrap(), "Attached").unwrap();
    assert_eq!(json["offset"], serde_json::json!([0.0, -3.0]));
    assert!(json.get("to").is_some(), "{json}");
}
