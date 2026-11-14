//! Lantern Yard, the game the guide's "Tutorial: a first 3D game" builds
//! step by step. In a project made with `ponko new --3d`, this file is
//! `src/lib.rs`.
//!
//! Each step of the tutorial is one plugin: [`Yard`] (a glTF scene you can
//! stand on), [`Walker`] (a character controller and a camera), [`Lanterns`]
//! (point lights), [`Bodies`] (physics: crates to push and sensors that light
//! the lanterns) and [`Hud`] (UI). The tests in `tests/tutorial_3d.rs` walk
//! the finished game headlessly.

// ANCHOR: use_yard
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{MaterialDesc, Mesh, MeshData};
// ANCHOR_END: use_yard
// ANCHOR: use_hud
use ponko::ui::{UiEvent, UiLayout};
// ANCHOR_END: use_hud

// ANCHOR: game
/// The window title.
pub const TITLE: &str = "Lantern Yard";

/// The whole game: one plugin per step of the tutorial, ready to `run()`
/// in a window or `run_headless(ticks)`.
pub fn game() -> ConfiguredGame {
    Game::new(TITLE, 1280, 720)
        .configured() // the plugins hold all of the setup
        .plugin(Yard) // step 1: a glTF scene, solid, under the sun
        .plugin(Walker) // step 2: a character controller and a camera
        .plugin(Lanterns) // step 3: point lights
        .plugin(Bodies) // step 4: physics bodies and sensors
        .plugin(Hud) // step 5: UI
}
// ANCHOR_END: game

// ANCHOR: yard
/// The scene file, made in a 3D editor (Blender or any other that exports glTF).
pub const YARD: &str = "tutorial_3d/yard.glb";

/// Step 1: the yard, drawn and solid, lit by the sun.
pub struct Yard;

impl Plugin for Yard {
    fn build(&self, game: Game) -> Game {
        game.physics3d(vec3(0.0, -9.81, 0.0)) // gravity in m/s², +Y is up
    }

    fn setup(&self, ctx: &mut Ctx) {
        // What you see: every mesh and material in the file.
        let look = ctx.gfx.load_mesh(YARD);
        // What you stand on: the same triangles as a collision mesh.
        let solid = ctx.load_collision_mesh(YARD).expect("the yard loads");
        ctx.world.spawn((
            Transform::default(),
            Model::new(look),
            RigidBody::fixed(),
            Collider::mesh(solid),
        ));

        // A low evening sun with shadows, and a camera to start from.
        let lighting = &mut ctx.scene3d.lighting;
        lighting.sun_direction = vec3(-0.5, -0.6, -0.4);
        lighting.sun_color = Color::rgb(1.0, 0.85, 0.7);
        ctx.scene3d.camera.position = vec3(0.0, 12.0, 22.0);
        ctx.scene3d.camera.look_at(Vec3::ZERO);
    }
}
// ANCHOR_END: yard

// ANCHOR: bindings
/// Where the walker starts, at its feet.
pub const START: Vec3 = Vec3::new(0.0, 0.0, 6.0);
/// Walking speed in m/s.
pub const WALK_SPEED: f32 = 4.0;
/// How fast a jump starts, in m/s.
pub const JUMP_SPEED: f32 = 5.0;
/// How fast the view turns, in radians per second.
pub const TURN_SPEED: f32 = 2.5;

/// The walker: its entity and the way it faces (0 looks along -Z).
#[derive(Clone, Copy)]
pub struct Player {
    pub entity: Entity,
    pub yaw: f32,
}

// The walker's actions, looked up by name once.
actions! {
    pub struct Act {
        movement = "move",
        turn = "turn",
        jump = "jump",
    }
}

/// Keys and a gamepad, bound to the same actions.
pub fn bindings() -> ActionMap {
    use ponko::input::{Gamepad, GamepadAxis, GamepadButton, GamepadStick};
    let pad = Gamepad::from_slot(0).expect("slot 0 exists");
    let mut map = ActionMap::default();
    map.set(
        "move",
        Action::Axis2(vec![
            Axis2Binding::Keys {
                left: Key::A,
                right: Key::D,
                up: Key::W,
                down: Key::S,
            },
            Axis2Binding::Keys {
                left: Key::Left,
                right: Key::Right,
                up: Key::Up,
                down: Key::Down,
            },
            Axis2Binding::Gamepad {
                pad,
                stick: GamepadStick::Left,
                deadzone: 0.2,
            },
        ]),
    );
    map.set(
        "turn",
        Action::Axis(vec![
            AxisBinding::Keys {
                negative: Key::Q,
                positive: Key::E,
            },
            AxisBinding::Gamepad {
                pad,
                axis: GamepadAxis::RightStickX,
                deadzone: 0.2,
            },
        ]),
    );
    map.set(
        "jump",
        Action::Button(vec![
            ButtonBinding::Key(Key::Space),
            ButtonBinding::Gamepad {
                pad,
                button: GamepadButton::South,
            },
        ]),
    );
    map
}

// ANCHOR_END: bindings

// ANCHOR: walker
/// Step 2: a walking character, and a camera behind it.
pub struct Walker;

impl Plugin for Walker {
    fn build(&self, game: Game) -> Game {
        game.system(control).system(follow)
    }

    fn setup(&self, ctx: &mut Ctx) {
        ctx.input.set_actions(bindings());
        let act = Act::resolve(ctx);
        ctx.world.insert_resource(act);

        // A capsule 1.8 m tall; its transform is at its feet.
        let walker = Character::new(1.8, 0.35);
        // Its look: a stretched sphere standing on the origin.
        let mut body = MeshData::sphere(0.5, 24);
        for p in &mut body.positions {
            *p = *p * vec3(0.7, 1.8, 0.7) + Vec3::Y * 0.9;
        }
        body.normals.clear(); // worked out again from the new shape
        let blue = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x3d7bd9)));
        let mesh = ctx.gfx.create_mesh(&body, blue);
        let entity = ctx.world.spawn((Transform::at(START), walker, Model::new(mesh)));
        ctx.world.insert_resource(Player { entity, yaw: 0.0 });
    }
}

/// Frame system: the actions turn the view and walk the way it faces.
fn control(ctx: &mut Ctx) {
    let act = *ctx.world.resource::<Act>();
    let movement = ctx.input.action_axis2(act.movement); // y is down: W is -1
    let turn = ctx.input.action_axis(act.turn);
    let jump = ctx.input.action_pressed(act.jump);
    let dt = ctx.time.dt();
    let player = ctx.world.resource_mut::<Player>();
    player.yaw -= turn * TURN_SPEED * dt;
    let Player { entity, yaw } = *player;

    let facing = Quat::from_rotation_y(yaw);
    let wish = facing * vec3(movement.x, 0.0, movement.y);
    if let Some(c) = ctx.world.get_mut::<Character>(entity) {
        c.velocity.x = wish.x * WALK_SPEED;
        c.velocity.z = wish.z * WALK_SPEED;
        if jump && c.grounded {
            c.velocity.y = JUMP_SPEED;
        }
    }
    // Physics owns the position; the rotation is ours.
    if let Some(t) = ctx.world.get_mut::<Transform>(entity) {
        t.rotation = facing;
    }
}

/// Frame system: the camera stays behind the walker and looks at its head.
fn follow(ctx: &mut Ctx) {
    let Player { entity, yaw } = *ctx.world.resource::<Player>();
    if let Some(t) = ctx.world.get::<Transform>(entity) {
        let behind = Quat::from_rotation_y(yaw) * vec3(0.0, 3.0, 6.5);
        ctx.scene3d.camera.position = t.position + behind;
        ctx.scene3d.camera.look_at(t.position + Vec3::Y * 1.5);
    }
}
// ANCHOR_END: walker

// ANCHOR: lanterns
/// Where the lanterns stand.
pub const LANTERN_SPOTS: [Vec3; 5] = [
    Vec3::new(7.0, 1.5, -7.0),   // on the platform
    Vec3::new(-8.0, 1.0, -4.5),  // on the ledge up the stairs
    Vec3::new(0.0, 0.0, -12.0),  // behind the pillar
    Vec3::new(-12.0, 0.0, 11.0), // in a corner
    Vec3::new(12.0, 0.0, 11.0),  // in the other corner
];

/// A lantern: its glass (the entity with the light) and whether it is lit.
#[derive(Component)]
pub struct Lantern {
    pub glass: Entity,
    pub lit: bool,
}

/// The lanterns' two looks.
#[derive(Clone, Copy)]
pub struct LanternLooks {
    pub dark: Mesh,
    pub lit: Mesh,
}

/// How bright a lit lantern's light is.
pub const LANTERN_LIGHT: f32 = 25.0;

/// Step 3: lanterns, each a post and a glass with a point light in it.
pub struct Lanterns;

impl Plugin for Lanterns {
    fn build(&self, game: Game) -> Game {
        game
    }

    fn setup(&self, ctx: &mut Ctx) {
        let iron = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x2b2b30)));
        let post = ctx.gfx.create_mesh(&MeshData::cube(1.0), iron);
        let glass = MeshData::sphere(0.25, 16);
        let dark = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0x55606b)));
        let glow = MaterialDesc::color(Color::rgb(1.0, 0.85, 0.5)).with_emissive(Color::rgb(1.0, 0.75, 0.35)); // shines without light on it
        let glow = ctx.gfx.create_material(glow);
        let looks = LanternLooks {
            dark: ctx.gfx.create_mesh(&glass, dark),
            lit: ctx.gfx.create_mesh(&glass, glow),
        };
        ctx.world.insert_resource(looks);

        for at in LANTERN_SPOTS {
            let glass = ctx.world.spawn((
                Transform::at(at + Vec3::Y * 1.45),
                Model::new(looks.dark),
                PointLight {
                    color: Color::rgb(1.0, 0.7, 0.35),
                    intensity: 0.0, // dark until lit
                    range: 8.0,
                    ..Default::default()
                },
            ));
            ctx.world.spawn((
                Transform {
                    scale: vec3(0.15, 1.2, 0.15),
                    ..Transform::at(at + Vec3::Y * 0.6)
                },
                Model::new(post),
                Lantern { glass, lit: false },
            ));
        }
    }
}

/// Lights the lantern `post` (or puts it out).
pub fn set_lit(ctx: &mut Ctx, post: Entity, lit: bool) {
    let looks = *ctx.world.resource::<LanternLooks>();
    let Some(lantern) = ctx.world.get_mut::<Lantern>(post) else {
        return;
    };
    lantern.lit = lit;
    let glass = lantern.glass;
    if let Some(light) = ctx.world.get_mut::<PointLight>(glass) {
        light.intensity = if lit { LANTERN_LIGHT } else { 0.0 };
    }
    let look = if lit { looks.lit } else { looks.dark };
    ctx.world.insert(glass, Model::new(look));
}

/// How many lanterns are lit, and how many there are.
pub fn lanterns_lit(world: &mut World) -> (usize, usize) {
    let lanterns: Vec<bool> = world.query::<&Lantern>().iter().map(|l| l.lit).collect();
    (lanterns.iter().filter(|&&lit| lit).count(), lanterns.len())
}
// ANCHOR_END: lanterns

// ANCHOR: bodies
/// Step 4: crates that fall and can be pushed, and a sensor around each
/// lantern that lights it when the walker steps in.
pub struct Bodies;

impl Plugin for Bodies {
    fn build(&self, game: Game) -> Game {
        game.sim_system(light_touched_lanterns)
    }

    fn setup(&self, ctx: &mut Ctx) {
        let wood = ctx.gfx.create_material(MaterialDesc::color(Color::hex(0xb07a43)));
        let crate_mesh = ctx.gfx.create_mesh(&MeshData::cube(0.8), wood);
        for (i, at) in [vec3(3.0, 0.4, 2.0), vec3(4.0, 0.4, 2.0), vec3(3.5, 1.5, 2.0)]
            .into_iter()
            .enumerate()
        {
            ctx.world.spawn((
                Transform::at(at + Vec3::Y * i as f32 * 0.1),
                Model::new(crate_mesh),
                RigidBody::dynamic(),
                Collider::cuboid(Vec3::splat(0.8)).density(150.0).friction(0.7),
                Velocity::default(),
            ));
        }

        // A sensor reports overlaps and pushes nothing.
        let posts: Vec<Entity> = ctx.world.query::<(Entity, &Lantern)>().iter().map(|(e, _)| e).collect();
        for post in posts {
            ctx.world.insert(post, RigidBody::fixed());
            ctx.world.insert(post, Collider::sphere(0.9).sensor());
        }
    }
}

/// Sim system, after the physics step: a lantern the walker touched lights up.
fn light_touched_lanterns(ctx: &mut Ctx) {
    let walker = ctx.world.resource::<Player>().entity;
    let touched: Vec<Entity> = ctx
        .events
        .read::<CollisionStarted>()
        .iter()
        .filter(|c| c.sensor && (c.a == walker || c.b == walker))
        .map(|c| if c.a == walker { c.b } else { c.a })
        .collect();
    for post in touched {
        if ctx.world.get::<Lantern>(post).is_some_and(|l| !l.lit) {
            set_lit(ctx, post, true);
        }
    }
}
// ANCHOR_END: bodies

// ANCHOR: hud
/// The HUD's nodes.
#[derive(Clone, Copy)]
pub struct HudUi {
    pub count: UiId,
    pub done: UiId,
    pub again: UiId,
}

/// Step 5: a counter in the corner, and a panel when every lantern is lit.
pub struct Hud;

impl Plugin for Hud {
    fn build(&self, game: Game) -> Game {
        game.system(hud)
    }

    fn setup(&self, ctx: &mut Ctx) {
        let ui = &mut ctx.ui;
        // A HUD for the counter: always shown, under every screen.
        let corner = ui.root().layout(UiLayout::fill().padding(16.0)).id();
        let count = ui
            .label(corner, "")
            .layout(UiLayout::fixed(320.0, 32.0))
            .name("hud.count")
            .id();
        ui.show_hud(corner);

        // The panel, pushed on top once every lantern is lit.
        let done = ui
            .root()
            .layout(UiLayout::centered(380.0, 170.0).padding(20.0).gap(12.0))
            .name("hud.done")
            .id();
        ui.title(done, "Every lantern is lit!").layout(UiLayout::row(40.0));
        let again = ui
            .button(done, "Play again")
            .layout(UiLayout::row(44.0))
            .name("hud.again")
            .focused() // Enter plays again
            .id();
        ctx.world.insert_resource(HudUi { count, done, again });
    }
}

/// Frame system: keeps the counter current, opens the panel at the end and
/// starts over when asked.
fn hud(ctx: &mut Ctx) {
    let ids = *ctx.world.resource::<HudUi>();
    let mut again = false;
    for event in ctx.ui.events() {
        match event {
            UiEvent::Clicked(id) if id == ids.again => again = true,
            // Escape closes the panel: play again too.
            UiEvent::ScreenPopped(id) if id == ids.done => again = true,
            _ => {}
        }
    }
    if again {
        if ctx.ui.is_shown(ids.done) {
            ctx.ui.pop_screen();
        }
        start_over(ctx);
    }
    let (lit, all) = lanterns_lit(&mut ctx.world);
    ctx.ui.set_text(ids.count, format!("Lanterns lit: {lit} / {all}"));
    if lit == all && !ctx.ui.is_shown(ids.done) {
        ctx.ui.push_screen(ids.done, true);
    }
}

/// Puts every lantern out and the walker back at the start.
pub fn start_over(ctx: &mut Ctx) {
    let posts: Vec<Entity> = ctx.world.query::<(Entity, &Lantern)>().iter().map(|(e, _)| e).collect();
    for post in posts {
        set_lit(ctx, post, false);
    }
    let player = ctx.world.resource_mut::<Player>();
    player.yaw = 0.0;
    let walker = player.entity;
    if let Some(t) = ctx.world.get_mut::<Transform>(walker) {
        t.position = START;
    }
}
// ANCHOR_END: hud
