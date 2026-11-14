// The C++ code shown in the guide (book/src), next to the Rust versions.
// Everything here compiles in CI; main() runs the headless parts as a test.
#include "ponko.hpp"
#include <algorithm>
#include <array>
#include <cassert>
#include <cmath>
#include <cstdio>
#include <future>
#include <map>
#include <optional>
#include <string>
#include <vector>
#include <thread>
#include <chrono>

using namespace ponko;

// ---- Getting started --------------------------------------------------------
// ANCHOR: first_game_full
static void my_game() {
    ponko::texture crab{};
    ponko::vec2 pos{400, 300};
    ponko::game("My Game", 800, 600)
        .system([&](ponko::ctx& ctx) {
            if (ctx.key_down(PONKO_KEY_RIGHT)) pos.x += 200 * ctx.dt();
            if (ctx.key_down(PONKO_KEY_LEFT))  pos.x -= 200 * ctx.dt();
            if (ctx.key_pressed(PONKO_KEY_ESCAPE)) ctx.quit();
            ctx.draw(crab, pos);
        })
        .run([&](ponko::ctx& ctx) { crab = ctx.load_texture("crab.png"); });
}
// ANCHOR_END: first_game_full

// ANCHOR: action_map
static void action_map_snippet() {
    ponko::action_map bindings;
    bindings.set("jump", ponko::action::button({
        ponko::action_binding::key(PONKO_KEY_SPACE),
        ponko::action_binding::button({0}, PONKO_GAMEPAD_SOUTH)}));
    bindings.set("move", ponko::action::axis2({
        ponko::action_binding::keys2(PONKO_KEY_A, PONKO_KEY_D, PONKO_KEY_W, PONKO_KEY_S),
        ponko::action_binding::stick({0}, ponko::stick::left, 0.2f)}));
    bindings.set("throttle", ponko::action::axis({
        ponko::action_binding::axis({0}, PONKO_GAMEPAD_RIGHT_TRIGGER_AXIS, 0.1f)}));
    const std::string saved = bindings.to_json();
    auto restored = ponko::action_map::from_json(saved);
    ponko::game("actions", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        ctx.set_actions(restored);
        ctx.feed_input(ponko::input_event::key(PONKO_KEY_SPACE, true));
        if (!ctx.action_pressed("jump") || !ctx.action_down("jump")) throw std::runtime_error("missing action press");
        const ponko::vec2 move = ctx.action_axis2("move");
        const float throttle = ctx.action_axis("throttle");
        if (move.x != 0 || move.y != 0 || throttle != 0) throw std::runtime_error("unexpected axis input");
        ctx.feed_input(ponko::input_event::key(PONKO_KEY_SPACE, false));
        if (!ctx.action_released("jump")) throw std::runtime_error("missing action release");
        auto changed = ctx.actions();
        changed.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_ENTER)}));
        ctx.set_actions(changed);
        if (ctx.action_pressed("jump")) throw std::runtime_error("rebinding generated a press");
    });
}
// ANCHOR_END: action_map

// ANCHOR: pressed_this_frame
// A rebinding prompt: bind whatever button-like input the player presses next.
static bool rebind_jump(ponko::ctx& ctx) {
    for (const ponko::pressed_input& pressed : ctx.pressed_this_frame()) {
        if (pressed.is_gamepad_axis()) continue;  // axes bind with action::axis
        auto bindings = ctx.actions();
        bindings.set("jump", ponko::action::button({pressed.binding()}));
        ctx.set_actions(bindings);
        return true;
    }
    return false;
}
// ANCHOR_END: pressed_this_frame

// ANCHOR: set_action
// Give jump the player's new key, keeping its other sources.
static void rebind_jump_to_k(ponko::ctx& ctx) {
    ponko::action jump = ctx.actions().get("jump").value_or(ponko::action::button({}));
    std::vector<PonkoActionBinding> sources{ponko::action_binding::key(PONKO_KEY_K).raw};
    for (const PonkoActionBinding& s : jump.bindings)
        if (s.kind != PONKO_ACTION_KEY) sources.push_back(s);
    jump.bindings = sources;
    ctx.set_action("jump", jump);
}
// ANCHOR_END: set_action

static void pressed_this_frame_snippet() {
    ponko::game("rebind", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ponko::action_map bindings;
        bindings.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_SPACE)}));
        ctx.set_actions(bindings);
        if (rebind_jump(ctx)) throw std::runtime_error("nothing was pressed yet");
        ctx.feed_input(ponko::input_event::key(PONKO_KEY_K, true));
        if (!rebind_jump(ctx)) throw std::runtime_error("the press was not reported");
        if (!ctx.action_down("jump")) throw std::runtime_error("jump was not rebound");
    });
    ponko::game("set action", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ponko::action_map bindings;
        bindings.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_SPACE),
                                                    ponko::action_binding::button({0}, PONKO_GAMEPAD_SOUTH)}));
        ctx.set_actions(bindings);
        const auto id = ctx.action_id("jump");
        rebind_jump_to_k(ctx);
        const auto jump = ctx.actions().get("jump");
        if (!jump || jump->bindings.size() != 2 || jump->bindings[0].first != static_cast<std::uint32_t>(PONKO_KEY_K) ||
            jump->bindings[1].kind != PONKO_ACTION_GAMEPAD_BUTTON || ctx.action_id("jump") != id)
            throw std::runtime_error("set_action did not replace jump's key");
    });
}

// ---- Entities, components, queries -----------------------------------------
// ANCHOR: ecs_components
struct Health { float value; };
struct Enemy {};
// ANCHOR_END: ecs_components

static void ecs(ponko::ctx& ctx) {
    auto world = ctx.world();
    // ANCHOR: ecs_spawn
    ponko::entity e = world.spawn(transform_at({10, 20}), Health{100}, Enemy{});

    // Many at once:
    for (int i = 0; i < 100000; ++i)
        world.spawn(transform_at({float(i), 0}), Health{10});
    // ANCHOR_END: ecs_spawn

    // ANCHOR: ecs_one
    Health h = world.get<Health>(e);
    world.set(e, Health{h.value - 5});
    world.set(e, Enemy{});            // add or replace a component
    if (world.has<Enemy>(e)) world.remove<Enemy>(e);
    world.despawn(e);
    assert(!world.alive(e));          // old handles stay dead, even if the slot is reused
    assert(!world.try_get<Health>(e));
    // ANCHOR_END: ecs_one

    // ANCHOR: ecs_query
    world.each([](transform2d& t, const Health& hp) {
        t.position.y += hp.value;
    });
    // ANCHOR_END: ecs_query

    // ANCHOR: ecs_columns
    world.query<transform2d, Health>([](auto chunk) {
        auto [t, hp] = chunk.columns();   // one array per component
        for (std::size_t i = 0; i < chunk.size(); ++i)
            t[i].position.y += hp[i].value;
    });
    // ANCHOR_END: ecs_columns
}

// ANCHOR: ecs_commands
static void die(ponko::ctx& ctx) {
    ctx.world().each([&](ponko::entity e, const Health& hp) {
        if (hp.value <= 0) ctx.despawn_later(e);
    });
}
// ANCHOR_END: ecs_commands

// ANCHOR: ecs_changes
struct HealthBar { float width; };

static void resize_health_bars(ponko::ctx& ctx) {
    auto world = ctx.world();
    // Only the entities whose Health changed since this system last ran.
    world.each<ponko::changed<Health>>([](const Health& hp, HealthBar& bar) {
        bar.width = hp.value;
    });
    // The ones that are new, and the ones that lost it (or despawned).
    std::size_t spawned = world.count<ponko::added<Health>>();
    std::vector<ponko::entity> gone = world.removed<Health>();
    (void)spawned; (void)gone;
}
[[maybe_unused]] static void regenerate(ponko::ctx& ctx) {
    // `Health&` can write, so every entity visited counts as changed.
    // Take `const Health&` (or a copy) when you only read.
    ctx.world().each([](Health& hp) { hp.value += 1; });
}
// ANCHOR_END: ecs_changes

// ANCHOR: ecs_filters
struct Ally {};
struct Dead {};
// Living fighters: allies or enemies that aren't dead.
using Living = ponko::all_of<ponko::or_<ponko::with<Ally>, ponko::with<Enemy>>, ponko::without<Dead>>;
// An AND group inside an or_: both Ally and Enemy, or not Dead.
using Odd = ponko::or_<ponko::all_of<ponko::with<Ally>, ponko::with<Enemy>>, ponko::without<Dead>>;

static std::size_t fight(ponko::ctx& ctx) {
    auto world = ctx.world();
    world.each<Living>([](Health& hp) { hp.value -= 1; });
    return world.count<Odd>();
}
// ANCHOR_END: ecs_filters

static void ecs_filters(ponko::ctx& ctx) {
    auto world = ctx.world();
    const auto ally = world.spawn(Health{10}, Ally{});
    const auto foe = world.spawn(Health{10}, Enemy{});
    const auto corpse = world.spawn(Health{10}, Enemy{}, Dead{});
    const auto bystander = world.spawn(Health{10});
    const std::size_t odd = fight(ctx);
    assert(world.get<Health>(ally).value == 9 && world.get<Health>(foe).value == 9);
    assert(world.get<Health>(corpse).value == 10 && world.get<Health>(bystander).value == 10);
    assert(odd == 3);   // everything that isn't dead
}

// ANCHOR: ecs_query_cache
struct Boss {};

static std::size_t bosses(ponko::ctx& ctx) {
    auto world = ctx.world();
    // Runs rarely, but must not test every archetype of a big world when
    // it does: index it on first use.
    return world.count<Health, ponko::with<Boss>, ponko::cached>();
}
static std::size_t bosses_by_scan(ponko::ctx& ctx) {
    // Never use the index for this query: the same entities, found by
    // testing every archetype.
    return ctx.world().count<Health, ponko::with<Boss>, ponko::uncached>();
}
static void no_index(ponko::ctx& ctx) {
    // Or opt the whole world out.
    ctx.world().set_query_cache_policy(ponko_query_cache_policy_off());
}
// ANCHOR_END: ecs_query_cache

static void ecs_query_cache(ponko::ctx& ctx) {
    auto world = ctx.world();
    world.spawn(Health{100}, Boss{});
    world.spawn(Health{10});
    assert(bosses(ctx) == 1 && bosses_by_scan(ctx) == 1);
    no_index(ctx);
    assert(bosses(ctx) == 1 && world.query_cache_stats().signatures == 0);
}

// ANCHOR: resources
struct Score { std::uint32_t points; };

[[maybe_unused]] static void score_point(ponko::ctx& ctx) {
    auto world = ctx.world();
    Score score = world.try_resource<Score>().value_or(Score{0});
    score.points += 1;
    world.set_resource(score);
}
// ANCHOR_END: resources

// ---- Systems -----------------------------------------------------------------
[[maybe_unused]] static void steer(ponko::ctx&) {}
[[maybe_unused]] static void movement(ponko::ctx&) {}
[[maybe_unused]] static void camera_system(ponko::ctx&) {}

[[maybe_unused]] static void systems() {
// ANCHOR: systems
ponko::game("Sim", 1280, 720)
    .sim_system(steer, "steer")          // fixed rate: game logic
    .sim_system(movement, "movement")
    .system(camera_system, "camera")     // once per frame: input, camera, UI
    .run([](ponko::ctx&) { /* spawn things */ });
// ANCHOR_END: systems
}

// ANCHOR: stateful
struct Spawner {
    std::uint64_t every;
    int spawned = 0;
    void operator()(ponko::ctx& ctx) {
        if (ctx.sim_tick() % every == 0) {
            ctx.spawn_later(ponko::transform_at({0, 0}));
            ++spawned;
        }
    }
};
// ponko::game("t", 800, 600).sim_system(Spawner{30}, "spawner");
// ANCHOR_END: stateful

static void replace_builtins() {
    auto my_draw = [](ponko::ctx&) {};
// ANCHOR: remove_system
ponko::game game("t", 800, 600);
game.remove(ponko::systems::draw_sprites)     // draw your own way
    .system(my_draw, "my_draw");
// add_system returns an id for a system of your own.
ponko::system_id hud = game.add_system(ponko::phase::frame, my_draw, "hud");
game.remove(hud);

// Tilemaps over the sprites instead of under them.
ponko::game tiles("t", 800, 600);
tiles.remove(ponko::systems::draw_tilemaps)
    .add_builtin(ponko::phase::draw, ponko::systems::draw_tilemaps);
// ANCHOR_END: remove_system
    if (tiles.schedule_plan().find("draw_tilemaps") == std::string::npos) throw std::runtime_error("no tilemaps");
}

// ANCHOR: builtin_access
static void add_navigation(ponko::game& game) {
    game.add_builtin(ponko::phase::before_physics, ponko::systems::step_navmesh3d) // with its declaration
        .physics3d({0.0f, -9.81f, 0.0f});
}
// Whether `mine` could run beside the navigation step.
static bool beside_navigation(const ponko::access& mine) {
    return mine.conflicts_before(ponko::access::builtin(ponko::systems::step_navmesh3d), false) == 0;
}
// ANCHOR_END: builtin_access

// ANCHOR: plugin
// One area of the game: its systems and its startup work.
struct PlayerArea {
    float speed = 4;
    void build(ponko::game& game) const {
        game.add(ponko::phase::before_physics, [speed = speed](ponko::ctx& ctx) {
            ctx.world().each([&](ponko::transform2d& t) { t.position.x += speed * ctx.dt(); });
        }, "player/walk");
    }
    void setup(ponko::ctx& ctx) const { ctx.world().spawn(ponko::transform_at({0, 0})); }
};
// ANCHOR_END: plugin

[[maybe_unused]] static void plugin_snippets() {
// ANCHOR: plugin_use
ponko::game("Modules", 1280, 720)
    .plugin(PlayerArea{6})
    .setup([](ponko::ctx& ctx) { ctx.world().spawn(ponko::transform_at({10, 0})); })
    .run();
// ANCHOR_END: plugin_use
}

// ANCHOR: builder_setup
[[maybe_unused]] static void builder_setup(bool with_enemies) {
    ponko::game game("My Game", 800, 600);
    game.sim_system([](ponko::ctx& ctx) {
            ctx.world().each([&](ponko::transform2d& t) { t.position.x += 60 * ctx.dt(); });
        }, "movement")
        .setup([](ponko::ctx& ctx) {
            auto crab = ctx.load_texture("crab.png");
            ctx.world().spawn(ponko::transform_at({400, 300}), ponko::sprite_of(crab, ponko::colors::white, 0));
        });
    // Startup chosen at run time: each call adds one more callback.
    if (with_enemies)
        game.setup([](ponko::ctx& ctx) { ctx.world().spawn(ponko::transform_at({100, 100})); });
    game.run();   // or run_headless(ticks)
}
// ANCHOR_END: builder_setup

// The same shape without a window: setup callbacks run in the order added.
static void builder_setup_runs_headless() {
    std::string order;
    ponko::game game("My Game", 1, 1);
    game.sim_system([](ponko::ctx& ctx) {
            ctx.world().each([&](ponko::transform2d& t) { t.position.x += 60 * ctx.dt(); });
        }, "movement")
        .setup([&](ponko::ctx& ctx) { order += 'a'; ctx.world().spawn(ponko::transform_at({400, 300})); });
    game.setup([&](ponko::ctx& ctx) { order += 'b'; ctx.world().spawn(ponko::transform_at({100, 100})); });
    auto world = game.fixed_hz(60).run_headless(60);
    float sum = 0;
    world.each([&](const ponko::transform2d& t) { sum += t.position.x; });
    if (order != "ab" || world.size() != 2 || std::fabs(sum - 620.0f) > 1e-2f) { std::fprintf(stderr, "setup guide\n"); std::abort(); }
}

// The plugin above, run without a window: both entities walk.
static void plugins_run_headless() {
    auto world = ponko::game("Modules", 1, 1).fixed_hz(60)
        .plugin(PlayerArea{6})
        .setup([](ponko::ctx& ctx) { ctx.world().spawn(ponko::transform_at({10, 0})); })
        .run_headless(60);
    float sum = 0;
    world.each([&](const ponko::transform2d& t) { sum += t.position.x; });
    if (world.size() != 2 || std::fabs(sum - 22.0f) > 1e-3f) { std::fprintf(stderr, "plugin guide\n"); std::abort(); }
}

// ---- The simulation clock ---------------------------------------------------
// ANCHOR: controls
static void controls(ponko::ctx& ctx) {
    if (ctx.key_pressed(PONKO_KEY_SPACE)) {
        if (ctx.is_paused()) ctx.resume(); else ctx.pause();
    }
    if (ctx.key_pressed(PONKO_KEY_PERIOD))
        ctx.step();              // one tick while paused
    if (ctx.key_pressed(PONKO_KEY_NUM_4))
        ctx.set_sim_speed(64);   // fast-forward
}
// ponko::game("Colony", 1280, 720).sim_system(ants).system(controls);
// ANCHOR_END: controls

// ANCHOR: physics_pause
[[maybe_unused]] static void pause_menu(ponko::ctx& ctx) {
    if (ctx.key_pressed(PONKO_KEY_ESCAPE))
        ctx.set_physics_paused(!ctx.is_physics_paused()); // the step only
}
// ponko::game("Platformer", 1280, 720).physics({0, 980}).sim_system(pause_menu);
// ANCHOR_END: physics_pause

// ---- Input -------------------------------------------------------------------
// ANCHOR: gamepad
struct Player { std::uint32_t slot; float spin; };   // one per connected pad

static void move_players(ponko::ctx& ctx) {
    const float dt = ctx.dt();
    // Read the pads first: the loop below borrows the world.
    ponko::vec2 sticks[PONKO_MAX_GAMEPADS] = {};
    bool spins[PONKO_MAX_GAMEPADS] = {};
    float grow[PONKO_MAX_GAMEPADS] = {};
    for (ponko::gamepad pad : ctx.gamepads()) {
        sticks[pad.slot] = ctx.gamepad_stick(pad, ponko::stick::left);
        spins[pad.slot] = ctx.gamepad_pressed(pad, PONKO_GAMEPAD_SOUTH);
        grow[pad.slot] = ctx.gamepad_axis(pad, PONKO_GAMEPAD_RIGHT_TRIGGER_AXIS);
    }
    ctx.world().each([&](ponko::transform2d& t, Player& player) {
        t.position += sticks[player.slot] * 400 * dt;
        if (spins[player.slot]) player.spin = 6.2831853f;
        t.scale = ponko::vec2{1, 1} * (1 + 2 * grow[player.slot]);
    });
}
// ANCHOR_END: gamepad

[[maybe_unused]] static void plan_routes(ponko::ctx&) {}
[[maybe_unused]] static void grow_food(ponko::ctx&) {}
[[maybe_unused]] static void every() {
// ANCHOR: every
ponko::game("Colony", 1280, 720)
    .sim_system(movement, "movement")             // every tick
    .sim_system(plan_routes, "plan_routes", 10)   // 6 times a second
    .sim_system(grow_food, "grow_food", 10);      // also 6/s, but on other ticks
// ANCHOR_END: every
}

// ---- Spatial queries -----------------------------------------------------------
// ANCHOR: spatial_component
struct Neighbors { std::uint32_t count; };
// ANCHOR_END: spatial_component

// ANCHOR: spatial
// ponko::game("Boids", 1280, 720).spatial_grid(24).sim_system(count_neighbors);
static void count_neighbors(ponko::ctx& ctx) {
    const auto grid = ctx.spatial_view();
    ctx.world().each_par([&](ponko::entity me, const ponko::transform2d& t, Neighbors& n) {
        n.count = 0;
        grid.for_each_in_radius(t.position, 10, [&](const ponko::grid_hit& o) {
            if (ponko::bits(o.entity) != me) ++n.count;
        });
    });
}
// ANCHOR_END: spatial

// ---- Drawing ---------------------------------------------------------------------
[[maybe_unused]] static void drawing(ponko::ctx& ctx) {
    // ANCHOR: sprites
    auto crab = ctx.load_texture("crab.png");
    auto world = ctx.world();
    world.spawn(ponko::transform_at({400, 300}, 0.3f, {2, 2}),
                ponko::sprite_of(crab, ponko::colors::red, 1));
    // ANCHOR_END: sprites
    // ANCHOR: immediate
    ctx.clear(ponko::rgb(0.125f, 0.19f, 0.25f));
    ctx.draw(crab, {100, 100});
    auto params = ponko::draw_defaults();
    params.rotation = 0.5f;
    params.scale = {2, 2};
    params.color = ponko::colors::red;
    ctx.draw(crab, {200, 100}, params);
    // ANCHOR_END: immediate
    // ANCHOR: pixels
    std::vector<std::uint8_t> rgba(64 * 64 * 4, 255);
    auto heat = ctx.create_texture(64, 64, rgba.data(), rgba.size());
    rgba[0] = 0;   // ...change pixels...
    ctx.update_texture(heat, rgba.data(), rgba.size());
    // ANCHOR_END: pixels
}

// ANCHOR: sprite_materials
/// Burns a sprite away where its noise texture is below `material.params[0].x`.
static const char* dissolve_shader = R"(
fn fragment(p: SpritePixel) -> vec4<f32> {
    if material_sample0(p.local).r < material.params[0].x {
        discard;
    }
    return p.color;
})";

static ponko::sprite_material sprite_looks(ponko::ctx& ctx, ponko::texture crab, ponko::texture noise) {
    auto world = ctx.world();
    // A glow that brightens what's behind it:
    world.spawn(ponko::transform_at({400, 300}), ponko::sprite_of(crab), ponko::sprite_blend::additive);
    // A sprite with a shader of its own:
    auto desc = ponko::sprite_material_of(ctx.create_sprite_shader("dissolve.wgsl", dissolve_shader));
    ponko::set_texture(desc, 0, noise);
    auto dissolve = ctx.create_sprite_material(desc);
    world.spawn(ponko::transform_at({200, 300}), ponko::sprite_of(crab), dissolve);
    // Immediate draws take both too:
    ctx.draw(crab, {100, 100}, ponko::draw_defaults(), ponko::sprite_blend::additive, dissolve);
    return dissolve;
}

/// Burns it a little further (call it every frame).
static void burn(ponko::ctx& ctx, ponko::sprite_material dissolve, float progress) {
    if (auto desc = ctx.sprite_material_desc_of(dissolve)) {
        ponko::set_param(*desc, 0, progress, 0, 0, 0);
        ctx.update_sprite_material(dissolve, *desc);
    }
}
// ANCHOR_END: sprite_materials

/// Runs the sprite material snippet headless and checks what it made.
static void sprite_looks_run() {
    ponko::game("sprite material guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        std::uint8_t white[4] = {255, 255, 255, 255};
        const auto pixel = ctx.create_texture(1, 1, white, sizeof white);
        const auto dissolve = sprite_looks(ctx, pixel, pixel);
        burn(ctx, dissolve, 0.5f);
        const auto desc = ctx.sprite_material_desc_of(dissolve);
        if (!desc || desc->params[0] != 0.5f || !desc->has_textures[0] || desc->has_textures[1]) {
            std::fprintf(stderr, "sprite material guide: params\n");
            std::abort();
        }
        std::size_t glows = 0, burning = 0;
        // A blend mode is checked as it's stored, so it isn't a query column: get it by entity.
        std::vector<ponko::entity> placed;
        ctx.world().each([&](ponko::entity e, const ponko::transform2d&) { placed.push_back(e); });
        for (auto e : placed)
            glows += ctx.world().has<ponko::sprite_blend>(e) &&
                     ctx.world().get<ponko::sprite_blend>(e) == ponko::sprite_blend::additive;
        ctx.world().each([&](const ponko::sprite_material& m) { burning += m.id == dissolve.id; });
        const auto status = ctx.shader_status();
        if (glows != 1 || burning != 1 || status.size() != 1 || status[0].info.kind != PONKO_SHADER_SPRITE) {
            std::fprintf(stderr, "sprite material guide: components or status\n");
            std::abort();
        }
        ctx.free(dissolve);
        if (ctx.is_live(dissolve) || ctx.sprite_material_desc_of(dissolve)) std::abort();
        // A broken shader is an error that says where, in its own lines.
        const auto broken = ctx.try_create_sprite_shader("flash.wgsl", "// white\nfn fragment(p: SpritePixel) -> vec4<f32> {\n    return 1.0;\n}");
        if (broken || ctx.shader_status()[1].info.line != 3) {
            std::fprintf(stderr, "sprite material guide: errors\n");
            std::abort();
        }
    });
}

// ---- Sprite sheets, animation and tilemaps ------------------------------------------
[[maybe_unused]] static void sheets(ponko::ctx& ctx, ponko::vec2 pos) {
    auto world = ctx.world();
    // ANCHOR: sheet
    auto tex = ctx.load_texture("units.png");
    auto sheet = ponko::sheet_of(tex, 32, 32);   // 32 x 32 frames
    world.spawn(ponko::transform_at(pos), ponko::sprite_of(sheet, 5));   // frame 5
    // ANCHOR_END: sheet
    // ANCHOR: nearest
    ctx.set_texture_filter(tex, ponko::texture_filter::nearest);
    // ANCHOR_END: nearest
}

static void animate(ponko::world_view world, ponko::sprite_sheet sheet, ponko::vec2 pos) {
    // ANCHOR: animation
    world.spawn(ponko::transform_at(pos), ponko::sprite_of(sheet, 0),
                ponko::animation_of(sheet, 0, 8, 12));   // frames 0-7 at 12 fps
    auto boom = ponko::animation_of(sheet, 8, 8, 24);
    boom.looping = false;   // stops on the last frame; ponko::finished(boom) says when
    // ANCHOR_END: animation
    (void)boom;
}

constexpr std::uint16_t GRASS = 0, STONE = 3;

// ANCHOR: tilemap
static ponko::entity make_map(ponko::world_view world, ponko::sprite_sheet tiles, std::uint32_t side) {
    const ponko::entity map = world.spawn(ponko::transform_at({0, 0}));
    auto t = world.add_tilemap(map, tiles, side, side);   // the transform is its top-left corner
    for (std::uint32_t y = 0; y < side; ++y)
        for (std::uint32_t x = 0; x < side; ++x)
            t.set(x, y, (x * 7 + y * 3) % 5 == 0 ? STONE : GRASS);
    return map;
}
// ANCHOR_END: tilemap

// ANCHOR: paint
static void paint(ponko::ctx& ctx, ponko::entity map) {
    if (!ctx.mouse_down(PONKO_MOUSE_LEFT) || ctx.mouse_over_ui()) return;
    auto world = ctx.world();
    const ponko::vec2 origin = world.get<ponko::transform2d>(map).position;
    auto tiles = world.tilemap(map);
    if (auto tile = tiles.tile_at(origin, ctx.mouse_world())) tiles.set((*tile)[0], (*tile)[1], STONE);
}
// ANCHOR_END: paint

// ANCHOR: level_spawn
static ponko::entity spawn_meadow(ponko::ctx& ctx) {
    ponko::level_options options;
    options.prefabs = {{"Chest", "prefabs/chest.json"}}; // objects of this class become prefab instances
    options.solid = {"Ground"};                          // tiles of this layer collide too
    return ponko::spawn_level(ctx, "levels/meadow.tmx", {0, 0}, options);
}
// ANCHOR_END: level_spawn

// ANCHOR: level_objects
static ponko::vec2 start_position(ponko::ctx& ctx, ponko::entity level) {
    for (ponko::entity e : ponko::level_entities(ctx, level)) {
        if (!ctx.world().try_get<ponko::transform2d>(e)) continue;
        try {
            if (ponko::level_object_class(ctx, e) == "Spawn") return ponko::level_object_of(ctx, e).position;
        } catch (const ponko::error&) {} // a layer or a collider, not an object
    }
    return {48, 120};
}
// ANCHOR_END: level_objects

// ANCHOR: level_saves
static void save_meadow(ponko::ctx& ctx) {
    ctx.save_world("meadow"); // the chests and the player, not the level: it isn't saved
}

static ponko::entity load_meadow(ponko::ctx& ctx) {
    ctx.load_world("meadow"); // the saved chests and player, as they were...
    ponko::level_options options;
    options.prefabs = {{"Chest", "prefabs/chest.json"}};
    options.solid = {"Ground"};
    options.without_saved_objects = true; // ...and the level around them, from its file
    return ponko::spawn_level(ctx, "levels/meadow.tmx", {0, 0}, options);
}
// ANCHOR_END: level_saves

// ANCHOR: autotile
static void build_wall(ponko::ctx& ctx, ponko::entity walls, std::uint32_t x, std::uint32_t y) {
    ponko::autotile_rule rule;
    rule.frames = {7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7}; // one piece per mask N=1 E=2 S=4 W=8
    ponko::tilemap_paint_autotile(ctx, walls, rule, x, y, true);
    const std::size_t bodies = ponko::tilemap_spawn_colliders(ctx, walls, {7});
    (void)bodies;
}
// ANCHOR_END: autotile

static void levels_guide(ponko::ctx& ctx) {
    const ponko::entity level = spawn_meadow(ctx);
    const ponko::vec2 start = start_position(ctx, level);
    if (start.x != 48.0f) { std::fprintf(stderr, "level start\n"); std::abort(); }
    // ANCHOR: tile_extras
    const ponko::entity sky = ctx.world().spawn(ponko::transform_at({0, 0}));
    ctx.world().add_tilemap(sky, ponko::sheet_of(ctx.load_texture("levels/meadow.png"), 16, 16), 40, 15);
    ponko::tilemap_set_parallax(ctx, sky, {0.3f, 0.3f});                 // far away: scrolls slowly
    ponko::tilemap_animate(ctx, sky, 4, {{4, 0.4f}, {5, 0.4f}});         // tile 4 shows 4, 5, 4, 5, ...
    ponko::tilemap_set_flipped(ctx, sky, 3, 2, 7, PONKO_TILE_FLIP_X);   // a mirrored tile
    // ANCHOR_END: tile_extras
    build_wall(ctx, sky, 10, 10);
    if (ponko::tilemap_flip(ctx, sky, 3, 2) != PONKO_TILE_FLIP_X) { std::fprintf(stderr, "flip\n"); std::abort(); }
    const std::size_t parts = ponko::level_entities(ctx, level).size();
    save_meadow(ctx);
    const ponko::entity again = load_meadow(ctx);
    if (ponko::level_entities(ctx, again).size() > parts) { std::fprintf(stderr, "level saves\n"); std::abort(); }
    ponko::despawn_level(ctx, again);
    ctx.storage_delete("saves/meadow.json"); // the storage guide shares the folder
}

// ---- Camera, text and UI ---------------------------------------------------------
[[maybe_unused]] static void camera(ponko::ctx& ctx, ponko::vec2 bounds) {
    // ANCHOR: camera_fit
    const ponko::vec2 window = ctx.window_size();
    auto cam = ctx.camera();
    cam.zoom = std::min(window.x / bounds.x, window.y / bounds.y);
    cam.position = bounds / 2 - window / (2 * cam.zoom); // look at the middle
    ctx.set_camera(cam);
    // ANCHOR_END: camera_fit
    // ANCHOR: text
    ctx.draw_text("Hello", {20, 20}, 24, ponko::colors::white);
    const ponko::vec2 size = ctx.measure_text("Paused", 32);   // for centering
    const ponko::font title = ctx.load_font("fonts/Title.ttf");  // TrueType or OpenType
    auto params = ponko::text_defaults();
    params.font = title;
    params.size = 40;
    ctx.draw_text("Colony", {20, 60}, params);
    // ANCHOR_END: text
    (void)size;
}

[[maybe_unused]] static void shapes(ponko::ctx& ctx, ponko::vec2 from, ponko::vec2 to, ponko::vec2 tower) {
    // ANCHOR: shapes
    ctx.draw_rect(ponko::rect_at({0, 0}, {200, 20}), ponko::rgb(0.23f, 0.35f, 0.25f));
    ctx.draw_line(from, to, 2, ponko::colors::red);
    ctx.draw_circle(tower, 120, ponko::rgba(0, 0, 1, 0.2f));
    // ANCHOR_END: shapes
}

// ANCHOR: hud
struct Hud {
    int dropped = 0;
    bool follow = false;
    void operator()(ponko::ctx& ctx) {
        const ponko::rect panel = ponko::rect_at({12, 12}, {230, 172});
        ctx.ui_panel(panel);
        const float pad = ctx.ui_style().padding;
        ponko::vec2 at = panel.min + ponko::vec2{pad, pad};
        char lines[128];
        std::snprintf(lines, sizeof lines, "zoom %.2fx\ncrabs dropped: %d\n%.0f FPS",
                      double(ctx.camera().zoom), dropped, double(ctx.fps()));
        at.y += ctx.ui_label(at, lines).y + 8;

        if (ctx.ui_button(ponko::rect_at(at, {210, 30}), follow ? "Stop following" : "Follow orbiter"))
            follow = !follow;
        at.y += 38;
        if (ctx.ui_button(ponko::rect_at(at, {210, 30}), "Reset camera")) {
            ctx.set_camera({{0, 0}, 1});
            follow = false;
        }
    }
};
// ponko::game("UI", 1280, 720).camera_controls().system(Hud{}, "hud");
// ANCHOR_END: hud

[[maybe_unused]] static void screenshot(ponko::ctx& ctx) {
    // ANCHOR: screenshot
    if (ctx.key_pressed(PONKO_KEY_F12)) ctx.save_screenshot("shot.png");
    // ANCHOR_END: screenshot
}

// ---- Hot reload --------------------------------------------------------------------
[[maybe_unused]] static void hot_reload_game() {
    // ANCHOR: hot_reload
    ponko::texture level{};
    ponko::game("Art test", 1280, 720)
        .hot_reload(true)   // the default in debug builds of the engine
        .system([&](ponko::ctx& ctx) {
            for (ponko::texture t : ctx.reloaded_textures())
                if (t.id == level.id) std::puts("level.png changed: rebuilding the map");
        }, "watch_level")
        .run([&](ponko::ctx& ctx) { level = ctx.load_texture("level.png"); });
    // ANCHOR_END: hot_reload
}

// ---- Physics -----------------------------------------------------------------------
// ANCHOR: physics_bodies
struct Bell { float rang; };

static void build_level(ponko::world_view world) {
    // Fixed bodies never move: the ground.
    world.spawn(ponko::transform_at({500, 690}), ponko::fixed_body(), ponko::rect_collider({1000, 20}));
    // A bouncy ball and a crate, both dynamic: gravity and collisions move them.
    auto ball = ponko::circle_collider(16);
    ball.restitution = 0.8f;
    world.spawn(ponko::transform_at({300, 100}), ponko::dynamic_body(), ball);
    auto box = ponko::rect_collider({40, 40});
    box.friction = 0.9f;
    world.spawn(ponko::transform_at({320, 0}), ponko::dynamic_body(), box);
    // A ramp: any convex shape of up to 8 corners, around its position.
    world.spawn(ponko::transform_at({150, 620}), ponko::fixed_body(),
                ponko::polygon_collider({{-140, 60}, {140, 60}, {-140, -60}}));
    // A sensor reports overlaps but doesn't push.
    auto bucket = ponko::rect_collider({120, 80});
    bucket.sensor = true;
    world.spawn(ponko::transform_at({800, 640}), ponko::fixed_body(), bucket, Bell{0});
}
// ANCHOR_END: physics_bodies

// ANCHOR: physics_collisions
static void ring_the_bell(ponko::ctx& ctx) {
    auto world = ctx.world();
    bool rang = false;
    for (const ponko::collision& c : ctx.collisions_started())
        rang |= c.sensor && (world.has<Bell>(ponko::bits(c.a)) || world.has<Bell>(ponko::bits(c.b)));
    world.each([&](Bell& bell) { bell.rang = rang ? 1.0f : std::max(0.0f, bell.rang - ctx.dt() * 2); });
}
// ANCHOR_END: physics_collisions

// ANCHOR: physics_queries
static void blast(ponko::ctx& ctx, ponko::vec2 center) {
    std::vector<PonkoEntity> near;
    ctx.entities_in_circle(center, 200, near);
    for (PonkoEntity found : near) {
        const ponko::entity e = ponko::bits(found);
        const ponko::vec2 away = ponko::normalize_or(ctx.world().get<ponko::transform2d>(e).position - center, {0, -1});
        if (auto mass = ctx.mass(e)) ctx.apply_impulse(e, away * *mass * 600.0f);
    }
}

static std::optional<ponko::ray_hit> laser(ponko::ctx& ctx, ponko::vec2 from) {
    return ctx.cast_ray(from, {0, 1}, 700);   // what's straight below
}
// ANCHOR_END: physics_queries

static bool physics_snippets() {
    bool landed = false;
    // ANCHOR: physics_setup
    auto world = ponko::game("Physics", 1000, 700)
        .physics({0, 980})   // pixels/s², y is down
        .sim_system(ring_the_bell, "bell")
        // ANCHOR_END: physics_setup
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 100) blast(ctx, {300, 690});
            if (ctx.sim_tick() == 1) landed = laser(ctx, {500, 0}).has_value();
        }, "test")
        .run_headless(120, [](ponko::ctx& ctx) { build_level(ctx.world()); });
    return landed && world.size() == 5;
}

// ---- 3D physics ---------------------------------------------------------------------
// ANCHOR: physics3d_world
struct Walker { ponko::entity id; };   // a resource: who walks

static void build_world3d(ponko::ctx& ctx) {
    auto world = ctx.world();
    // Ground: a heightfield from the same heights as the terrain tile it
    // stands for (here a gentle slope), at the tile's corner.
    const int n = 33;
    std::vector<float> heights(n * n);
    for (int i = 0; i < n * n; ++i) heights[i] = float(i % n) * 0.1f;
    auto ground = ctx.heightfield_collider(heights, 64);
    world.spawn(ponko::transform3d_at({-32, 0, -32}), ponko::fixed_body3d(), ponko::mesh_collider(ground));
    // A crate that falls, slides and can be pushed.
    auto crate = ponko::box_collider({1, 1, 1});
    crate.density = 200;   // kg per m³
    world.spawn(ponko::transform3d_at({4, 6, 0}), ponko::dynamic_body3d(), crate, ponko::velocity3d{});
    // The walker: a capsule 1.8 m tall, its transform at its feet.
    auto walker = ponko::character(1.8f, 0.3f);
    walker.step_height = 0.3f;   // climbs stairs this high
    const ponko::entity me = world.spawn(ponko::transform3d_at({0, 4, 0}), walker);
    world.set_resource(Walker{me});
}
// ANCHOR_END: physics3d_world

// ANCHOR: physics3d_walk
/// WASD walks, space jumps (only from the ground).
static void walk(ponko::ctx& ctx) {
    auto world = ctx.world();
    const ponko::entity me = world.resource<Walker>().id;
    auto c = world.get<ponko::character3d>(me);
    float x = float(ctx.key_down(PONKO_KEY_D)) - float(ctx.key_down(PONKO_KEY_A));
    float z = float(ctx.key_down(PONKO_KEY_S)) - float(ctx.key_down(PONKO_KEY_W));
    c.velocity.x = x * 4;   // m/s
    c.velocity.z = z * 4;
    if (c.grounded && ctx.key_pressed(PONKO_KEY_SPACE)) c.velocity.y = 5;
    world.set(me, c);
}
// ANCHOR_END: physics3d_walk

// ANCHOR: physics3d_queries
/// What the walker is looking at, up to 50 m away (not itself).
static std::optional<ponko::hit3d> look(ponko::ctx& ctx, ponko::vec3 eye, ponko::vec3 forward) {
    const ponko::entity me = ctx.world().resource<Walker>().id;
    return ctx.cast_ray3d(eye, forward, 50, ponko::filter3d().exclude(me));
}
// ANCHOR_END: physics3d_queries

static bool physics3d_snippets() {
    bool saw_ground = false;
    // ANCHOR: physics3d_setup
    auto world = ponko::game("3D physics", 1280, 720)
        .physics3d({0, -9.81f, 0})   // m/s², +Y up; 50 steps a second
        .sim_system(walk, "walk")
        // ANCHOR_END: physics3d_setup
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 1) saw_ground = look(ctx, {0, 10, 0}, {0, -1, 0}).has_value();
        }, "test")
        .run_headless(100, build_world3d);
    const auto me = world.resource<Walker>().id;
    return saw_ground && world.get<ponko::character3d>(me).grounded;
}

// ANCHOR: collider_streaming
/// A streamed tile, built on a worker: its meshes and its collider, from
/// the same heights.
struct BuiltTile {
    ponko::terrain_tile_data drawn;
    ponko::collision_mesh_data solid;
};
static std::future<BuiltTile> build_tile(std::vector<float> heights) {
    return std::async(std::launch::async, [heights = std::move(heights)] {
        return BuiltTile{ponko::terrain_tile_data(heights, 128), ponko::collision_mesh_data::heightfield(heights, 128)};
    });
}

/// Once built, on the game's thread: drawn and solid. Free both when the
/// tile goes (ctx.free_terrain_tile, ctx.free_collision_mesh).
static ponko::entity place_tile(ponko::ctx& ctx, ponko::vec3 corner, const BuiltTile& built, ponko::material ground) {
    auto tile = ctx.upload_terrain_tile(built.drawn, ground);
    auto mesh = ctx.add_collision_mesh(built.solid);
    return ctx.world().spawn(ponko::transform3d_at(corner), tile, ponko::solid_terrain(mesh));
}

/// Drawn out to the streamer's 600 m, but solid only near the player and
/// the car (further ahead of the car: it's faster), 4 new colliders a step.
static void solid_near(ponko::ctx& ctx, ponko::entity player, ponko::entity car) {
    ctx.stream_colliders(4);
    ctx.world().set(player, ponko::focus_colliders(150, 200));
    ctx.world().set(car, ponko::focus_colliders(300, 350));
}
// ANCHOR_END: collider_streaming

static void tiles_are_solid_only_near_the_player_and_the_car() {
    std::vector<ponko::entity> tiles;
    std::vector<bool> active;
    ponko::game("solid", 1, 1)
        .physics3d({0, -9.81f, 0})
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 5)
                for (auto t : tiles) active.push_back(ctx.is_collider_active(t));
        })
        .run_headless(6, [&](ponko::ctx& ctx) {
            auto ground = ctx.default_material();
            for (int k = 0; k < 8; ++k)
                tiles.push_back(place_tile(ctx, {float(k) * 128, 0, 0}, build_tile(std::vector<float>(33 * 33, 0.0f)).get(), ground));
            auto player = ctx.world().spawn(ponko::transform3d_at({10, 0, 10}), ponko::character(1.8f, 0.3f));
            auto car = ctx.world().spawn(ponko::transform3d_at({900, 1, 10}), ponko::dynamic_body3d(), ponko::box_collider({2, 1, 4}));
            solid_near(ctx, player, car);
        });
    const std::vector<bool> want{true, true, false, false, true, true, true, true};
    if (active != want) { std::fprintf(stderr, "solid tiles\n"); std::abort(); }
}

// ---- Vehicles -------------------------------------------------------------------------
struct Driven { ponko::entity car; };   // a resource: the car the player drives

// ANCHOR: vehicle3d_car
/// A 1200 kg box car facing -Z, its center of mass low between the axles,
/// front wheels first.
static ponko::entity spawn_car(ponko::ctx& ctx, ponko::vec3 at) {
    std::vector<ponko::wheel3d> wheels;
    for (float z : {-1.3f, 1.3f})
        for (float x : {-0.8f, 0.8f}) {
            auto w = ponko::wheel({x, -0.3f, z}, 0.35f);
            w.sideways_friction.stiffness = 1.3f;
            wheels.push_back(w);
        }
    auto body = ponko::with_center_of_mass(ponko::with_mass(ponko::dynamic_body3d(), 1200), {0, -0.35f, 0});
    return ctx.world().spawn(ponko::transform3d_at(at), body, ponko::box_collider({1.8f, 0.6f, 4.2f}),
                             ponko::velocity3d{}, ponko::vehicle({wheels[0], wheels[1], wheels[2], wheels[3]}));
}
// ANCHOR_END: vehicle3d_car

// ANCHOR: vehicle3d_drive
/// Before each physics step: W drives the rear wheels, S brakes all four,
/// A and D steer the front ones.
static void drive(ponko::ctx& ctx) {
    auto world = ctx.world();
    const ponko::entity car = world.resource<Driven>().car;
    auto v = world.get<ponko::vehicle3d>(car);
    const float throttle = float(ctx.key_down(PONKO_KEY_W));
    const bool brake = ctx.key_down(PONKO_KEY_S);
    const float steer = 0.5f * (float(ctx.key_down(PONKO_KEY_D)) - float(ctx.key_down(PONKO_KEY_A)));
    for (std::uint32_t i = 0; i < v.wheel_count; ++i) {
        const bool front = i < 2;
        v.wheels[i].steer_angle = front ? steer : 0;   // radians, positive right
        v.wheels[i].motor_torque = front ? 0 : throttle * 900;   // N·m
        v.wheels[i].brake_torque = brake ? 3000 : 0;
    }
    world.set(car, v);
}
// ANCHOR_END: vehicle3d_drive

static bool vehicle3d_snippets() {
    // ANCHOR: vehicle3d_setup
    auto world = ponko::game("vehicles", 1280, 720)
        .add(ponko::phase::before_physics, drive, "drive") // sets the wheels' inputs for this tick's step
        .physics3d({0, -9.81f, 0})
        // ANCHOR_END: vehicle3d_setup
        .run_headless(100, [](ponko::ctx& ctx) {
            ctx.world().spawn(ponko::transform3d_at({0, -0.5f, 0}), ponko::fixed_body3d(),
                              ponko::box_collider({100, 1, 100}));
            ctx.world().set_resource(Driven{spawn_car(ctx, {0, 1, 0})});
        });
    const auto car = world.get<ponko::vehicle3d>(world.resource<Driven>().car);
    bool grounded = true;
    for (std::uint32_t i = 0; i < car.wheel_count; ++i) grounded = grounded && car.wheels[i].grounded;
    return car.wheel_count == 4 && grounded;
}

// ---- Lockstep networking ----------------------------------------------------------
// ANCHOR: lockstep_input
struct Order { float x, y; bool go; };   // what one player did this tick
struct Owner { std::uint32_t player; };

static void send_orders(ponko::ctx& ctx) {
    // Usually from the mouse, in a frame system; offline it just works too.
    ctx.set_input(Order{100, 50, ctx.sim_tick() % 30 == 0});
}

static void obey_orders(ponko::ctx& ctx) {
    // Every peer applies every player's input, in the same order.
    std::vector<std::optional<Order>> orders;
    for (std::uint32_t p = 0; p < ctx.players(); ++p) orders.push_back(ctx.input<Order>(p));
    ctx.world().each([&](ponko::transform2d& t, const Owner& owner) {
        if (owner.player < orders.size() && orders[owner.player] && orders[owner.player]->go)
            t.position = {orders[owner.player]->x, orders[owner.player]->y};
    });
}
// ANCHOR_END: lockstep_input

[[maybe_unused]] static void multiplayer(bool hosting) {
    // ANCHOR: lockstep_setup
    ponko::game game("RTS", 1280, 720);
    if (hosting)
        game.lockstep_host("0.0.0.0:7777", 2);        // 2 players, 3 ticks input delay
    else
        game.lockstep_join("192.168.1.20:7777");
    game.sim_system(send_orders).sim_system(obey_orders).run([](ponko::ctx&) {});
    // ANCHOR_END: lockstep_setup
}

[[maybe_unused]] static void steam_multiplayer(bool hosting) {
    // ANCHOR: steam_setup
    // ponko_ffi built with --features steam; 480 is Valve's test app.
    ponko::game game("RTS", 1280, 720);
    if (std::uint64_t lobby = ponko::steam_lobby_from_args())   // launched from an invite
        game.lockstep_steam_join(480, lobby);
    else if (hosting)
        game.lockstep_steam_host(480, 2);                          // then ctx.steam_invite()
    else
        game.lockstep_steam_join(480, ponko::steam_wait_for_invite(480, 120000));
    // ANCHOR_END: steam_setup
    game.sim_system(send_orders).sim_system(obey_orders).run([](ponko::ctx&) {});
}

// ---- Data files ----------------------------------------------------------------------
// ANCHOR: data_files
struct Tuning { float speed; float damage; };

[[maybe_unused]] static void load_tuning(ponko::ctx& ctx) {
    ctx.world().reflect_resource<Tuning>({ponko::field("speed", &Tuning::speed), ponko::field("damage", &Tuning::damage)});
    ctx.load_json<Tuning>("tuning.json");   // {"speed": 30, "damage": 2}
    // Systems read ctx.world().resource<Tuning>(): the file's current values.
}
// ANCHOR_END: data_files

// ---- Sound -------------------------------------------------------------------------
[[maybe_unused]] static void sound(ponko::ctx& ctx) {
    // ANCHOR: sound
    const ponko::sound hit = ctx.load_sound("hit.ogg");   // OGG Vorbis, WAV, MP3 or FLAC from assets/
    ctx.play_sound(hit);

    const ponko::sound music = ctx.load_sound("theme.ogg");
    auto params = ponko::sound_defaults();
    params.volume = 0.5f;
    params.looping = true;
    const ponko::playing song = ctx.play_sound(music, params);
    ctx.stop_sound(song);
    // ANCHOR_END: sound
}

static ponko::sound beep(ponko::ctx& ctx) {
    // ANCHOR: sound_samples
    std::vector<float> samples(4410);   // 0.1 s at 44.1 kHz
    for (std::size_t i = 0; i < samples.size(); ++i)
        samples[i] = 0.3f * std::sin(float(i) * 880.0f * 6.2831853f / 44100.0f);
    const ponko::sound click = ctx.create_sound(44100, samples.data(), samples.size());
    // ANCHOR_END: sound_samples
    return click;
}

// ANCHOR: turns
struct Brain { std::uint32_t plans; };

static void think(ponko::ctx& ctx) {
    const ponko::turns turn = ctx.turns(8);   // each ant thinks every 8th tick
    ctx.world().each_par([&](ponko::entity e, Brain& brain) {
        if (turn.includes(e))
            brain.plans += 1;   // turn.dt(): time since this ant's last turn
    });
}
// ANCHOR_END: turns

// ---- Reflection and saving ---------------------------------------------------------
// ANCHOR: reflect
struct Unit {
    float current;
    float max;
    ponko::vec2 home;
    ponko::entity target;   // entity ids are uint64_t
    bool angry;
};

static void reflect_units(ponko::world_view world) {
    // Tools and agents (ponko-mcp) now see {"current", "max", "home", "target", "angry"}.
    world.reflect<Unit>({
        ponko::field("current", &Unit::current),
        ponko::field("max", &Unit::max),
        ponko::field("home", &Unit::home),
        ponko::field("target", &Unit::target),
        ponko::field("angry", &Unit::angry),
    });
}
// ANCHOR_END: reflect

// ANCHOR: reflect_resource
struct Teams { std::uint32_t red, blue; };

static void reflect_teams(ponko::world_view world) {
    world.reflect_resource<Teams>({ponko::field("red", &Teams::red), ponko::field("blue", &Teams::blue)});
    world.set_resource(Teams{0, 0});   // agents: get_resource "Teams" -> {"blue": 0, "red": 0}
}
// ANCHOR_END: reflect_resource

// ---- Inspector ----------------------------------------------------------------------
[[maybe_unused]] static void playtest_build() {
    // ANCHOR: inspector
    ponko::game("Playtest", 1280, 720)
        .inspector(true)   // also in release builds: the ` key opens it
        .run([](ponko::ctx& ctx) { reflect_units(ctx.world()); });
    // ANCHOR_END: inspector
}

static void save_and_load(ponko::world_view world) {
    // ANCHOR: save
    const std::string save = world.save();   // JSON text; write it to a file
    // ...
    world.load(save);                        // every entity back, with its id
    // ANCHOR_END: save
}

// ---- Headless runs and testing ---------------------------------------------------
// ANCHOR: headless
struct Pos { float y; };

static void things_fall_ten_units_per_second() {
    ponko::entity thing = 0;
    auto world = ponko::game("test", 800, 600)
        .sim_system([](ponko::ctx& ctx) {
            const float dt = ctx.dt();
            ctx.world().each([dt](Pos& p) { p.y -= 10 * dt; });
        }, "fall")
        .run_headless(60, [&](ponko::ctx& ctx) { thing = ctx.world().spawn(Pos{100}); });
    const float y = world.get<Pos>(thing).y;
    if (std::fabs(y - 90) > 1e-3f) { std::fprintf(stderr, "fell to %f\n", double(y)); std::abort(); }
}
// ANCHOR_END: headless

// ---- Randomness ------------------------------------------------------------------
// ANCHOR: random
struct Heading { float radians; };

static void wander(ponko::ctx& ctx) {
    const ponko::random random = ctx.random();
    ctx.world().each_par([&](ponko::entity e, Heading& h) {
        h.radians += random.of(e).range(-0.2f, 0.2f);
    });
}
// ANCHOR_END: random

// ---- Pathfinding with flow fields --------------------------------------------------
// ANCHOR: flow_map
static void make_walls(ponko::nav_grid& nav) {
    // 1 is normal ground, higher is slower (mud), nav_grid::wall blocks.
    for (std::uint32_t y = 0; y < 60; ++y) nav.set_cost(40, y, ponko::nav_grid::wall);
    for (std::uint32_t x = 50; x < 70; ++x)
        for (std::uint32_t y = 20; y < 40; ++y) nav.set_cost(x, y, 6);
}
// ANCHOR_END: flow_map

// ANCHOR: flow_steer
static void steer(ponko::ctx& ctx, const ponko::flow_field& flow) {
    const float step = 40 * ctx.dt();
    const ponko::flow_view view = flow.view();   // inline lookups, safe from every thread
    ctx.world().each_par([&](ponko::transform2d& t) {
        t.position += view.direction_at(t.position) * step;
    });
}
// ANCHOR_END: flow_steer

// ---- Events --------------------------------------------------------------------------
// ANCHOR: events
struct Hp { int value; };
struct Died { ponko::entity who; };

// Sends one Died per entity out of health, from inside a loop.
static void deaths(ponko::ctx& ctx) {
    ctx.world().each([&](ponko::entity e, const Hp& hp) {
        if (hp.value <= 0) ctx.send(Died{e});
    });
}

// Any later system (or this one next tick) reads them, once each.
static void funerals(ponko::ctx& ctx) {
    for (const Died& d : ctx.read<Died>()) ctx.despawn_later(d.who);
}
// ANCHOR_END: events

// ---- When commands apply --------------------------------------------------------------
// ANCHOR: command_barrier
struct Shot { int power; };
static std::size_t shots_seen = 0;

// Records a spawn. The world doesn't change until this system returns.
static void fire(ponko::ctx& ctx) {
    const std::size_t before = ctx.world().size();
    ctx.spawn_later(Shot{3});
    if (ctx.world().size() != before) std::abort();   // still pending
}

// The next system finds the shot in the world, in the same tick.
static void count_shots(ponko::ctx& ctx) {
    shots_seen = 0;
    ctx.world().each([&](const Shot&) { ++shots_seen; });
}
// ANCHOR_END: command_barrier

static void shots_arrive_before_the_next_system() {
    ponko::game("barrier", 1, 1)
        .sim_system(fire, "fire")
        .sim_system(count_shots, "count shots")
        .run_headless(3);
    if (shots_seen != 3) {
        std::fprintf(stderr, "barrier: %zu shots seen, expected 3\n", shots_seen);
        std::abort();
    }
}

static void the_dead_are_buried() {
    auto world = ponko::game("events", 1, 1)
        .sim_system(deaths, "deaths")
        .sim_system(funerals, "funerals")
        .run_headless(1, [](ponko::ctx& ctx) {
            // ANCHOR: events_reflect
            // Described with its fields: tools can collect Died events as JSON.
            ctx.world().reflect<Died>({ponko::field("who", &Died::who)});
            // ANCHOR_END: events_reflect
            for (int hp : {0, 3, -1, 5}) ctx.world().spawn(Hp{hp});
        });
    if (world.size() != 2) {
        std::fprintf(stderr, "events: %zu alive, expected 2\n", world.size());
        std::abort();
    }
}

static void crowds_reach_the_goal() {
    // ANCHOR: flow_compute
    ponko::nav_grid nav(100, 80, 4.0f);   // 100 x 80 cells, 4 world units each
    make_walls(nav);
    ponko::flow_field flow;
    flow.compute(nav, ponko::vec2{390, 10});   // or several goals: the nearest wins
    // ANCHOR_END: flow_compute
    auto world = ponko::game("flow", 1, 1)
        .sim_system([&](ponko::ctx& ctx) { steer(ctx, flow); }, "steer")
        .run_headless(1200, [](ponko::ctx& ctx) {
            for (int i = 0; i < 1000; ++i)
                ctx.world().spawn(ponko::transform_at({2.0f + float(i % 30) * 5, 2.0f + float(i / 30) * 9}));
        });
    std::size_t arrived = 0;
    world.each([&](const ponko::transform2d& t) {
        arrived += flow.distance_at(t.position).value_or(1e9f) < 12;
    });
    const auto start = flow.distance_at({2, 2});
    if (arrived < 990 || !start || *start < 390 || nav.walkable({162, 10}) || !nav.walkable({2, 2})) {
        std::fprintf(stderr, "flow: %zu of 1000 arrived\n", arrived);
        std::abort();
    }
}

// ---- Attachments -----------------------------------------------------------------
// ANCHOR: attach
struct Tank { float speed; };

static void turrets_ride_on_tanks() {
    ponko::entity turret = 0;
    auto world = ponko::game("tanks", 800, 600)
        .sim_system([](ponko::ctx& ctx) {
            ctx.world().each([](ponko::transform2d& t, const Tank& tank) { t.position.x += tank.speed; });
        }, "drive")
        .follow_attachments() // after the systems that move things
        .run_headless(10, [&](ponko::ctx& ctx) {
            auto w = ctx.world();
            const auto tank = w.spawn(ponko::transform_at({0, 0}), Tank{1});
            turret = w.spawn(ponko::transform_at({0, 0}), ponko::attach_to(tank, {0, -4}));
        });
    const auto at = world.get<ponko::transform2d>(turret).position;
    if (at != ponko::vec2{10, -4}) { std::fprintf(stderr, "turret at %f,%f\n", double(at.x), double(at.y)); std::abort(); }
}
// ANCHOR_END: attach

// ---- Scenes and prefabs -------------------------------------------------------------
// ANCHOR: scenes
struct Palm { float height; };

static void beach(ponko::ctx& ctx) {
    ctx.world().reflect<Palm>({ponko::field("height", &Palm::height)});   // components in files must be reflected
    ctx.load_scene("levels/beach.json");   // the level: its entities, ids kept
    for (int i = 0; i < 3; ++i)            // three more palms, all alike, in a row
        ctx.spawn_prefab("prefabs/palm.json", {400.0f + float(i) * 64, 400});
}
// ANCHOR_END: scenes

static void the_beach_has_five_palms() {
    auto world = ponko::game("guide", 1, 1).run_headless(0, beach);
    std::size_t palms = 0;
    world.each([&](const Palm&) { ++palms; });
    if (palms != 5) { std::fprintf(stderr, "%zu palms\n", palms); std::abort(); }
}

// ---- Saves and settings --------------------------------------------------------------
// ANCHOR: storage
static void keep_volume(ponko::ctx& ctx, float volume) {
    // The old file stays whole until the new one is, then becomes the backup.
    ctx.storage_write("volume.txt", std::to_string(volume));
}

static float saved_volume(ponko::ctx& ctx) {
    auto text = ctx.storage_read_text("volume.txt");   // nothing on a first run
    return text ? std::stof(*text) : 1.0f;
}

static void quick_save(ponko::ctx& ctx) { ctx.save_world("quick"); }   // saves/quick.json
static void quick_load(ponko::ctx& ctx) { ctx.load_world("quick"); }
// ANCHOR_END: storage

static void storage_keeps_files_between_runs() {
    // A folder of the test's own (beside the test binary), not the player's data folder.
    ponko::game("guide", 1, 1).data_dir("guide-storage").run_headless(0, [](ponko::ctx& ctx) {
        ctx.storage_delete("volume.txt");
        if (saved_volume(ctx) != 1.0f) std::abort();
        keep_volume(ctx, 0.25f);
        ctx.world().reflect<Palm>({ponko::field("height", &Palm::height)});
        ctx.world().spawn(Palm{3});
        quick_save(ctx);
    });
    auto world = ponko::game("guide", 1, 1).data_dir("guide-storage").run_headless(0, [](ponko::ctx& ctx) {
        if (saved_volume(ctx) != 0.25f) { std::fprintf(stderr, "volume %f\n", double(saved_volume(ctx))); std::abort(); }
        if (ctx.storage_list() != std::vector<std::string>{"saves/quick.json", "volume.txt"}) std::abort();
        if (ctx.storage_read("missing.txt").has_value()) std::abort();
        ctx.world().reflect<Palm>({ponko::field("height", &Palm::height)});
        quick_load(ctx);
    });
    std::size_t palms = 0;
    world.each([&](const Palm& p) { palms += p.height == 3 ? 1 : 0; });
    if (palms != 1) { std::fprintf(stderr, "%zu saved palms\n", palms); std::abort(); }
    bool refused = false;
    ponko::game("guide", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {   // no app id: no files
        try { keep_volume(ctx, 0.5f); } catch (const ponko::error&) { refused = true; }
    });
    if (!refused) std::abort();
}

namespace old_saves {
// ANCHOR: save_migrations
// The game's component as it is now, at save version 2.
struct Health { float now; float max; };   // max in hearts; version 1 saved hit points

static void setup(ponko::world& world) {
    // Named, so saves don't depend on the C++ namespace.
    world.reflect<Health>("Health", {ponko::field("now", &Health::now), ponko::field("max", &Health::max)});
    world.set_save_version(2);                       // before the first load
    world.migration(0)                               // 0 -> 1: saves from before versions
        .rename_component("Hp", "Health");
    world.migration(1)                               // 1 -> 2
        .rename_field("Health", "current", "now")
        .convert_field("Health", "max", [](const std::string& points) {
            return std::to_string(std::ceil(std::stod(points) / 4.0));   // 4 points a heart
        });
}
// ANCHOR_END: save_migrations

static void old_saves_still_load() {
    ponko::world world;
    setup(world);
    world.load(R"({"format": "ponko-world", "version": 1,
                   "entities": [{"entity": 0, "components": {"Hp": {"current": 3, "max": 13}}}]})");
    std::size_t found = 0;
    world.each([&](const Health& h) { found += h.now == 3.0f && h.max == 4.0f ? 1 : 0; });
    if (found != 1 || world.save().find("\"game_version\":2") == std::string::npos) { std::fprintf(stderr, "migration\n"); std::abort(); }
    bool refused = false;
    try {
        world.load(R"({"format": "ponko-world", "version": 1, "game_version": 3, "entities": []})");
    } catch (const ponko::error& e) {
        refused = e.kind() == PONKO_ERROR_UNSUPPORTED;
    }
    if (!refused) { std::fprintf(stderr, "a newer save loaded\n"); std::abort(); }
}
} // namespace old_saves

// ANCHOR: prefab_tree
static void carts(ponko::ctx& ctx) {
    // A cart: its body (the root), two wheels, a driver and the driver's hat.
    auto cart = ctx.spawn_prefab("prefabs/cart.json", {200, 300});
    auto parts = ctx.prefab_parts(cart);   // the four parts spawned with it
    ctx.despawn_prefab(cart);              // the body and its four parts
    if (parts.size() != 4 || ctx.world().size() != 0) std::abort();
}
// ANCHOR_END: prefab_tree

static void a_cart_comes_and_goes_whole() {
    auto world = ponko::game("guide", 1, 1).run_headless(0, carts);
    if (world.size() != 0) { std::fprintf(stderr, "%zu left\n", world.size()); std::abort(); }
}

// ---- 3D -------------------------------------------------------------------------------
// ANCHOR: three_d
/// Turns around its own Y axis, this many radians a second.
struct Spin { float speed; };

static void village(ponko::ctx& ctx) {
    auto grass_look = ponko::material_of(ponko::rgb(.37f, .55f, .29f));
    grass_look.roughness = 0.9f;
    auto grass = ctx.create_material(grass_look);
    auto ground = ctx.create_mesh(ponko::mesh_data::plane(100), grass);
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(ground));

    auto house = ctx.load_mesh("models/house.glb");   // its walls and roof keep their materials
    // ANCHOR: loaded_materials
    // The house's walls, drawn with a shader of our own: keep what they
    // were made from (textures, colors) and change the shader.
    auto weathered = ctx.create_shader("weathered.wgsl",
        "fn surface(s: Surface) -> Surface { var o = s; o.albedo *= 0.8; o.roughness = 0.95; return o; }");
    for (auto [walls, desc] : ctx.loaded_materials("walls")) {
        desc.has_shader = true;
        desc.shader = weathered;
        ctx.update_material(walls, desc);
    }
    // ANCHOR_END: loaded_materials
    for (int i = 0; i < 5; ++i) {
        ponko::vec3 at{float(i) * 4 - 8, 0, -6};
        ctx.world().spawn(ponko::transform3d_at(at), ponko::model_of(house), Spin{0.5f});
    }

    auto camera = ctx.camera3d();
    camera.position = {0, 6, 12};
    ctx.set_camera3d(camera);
    ctx.camera3d_look_at({0, 0, -4});
    auto light = ctx.lighting();
    light.sun_direction = {-0.8f, -0.6f, -0.4f};   // late afternoon: long shadows
    ctx.set_lighting(light);
}

static void spin(ponko::ctx& ctx) {
    const float dt = ctx.dt();
    ctx.world().each([&](ponko::transform3d& t, const Spin& s) {
        t.rotation *= ponko::quat_from_rotation_y(s.speed * dt);
    });
}
// ANCHOR_END: three_d

// ANCHOR: trilight
/// Without an atmosphere: a blue sky, a green-grey horizon and brown ground.
static void trilight(ponko::ctx& ctx) {
    auto light = ctx.lighting();
    light.sky_color = ponko::rgb(0.56f, 0.70f, 0.85f);
    light.has_equator = true;
    light.equator_color = ponko::rgb(0.55f, 0.60f, 0.53f);
    light.ground_color = ponko::rgb(0.30f, 0.25f, 0.21f);
    ctx.set_lighting(light);
}
// ANCHOR_END: trilight

// ANCHOR: sky
/// A misty morning: the sun low in the east, fog lying in the valleys.
static void misty_morning(ponko::ctx& ctx) {
    auto light = ctx.lighting();
    light.sun_direction = ponko::sun_direction_at(7.5f);
    ctx.set_lighting(light);
    auto sky = ctx.sky();
    sky.haze = 2;              // a paler sky, a bigger glow around the sun
    sky.fog_density = 0.03f;   // thick near the ground
    sky.fog_height = 0;
    sky.fog_falloff = 10;      // and thin ten meters up
    ctx.set_sky(sky);
}
// ANCHOR_END: sky

// ANCHOR: mesh_shadows
/// A meadow: thousands of grass tufts, too small for their shadows to show.
static void plant_meadow(ponko::ctx& ctx, ponko::mesh tuft) {
    ctx.set_mesh_shadows(tuft, false); // no shadow map work for grass
    for (int i = 0; i < 1000; ++i) {
        ponko::vec3 at{(i % 40) * 0.5f, 0, (i / 40) * 0.5f};
        ctx.world().spawn(ponko::transform3d_at(at), ponko::model_of(tuft));
    }
}
// ANCHOR_END: mesh_shadows

// ANCHOR: model_material
/// One uploaded ball in three looks, and a house with only its roof changed.
static void recolored(ponko::ctx& ctx, ponko::mesh house, ponko::material gold, ponko::material rust) {
    auto ball = ctx.create_plain_mesh(ponko::mesh_data::sphere(0.5f, 32));
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(ball));  // plain white
    auto shiny = ctx.world().spawn(ponko::transform3d_at({1, 0, 0}), ponko::model_of(ball));
    ctx.set_model_material(shiny, gold);
    auto old = ctx.world().spawn(ponko::transform3d_at({2, 0, 0}), ponko::model_of(ball));
    ctx.set_model_material(old, rust);
    // Part 1 only; the other parts keep the materials from the file.
    auto home = ctx.world().spawn(ponko::transform3d_at({6, 0, 0}), ponko::model_of(house));
    ctx.set_model_part_materials(home, {{1, rust}});
}
// ANCHOR_END: model_material

// ANCHOR: shadow_mesh
/// A detailed tree with a cheap shadow, a flat sign with none, and a stand-in
/// shadow for a row of fence pieces that cast none themselves.
static void yard(ponko::ctx& ctx, ponko::mesh tree, ponko::mesh blob, ponko::mesh sign,
                 ponko::mesh fence, ponko::mesh fence_shadow) {
    auto t = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(tree));
    ctx.set_shadow_mesh(t, blob);
    auto s = ctx.world().spawn(ponko::transform3d_at({4, 0, 0}), ponko::model_of(sign));
    ctx.set_no_shadow(s);
    for (int i = 0; i < 10; ++i) {
        auto f = ctx.world().spawn(ponko::transform3d_at({float(i), 0, 6}), ponko::model_of(fence));
        ctx.set_no_shadow(f);
    }
    // Only a shadow: no model.
    auto only = ctx.world().spawn(ponko::transform3d_at({0, 0, 6}));
    ctx.set_shadow_mesh(only, fence_shadow);
}
// ANCHOR_END: shadow_mesh

// ANCHOR: lod
/// A 2 m object: the sphere until it's 25% of the screen's height, the cube
/// down to 3%, then nothing. The sphere crossfades into the cube over the top
/// quarter of its range.
static ponko::entity lod_object(ponko::ctx& ctx, ponko::mesh high, ponko::mesh low) {
    auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
    ponko::lod_level near_level{0.25f, 0.25f, {ponko::lod_renderer_of(high)}};
    ponko::lod_level far_level{0.03f, 0, {ponko::lod_renderer_of(low)}};
    ctx.set_lod_group(e, 2, {near_level, far_level});
    // Lower quality: drop detail at 70% of the distance.
    ctx.set_lod({0.7f, true});
    return e;
}
// ANCHOR_END: lod

// ANCHOR: occlusion
/// A town or a forest: skip what houses, hills and trunks hide.
static void cull_hidden(ponko::ctx& ctx) {
    ctx.set_occlusion_culling(true);
}
// ANCHOR_END: occlusion

// ANCHOR: profiling
/// F3 shows the performance overlay; F4 records the next 300 frames as a
/// trace to open in Perfetto. Slow stretches are logged for investigation.
static void perf_keys(ponko::ctx& ctx) {
    static bool hud = false;
    if (ctx.key_pressed(PONKO_KEY_F3)) ctx.set_perf_hud(hud = !hud);
    if (ctx.key_pressed(PONKO_KEY_F4)) ctx.record_trace("trace.json", 300);
    auto stats = ctx.frame_stats();
    if (stats.frames > 0 && stats.p99_ms > 20 && ctx.frame() % 600 == 0) {
        // Display wait includes pacing; inspect measured CPU/GPU work separately.
        std::printf("slow: p99 %.1f ms, gpu %.2f ms\n", stats.p99_ms, stats.gpu_ms);
    }
}
// ANCHOR_END: profiling

// ANCHOR: uploads
/// Logs frames that send the GPU far more than a still scene does (which
/// sends next to nothing): many models moved or spawned, or a buffer
/// outgrown and made anew.
static void upload_watch(ponko::ctx& ctx) {
    auto u = ctx.gpu_uploads();
    if (u.bytes > 256 * 1024 || u.reallocations > 0) {
        std::printf("frame %llu: %llu KiB uploaded (%llu KiB of models), %u buffers outgrown\n",
            (unsigned long long)ctx.frame(), (unsigned long long)(u.bytes / 1024),
            (unsigned long long)(u.instances / 1024), u.reallocations);
    }
}
// ANCHOR_END: uploads

// ANCHOR: profile_scopes
// ANCHOR: count_allocations
// In one .cpp file, at global scope: count C++ `new` calls in the reports.
PONKO_COUNT_CPP_ALLOCATIONS
// ANCHOR_END: count_allocations

/// Time the parts of your own code you suspect.
static void steering(ponko::ctx& ctx) {
    PONKO_PROFILE_SCOPE("steering");
    {
        PONKO_PROFILE_SCOPE("steering/neighbors");
        volatile float x = 0;
        for (int i = 0; i < 2000; ++i) x = x + std::sqrt(float(i));
    }
    (void)ctx;
}
// ANCHOR_END: profile_scopes

// ANCHOR: profile_live
/// F5 starts timing your scopes and counting allocations; F6 prints where
/// the last half minute went.
static void perf_report_keys(ponko::ctx& ctx) {
    static bool on = false;
    if (ctx.key_pressed(PONKO_KEY_F5)) ctx.set_profiling(on = !on);
    if (ctx.key_pressed(PONKO_KEY_F6)) std::printf("%s", ctx.perf_report().text().c_str());
}
// ANCHOR_END: profile_live

// ANCHOR: profile_headless
/// Run the game headless for 200 ticks (after 20 warm-up ticks) and keep it
/// within budget. Call it from a test or a CI job; keep time limits generous.
static void steering_stays_within_budget() {
    auto run = ponko::game("bench", 1, 1)
        .sim_system(steering, "steering")
        .profile_headless(20, 200, [](ponko::ctx&) {});
    std::printf("%s", run.report.text().c_str());
    std::string broken = run.report.check(R"({
        "frame_p99_ms": 50,
        "systems": {"steering": 10},
        "scopes": {"steering/neighbors": 10}
    })");
    if (!broken.empty()) throw std::runtime_error("over budget:\n" + broken);
}
// ANCHOR_END: profile_headless

// ANCHOR: distance_fog
/// A big open world fading out far away, glowing toward the sun.
static void far_haze(ponko::ctx& ctx) {
    auto sky = ctx.sky();
    sky.fog_start = 500;        // clear up to half a kilometre
    sky.fog_end = 1200;         // and gone in the haze past 1.2 km
    sky.fog_sun_glow = 0.06f;   // the haze glows toward the sun
    sky.fog_sun_focus = 0.7f;   // most of all close around it
    ctx.set_sky(sky);
}
// ANCHOR_END: distance_fog

// ANCHOR: light_shafts
/// Beams of morning sun through the trees.
static void sunbeams(ponko::ctx& ctx) {
    auto sky = ctx.sky();
    sky.fog_density = 0.03f;    // low mist to light up
    sky.fog_sun_glow = 0.1f;    // glowing in the sun
    sky.light_shafts = 1;       // but not in the shade
    ctx.set_sky(sky);
    auto light = ctx.lighting();
    light.shadows = true;       // shafts follow the sun's shadows
    ctx.set_lighting(light);
    auto post = ctx.post();
    post.anti_aliasing = uint32_t(ponko::anti_aliasing::taa); // smooth their grain
    ctx.set_post(post);
}
// ANCHOR_END: light_shafts

// ANCHOR: post
/// A warm, glowing evening look.
static void golden_hour(ponko::ctx& ctx) {
    auto post = ctx.post();
    // ANCHOR: ao_aa
    post.ao_enabled = true;
    post.ao_half_resolution = true;
    post.ao_temporal = true;
    post.ao_radius = 1.5f; // world units
    post.ao_intensity = 1.0f;
    post.ao_slices = 3;
    post.ao_steps = 6;
    post.contact_shadows = 0.3f; // small, sharp sun shadows under feet and props
    post.anti_aliasing = uint32_t(ponko::anti_aliasing::smaa);
    // ANCHOR_END: ao_aa
    post.exposure = 0.3f;      // a third of a stop brighter
    post.bloom = 0.2f;         // highlights glow
    post.temperature = 0.4f;   // warmer
    post.saturation = 1.1f;
    post.vignette = 0.3f;      // darker corners
    post.tonemap = uint32_t(ponko::tonemap::agx);
    ctx.set_post(post);
}
// ANCHOR_END: post

// ANCHOR: lens
/// The rush of speed: colors split at the edges, the middle pinches in.
static void speed_lens(ponko::ctx& ctx, float speed) {
    float rush = std::clamp(speed / 40.0f, 0.0f, 1.0f);
    auto post = ctx.post();
    post.chromatic_aberration = 0.8f * rush;
    post.lens_distortion = -0.26f * rush;
    post.lens_scale = 1.0f + 0.04f * rush; // hide the pulled-in edges
    ctx.set_post(post);
}
// ANCHOR_END: lens

// ANCHOR: ssr
/// Wet streets and polished floors reflect what's on screen.
static void screen_reflections(ponko::ctx& ctx) {
    auto post = ctx.post();
    post.ssr = 1.0f;
    post.ssr_max_distance = 40.0f; // world units along the reflected ray
    post.anti_aliasing = uint32_t(ponko::anti_aliasing::taa); // smooths their grain
    ctx.set_post(post);
    // Glossy surfaces blur it: full strength up to roughness 0.4, gone by 0.7.
    auto wet = ponko::material_of(ponko::rgb(.2f, .2f, .22f));
    wet.roughness = 0.1f;
    (void)wet;
}
// ANCHOR_END: ssr

// ANCHOR: gi
/// Sunlight bounces off walls and floors onto other surfaces.
static void bounced_light(ponko::ctx& ctx) {
    auto light = ctx.lighting();
    light.gi_enabled = true;
    light.gi_extent = {48, 16, 48};  // the probe grid around the camera
    light.gi_spacing = 2.0f;         // a probe every 2 world units
    light.gi_probes_per_frame = 4;   // how many look around each frame
    light.gi_intensity = 1.0f;
    ctx.set_lighting(light);
}
// ANCHOR_END: gi

// ANCHOR: quality
/// An options menu: pick a preset, then change single settings on top of it.
static void options_menu(ponko::ctx& ctx) {
    ctx.set_graphics_quality(ponko::graphics_quality_preset(ponko::quality_level::medium));
    auto quality = ctx.graphics_quality();
    quality.render_scale = 0.8f; // draw the 3D scene at 80% and scale it up
    quality.sharpen = 0.3f;
    quality.shadow_resolution = 4096;
    ctx.set_graphics_quality(quality);
    // The settings are the ordinary ones in ctx.post() and ctx.lighting().
    auto level = ponko::graphics_quality_level(ctx.graphics_quality()); // quality_level::custom
    (void)level;
}
// ANCHOR_END: quality

// ANCHOR: gpu_tiers
/// At startup: the preset for this GPU's tier, fitted to what it has.
static void quality_for_this_gpu(ponko::ctx& ctx) {
    auto quality = ctx.recommended_graphics_quality() // empty before a GPU opens
        .value_or(ponko::graphics_quality_for_tier(ponko::gpu_tier::desktop));
    ctx.set_graphics_quality(quality);
    if (auto tier = ctx.gpu_tier(); tier && *tier == ponko::gpu_tier::webgl2) {
        // No compute shaders: TAA draws FXAA, AO and reflections are off.
    }
    // What a weaker GPU lacks, with no such GPU at hand: a JSON report of the
    // tier's floor, each missing feature with its fallback and the reason.
    std::string baseline = ponko::gpu_tier_report(ponko::gpu_tier::baseline);
    bool decals = ponko::gpu_tier_has(ponko::gpu_tier::baseline, ponko::gpu_feature::decals); // false
    (void)baseline; (void)decals;
}
// ANCHOR_END: gpu_tiers

// ANCHOR: taa
/// Temporal antialiasing, and a camera cut.
static void temporal_antialiasing(ponko::ctx& ctx) {
    auto post = ctx.post();
    post.anti_aliasing = uint32_t(ponko::anti_aliasing::taa);
    post.taa_sharpness = 0.3f; // it softens the image a little
    ctx.set_post(post);
    // After teleporting the camera (or an object), forget the old view:
    ctx.reset_temporal_history();
}
// ANCHOR_END: taa

// ANCHOR: reset_motion
/// Teleports one object: only its motion is dropped, the rest of the
/// picture keeps its temporal history.
static void teleport_one(ponko::ctx& ctx, ponko::entity hero) {
    ctx.world().set(hero, ponko::transform3d_at({100, 0, 0}));
    ctx.reset_motion(hero);
}
// ANCHOR_END: reset_motion

// ANCHOR: shader
/// Grass that sways in the wind: the higher a vertex, the more it moves.
static const char* wind_shader = R"(
fn vertex(v: Vertex) -> Vertex {
    var out = v;
    let sway = sin(frame.time.x * 2.0 + v.position.x) * material.custom[0].x;
    out.position.x += sway * v.uv.y * v.uv.y;
    return out;
})";

static void meadow(ponko::ctx& ctx) {
    auto look = ponko::material_of(ponko::rgb(.5f, .66f, .24f));
    look.has_shader = true;
    look.shader = ctx.create_shader("wind", wind_shader);
    look.double_sided = true;
    look.custom[0] = 0.2f;
    auto grass = ctx.create_material(look);
    auto blade = ctx.create_mesh(ponko::mesh_data::plane(0.5f), grass);
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(blade));
}
// ANCHOR_END: shader

// ANCHOR: effect
/// Under water, light fades to deep blue with distance (and the sky above
/// is gone). `material.custom[0].x` is the water's height.
static const char* underwater_effect = R"(
fn effect(p: Pixel) -> vec3<f32> {
    if frame.camera.y > material.custom[0].x {
        return p.color; // above the water: nothing changes
    }
    let deep = vec3<f32>(0.02, 0.12, 0.18);
    let fade = 1.0 - exp(-p.distance * 0.08);
    return mix(p.color * vec3<f32>(0.6, 0.9, 1.0), deep, fade);
})";

static void dive(ponko::ctx& ctx) {
    auto underwater = ctx.create_effect("underwater.wgsl", underwater_effect);
    auto desc = ponko::material_of(ponko::colors::white);
    desc.custom[0] = 2; // water at y = 2
    auto lake = ctx.create_material(desc);
    auto list = ctx.effects();
    list.push_back(ponko::screen_effect_of(underwater, lake));
    ctx.set_effects(list);
}
// ANCHOR_END: effect

// ANCHOR: compute
/// Ripples: each texel moves toward the average of its neighbours,
/// overshooting like water. Red is the height now, green the height a
/// frame ago; `material.custom[0]` drops a stone (uv x, uv y, strength).
static const char* ripples_compute = R"(
fn compute(c: Cell) -> vec4<f32> {
    let around = cell_at(c.pixel + vec2<i32>(1, 0)).x + cell_at(c.pixel - vec2<i32>(1, 0)).x
        + cell_at(c.pixel + vec2<i32>(0, 1)).x + cell_at(c.pixel - vec2<i32>(0, 1)).x;
    var height = (around * 0.5 - c.value.y) * 0.985;
    let drop = material.custom[0];
    if drop.z > 0.0 && distance(c.uv, drop.xy) * f32(c.size.x) < 3.0 {
        height -= drop.z;
    }
    return vec4<f32>(height, c.value.x, 0.0, 0.0);
})";

struct ripples_state { ponko::texture heights; ponko::material stones; };

/// The simulated heights, for a water material to read, and the material
/// that drops stones into them.
static ripples_state ripples(ponko::ctx& ctx) {
    auto heights = ctx.create_render_target(256, 256);
    auto ripples = ctx.create_compute("ripples", ripples_compute);
    auto stones = ctx.create_material(ponko::material_of(ponko::colors::white));
    auto list = ctx.compute_passes();
    list.push_back(ponko::compute_pass_of(ripples, heights, stones));
    ctx.set_compute_passes(list);
    return {heights, stones};
}

/// Drops a stone at `uv` (0 to 1 across the water) this frame; `strength`
/// 0 drops none.
static void drop_stone(ponko::ctx& ctx, ponko::material stones, ponko::vec2 uv, float strength) {
    auto desc = ponko::material_of(ponko::colors::white);
    desc.custom[0] = uv.x;
    desc.custom[1] = uv.y;
    desc.custom[2] = strength;
    ctx.update_material(stones, desc);
}
// ANCHOR_END: compute

// ANCHOR: mirror
/// A still pond: its color is the world mirrored in it, which a target
/// camera below the surface draws into `custom_texture0`, upside down.
static const char* mirror_shader = R"(
fn surface(s: Surface) -> Surface {
    var out = s;
    let uv = s.screen * frame.screen.zw;
    out.emissive = textureSample(custom_texture0, material_sampler, vec2<f32>(uv.x, 1.0 - uv.y)).rgb * 0.9;
    out.albedo = vec3<f32>(0.0);
    return out;
})";

static void pond(ponko::ctx& ctx) {
    auto size = ctx.window_size();
    auto reflection = ctx.create_render_target(std::uint32_t(size.x / 2), std::uint32_t(size.y / 2));
    auto look = ponko::material_of(ponko::colors::white);
    look.has_shader = true;
    look.shader = ctx.create_shader("mirror", mirror_shader);
    look.has_custom_textures[0] = true;
    look.custom_textures[0] = reflection;
    auto surface = ctx.create_mesh(ponko::mesh_data::plane(20), ctx.create_material(look));
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(surface));
    ctx.set_target_cameras({ctx.reflection_camera(ctx.camera3d(), reflection, 0)});
}

/// Every frame: the mirrored camera follows the real one.
static void follow_reflections(ponko::ctx& ctx) {
    auto list = ctx.target_cameras();
    for (auto& t : list) t = ctx.reflection_camera(ctx.camera3d(), t.target, 0);
    ctx.set_target_cameras(list);
}
// ANCHOR_END: mirror

// ANCHOR: terrain_material
static float hill_height(float x, float z) { return std::sin(x * 0.02f) * 6 + std::cos(z * 0.03f) * 4; }

/// Three ground layers, speckled so they don't look flat (in a game,
/// ctx.load_texture_array from image files), and one weight map over
/// all nine tiles from the heights: red grass, green dirt, blue rock.
static ponko::material hill_ground(ponko::ctx& ctx) {
    std::vector<std::uint8_t> pixels;
    for (auto base : {std::array<int, 3>{70, 120, 50}, {110, 85, 60}, {120, 120, 115}})
        for (std::uint32_t i = 0; i < 64 * 64; ++i) {
            int shade = int((i * 2654435761u) >> 27); // 0..31
            for (int c : base) pixels.push_back(std::uint8_t(std::min(c + shade, 255)));
            pixels.push_back(255);
        }
    auto layers = ctx.create_texture_array(64, 64, pixels, ponko::texture_kind::color);
    const std::uint32_t cells = 96;
    const float area = 384; // 4 m a pixel, from (-128, -128)
    std::vector<std::uint8_t> weights;
    for (std::uint32_t i = 0; i < cells * cells; ++i) {
        float x = float(i % cells) * area / float(cells) - 128, z = float(i / cells) * area / float(cells) - 128;
        float up = std::clamp((hill_height(x, z) + 10) / 20, 0.0f, 1.0f); // 0 low, 1 high
        float rock = std::clamp((up - 0.7f) * 4, 0.0f, 1.0f);
        float dirt = std::clamp(1 - std::abs(up - 0.5f) * 4, 0.0f, 1.0f);
        float grass = 1 - std::max(rock, dirt);
        for (float w : {grass, dirt, rock, 0.0f}) weights.push_back(std::uint8_t(w * 255));
    }
    auto weight_map = ctx.create_texture_array(cells, cells, weights, ponko::texture_kind::data);
    auto desc = ponko::terrain_material_of(layers, weight_map, {-128, -128}, {area, area});
    const float sizes[] = {6, 4, 8};
    std::copy(std::begin(sizes), std::end(sizes), desc.layer_size);
    return ctx.create_terrain_material(desc);
}
// ANCHOR_END: terrain_material

// ANCHOR: terrain
/// Rolling hills: 3 x 3 tiles of 128 m, each from 65 x 65 heights 2 m
/// apart, sharing their edges, with a pond-sized hole in the middle tile.
/// Grass in the valleys, dirt higher up and rock on the tops.
static void hills(ponko::ctx& ctx) {
    auto ground = hill_ground(ctx);
    const int n = 65;
    const float size = 128;
    for (int tz = -1; tz <= 1; ++tz)
        for (int tx = -1; tx <= 1; ++tx) {
            ponko::vec3 corner{float(tx) * size, 0, float(tz) * size};
            std::vector<float> heights(n * n);
            for (int i = 0; i < n * n; ++i) {
                float x = float(i % n) * 2 + corner.x, z = float(i / n) * 2 + corner.z;
                bool hole = tx == 0 && tz == 0 && std::hypot(x - 64, z - 64) < 6;
                heights[i] = hole ? NAN : hill_height(x, z);
            }
            auto tile = ctx.create_terrain_tile(heights, size, ground);
            ctx.world().spawn(ponko::transform3d_at(corner), tile);
        }
    auto cam = ctx.camera3d();
    cam.position = {64, 30, 220};
    ctx.set_camera3d(cam);
    ctx.camera3d_look_at({64, 0, 64});
}
// ANCHOR_END: terrain

// ANCHOR: ground_height
/// Trees standing on a 128 m tile: each at the ground's height under it,
/// none over a hole.
static void plant_trees(ponko::ctx& ctx, const std::vector<float>& heights, ponko::vec3 corner, ponko::mesh tree) {
    ponko::heightfield ground(heights, 128);
    for (int i = 0; i < 20; ++i) {
        float x = float(i * 37 % 128), z = float(i * 71 % 128);
        if (auto y = ground.height_at(x, z))
            ctx.world().spawn(ponko::transform3d_at({corner.x + x, corner.y + *y, corner.z + z}), ponko::model_of(tree));
    }
}
// ANCHOR_END: ground_height

// ANCHOR: streaming
/// Ground that loads around the camera: tiles 128 m wide within 600 m,
/// built on worker threads, freed past 800 m, and coarse far ground
/// beyond them.
static float hill(float x, float z) { return std::sin(x * 0.01f) * std::cos(z * 0.013f) * 20.0f; }

/// The far ground: 8 km around the start, a height every 32 m.
static std::vector<float> far_heights() {
    std::vector<float> heights(257 * 257);
    for (std::size_t i = 0; i < heights.size(); ++i)
        heights[i] = hill(-4096 + float(i % 257) * 32, -4096 + float(i / 257) * 32);
    return heights;
}

struct Ground {
    struct Tile {
        std::future<ponko::terrain_tile_data> building;
        ponko::entity entity = 0;   // once built (`building` is then spent)
        ponko::terrain_tile tile{};
    };
    ponko::cell_streamer cells{128, 600, 800};
    std::map<std::pair<int, int>, Tile> tiles;
    ponko::material material;
    ponko::far_terrain far;

    explicit Ground(ponko::material m) : material(m), far(far_heights(), 257, {-4096, -4096}, 32, 128, m) {}

    /// Every frame: start up to 4 tiles, drop the far ones, and put up to
    /// 2 finished ones in the world.
    void update(ponko::ctx& ctx) {
        cells.update(ctx.camera3d().position, 4);
        for (auto cell : cells.to_load()) {
            auto corner = cells.corner(cell);
            tiles[{cell.x, cell.z}].building = std::async(std::launch::async, [corner] {
                // 65 x 65 heights, 2 m apart.
                std::vector<float> heights(65 * 65);
                for (std::size_t i = 0; i < heights.size(); ++i)
                    heights[i] = hill(corner.x + float(i % 65) * 2, corner.z + float(i / 65) * 2);
                return ponko::terrain_tile_data(heights, 128);
            });
        }
        for (auto cell : cells.to_drop()) {
            auto found = tiles.find({cell.x, cell.z});
            if (found == tiles.end()) continue;
            if (!found->second.building.valid()) {
                ctx.world().despawn(found->second.entity);
                ctx.free_terrain_tile(found->second.tile);
                far.uncut(cell);
            }
            tiles.erase(found);   // a tile still building is waited for here
        }
        int uploads = 0;
        for (auto& [cell, t] : tiles) {
            if (uploads == 2) break;
            if (!t.building.valid() || t.building.wait_for(std::chrono::seconds(0)) != std::future_status::ready) continue;
            t.tile = ctx.upload_terrain_tile(t.building.get(), material);
            t.entity = ctx.world().spawn(ponko::transform3d_at(cells.corner({cell.first, cell.second})), t.tile);
            far.cut({cell.first, cell.second});   // shown now: cut the far ground away under it
            ++uploads;
        }
        far.update(ctx);
    }
};
// ANCHOR_END: streaming

static void the_village_has_five_spinning_houses() {
    auto world = ponko::game("guide", 640, 480).sim_system(spin, "spin").run_headless(60, [](ponko::ctx& ctx) {
        village(ctx);
        golden_hour(ctx);
        speed_lens(ctx, 40.0f);
        quality_for_this_gpu(ctx);
        if (ponko::graphics_quality_level(ctx.graphics_quality()) != ponko::quality_level::high) { std::fprintf(stderr, "tier preset\n"); std::abort(); }
        options_menu(ctx);
        temporal_antialiasing(ctx);
        teleport_one(ctx, ctx.world().spawn(ponko::transform3d_at({0, 0, 0})));
        screen_reflections(ctx);
        if (ctx.post().ssr != 1.0f) { std::fprintf(stderr, "ssr\n"); std::abort(); }
        bounced_light(ctx);
        if (!ctx.lighting().gi_enabled) { std::fprintf(stderr, "gi\n"); std::abort(); }
        trilight(ctx);
        if (!ctx.lighting().has_equator) { std::fprintf(stderr, "trilight\n"); std::abort(); }
        misty_morning(ctx);
        far_haze(ctx);
        sunbeams(ctx);
        auto tuft = ctx.create_mesh(ponko::mesh_data::cube(0.1f), ctx.default_material());
        plant_meadow(ctx, tuft);
        yard(ctx, tuft, tuft, tuft, tuft, tuft);
        recolored(ctx, tuft, ctx.create_material(ponko::material_of(ponko::colors::white)), ctx.default_material());
        {
            std::vector<ponko::entity> models;
            ctx.world().each([&](ponko::entity e, const ponko::model&) { models.push_back(e); });
            std::size_t overridden = 0, listed = 0;
            for (auto e : models) {
                overridden += ctx.model_material(e).has_value();
                listed += ctx.model_part_materials(e).size();
            }
            if (overridden != 2 || listed != 1) { std::fprintf(stderr, "model materials\n"); std::abort(); }
        }
        auto lod = lod_object(ctx, tuft, tuft);
        cull_hidden(ctx);
        if (!ctx.occlusion_culling()) { std::fprintf(stderr, "occlusion\n"); std::abort(); }
        ctx.set_occlusion_culling(false);
        if (ctx.lod().bias != 0.7f || ctx.lod_pick_of(lod).kind == 0) { std::fprintf(stderr, "lod\n"); std::abort(); }
        ctx.set_lod({1, true});
        perf_keys(ctx);
        upload_watch(ctx);
        if (ctx.gpu_uploads().bytes != 0) { std::fprintf(stderr, "gpu uploads\n"); std::abort(); }
        perf_report_keys(ctx);
        if (ctx.frame_stats().frames != 0) { std::fprintf(stderr, "frame stats\n"); std::abort(); }
        if (ctx.mesh_shadows(tuft)) { std::fprintf(stderr, "mesh shadows\n"); std::abort(); }
        meadow(ctx);
        if (ctx.post().tonemap != uint32_t(ponko::tonemap::agx)) { std::fprintf(stderr, "post\n"); std::abort(); }
    });
    int houses = 0;
    world.each([&](const ponko::transform3d& t, const Spin&) {
        ++houses;
        if (std::abs(t.rotation.y - std::sin(0.25f)) > 1e-3f) { std::fprintf(stderr, "rotation %f\n", t.rotation.y); std::abort(); }
    });
    if (houses != 5) { std::fprintf(stderr, "%d houses\n", houses); std::abort(); }
}

static void the_dive_adds_one_effect() {
    ponko::game("dive", 64, 64).run_headless(1, [](ponko::ctx& ctx) {
        dive(ctx);
        auto list = ctx.effects();
        if (list.size() != 1 || !list[0].has_material) { std::fprintf(stderr, "effects\n"); std::abort(); }
    });
}

static void the_ripples_run_on_a_render_target() {
    ponko::game("ripples", 64, 64).run_headless(1, [](ponko::ctx& ctx) {
        auto state = ripples(ctx);
        drop_stone(ctx, state.stones, {0.5f, 0.5f}, 1);
        auto list = ctx.compute_passes();
        if (list.size() != 1 || list[0].target.id != state.heights.id || !list[0].has_material) {
            std::fprintf(stderr, "ripples\n");
            std::abort();
        }
    });
}

static void the_pond_mirrors_the_camera() {
    ponko::game("pond", 64, 64).run_headless(1, [](ponko::ctx& ctx) {
        pond(ctx);
        auto cam = ctx.camera3d();
        cam.position = {3, 5, 1};
        ctx.set_camera3d(cam);
        follow_reflections(ctx);
        auto list = ctx.target_cameras();
        if (list.size() != 1 || list[0].camera.position.y != -5 || list[0].target.width != 32) {
            std::fprintf(stderr, "pond\n");
            std::abort();
        }
    });
}

static void the_hills_are_nine_tiles() {
    auto world = ponko::game("hills", 640, 480).run_headless(1, [](ponko::ctx& ctx) { hills(ctx); });
    int tiles = 0;
    world.each([&](const ponko::terrain_tile& t) {
        ++tiles;
        if (t.lod_distance != 128) { std::fprintf(stderr, "lod distance %f\n", t.lod_distance); std::abort(); }
    });
    if (tiles != 9) { std::fprintf(stderr, "%d tiles\n", tiles); std::abort(); }
}

static void the_ground_streams_in_and_out() {
    ponko::game("ground", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        Ground ground(ctx.create_material(ponko::material_of(ponko::colors::white)));
        auto live = [&] { std::size_t n = 0; for (auto& [c, t] : ground.tiles) n += !t.building.valid(); return n; };
        for (int i = 0; i < 100000 && (live() < ground.tiles.size() || ground.tiles.empty()); ++i) ground.update(ctx);
        auto first = live();
        if (first < 20) { std::fprintf(stderr, "%zu tiles\n", first); std::abort(); }
        if (ground.far.mesh_count() != 16 || !ground.far.is_cut({0, 0})) { std::fprintf(stderr, "far ground\n"); std::abort(); }
        auto camera = ctx.camera3d();
        camera.position = {10000, 0, 0};
        ctx.set_camera3d(camera);
        ground.update(ctx);
        if (ground.cells.to_drop().size() != first) { std::fprintf(stderr, "dropped %zu\n", ground.cells.to_drop().size()); std::abort(); }
    });
}

static void the_trees_stand_on_the_ground() {
    auto world = ponko::game("trees", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        auto tree = ctx.create_mesh(ponko::mesh_data::cube(1), ctx.create_material(ponko::material_of(ponko::colors::white)));
        std::vector<float> heights(65 * 65, 3.0f);
        heights[0] = NAN;
        plant_trees(ctx, heights, {128, 0, 0}, tree);
    });
    int trees = 0;
    world.each([&](const ponko::transform3d& t, const ponko::model&) {
        ++trees;
        if (std::abs(t.position.y - 3) > 1e-4f) { std::fprintf(stderr, "tree at %f\n", t.position.y); std::abort(); }
    });
    if (trees != 19) { std::fprintf(stderr, "%d trees\n", trees); std::abort(); }
}

// ANCHOR: reflection_probes
static void reflection_probes(ponko::ctx& ctx) {
    auto volume = ponko::reflection_probe3d_default();
    volume.size = {12, 4, 8};
    auto probe = ctx.world().spawn(ponko::transform3d_at({0, 2, 0}), volume);
    ctx.bake_reflection_probe(probe);
    // To use a file instead: ctx.load_reflection_probe(probe, "room.exr");
}
// ANCHOR_END: reflection_probes

// ANCHOR: projected_decals
static void projected_decals(ponko::ctx& ctx) {
    // Images and normal maps share one texture array: layer 0 is a tyre mark,
    // layer 1 its normal map. Make it with texture_kind::data.
    std::vector<std::uint8_t> pixels(2 * 2 * 4 * 2, 255);
    ctx.set_decal_textures(ctx.create_texture_array(2, 2, pixels, ponko::texture_kind::data));
    auto mark = ponko::decal3d_default();
    mark.size = {1.5f, 3.0f, 0.5f}; // width, length, depth
    mark.image = 0;
    mark.normal_image = 1;
    mark.roughness = 0.9f;
    mark.max_distance = 40;
    // Looking down: the box's +Z turned to point up.
    auto at = ponko::transform3d_at({4, 0.1f, 2});
    at.rotation = ponko::quat_from_rotation_x(-1.5707964f);
    auto tyre = ctx.world().spawn(at, mark);
    (void)tyre;
    // A character that no decal lands on:
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::receives_decals{0});
}
// ANCHOR_END: projected_decals

// ANCHOR: punctual_lights
static void punctual_lights(ponko::ctx& ctx) {
    ctx.world().spawn(ponko::transform3d_at({0, 3, 0}),
        ponko::point_light3d{ponko::rgb(1, .3f, .1f), 30, 12, false});
    auto aim = ponko::looking_at(ponko::transform3d_at({4, 3, 0}), {0, 0, 0});
    ctx.world().spawn(aim,
        ponko::spot_light3d{ponko::colors::white, 60, 20, .2f, .5f, true}); // casts shadows
}
// ANCHOR_END: punctual_lights

// ANCHOR: light_shadow_atlas
/// A street at night: a sharp shadow for the lamp beside the player,
/// rough ones for the street lights down the road.
static void street_lights(ponko::ctx& ctx) {
    auto light = ctx.lighting();
    light.light_shadow_atlas = 8192; // room for more shadow views
    light.light_shadow_views = 24;   // as many as there can be
    ctx.set_lighting(light);
    ponko::point_light3d lamp{ponko::rgb(1, .8f, .6f), 40, 12, true};
    lamp.shadow_resolution = 2048; // always sharp
    ctx.world().spawn(ponko::transform3d_at({0, 3, 0}), lamp);
    for (int i = 0; i < 18; i++) {
        float x = -20 + 2.5f * i;
        auto at = ponko::looking_at(ponko::transform3d_at({x, 5, -30}), {x, 0, -29.9f});
        // Sized by how big they look: smaller the farther they are.
        ctx.world().spawn(at, ponko::spot_light3d{ponko::colors::white, 60, 8, .3f, .5f, true});
    }
}
// ANCHOR_END: light_shadow_atlas

// ANCHOR: skin_chain
constexpr int chain_joints = 8;
constexpr float chain_step = 0.5f;
static std::vector<ponko::joint> chain() {
    std::vector<ponko::joint> joints;
    for (int i = 0; i < chain_joints; i++)
        joints.push_back({"joint_" + std::to_string(i), i - 1, ponko::transform3d_at({0, i == 0 ? 0 : chain_step, 0})});
    return joints;
}
// ANCHOR_END: skin_chain

// ANCHOR: skin_geometry
static ponko::mesh_data tube(float bottom, float top, float radius) {
    const int slices = 12, rows = 28;
    std::vector<ponko::vec3> positions;
    std::vector<std::uint32_t> indices;
    std::vector<std::array<std::uint16_t, 4>> joints;
    std::vector<std::array<float, 4>> weights;
    for (int row = 0; row <= rows; row++) {
        float y = bottom + (top - bottom) * row / rows;
        float along = std::clamp(y / chain_step, 0.0f, float(chain_joints - 1));
        int lower = std::min(int(along), chain_joints - 2);
        float blend = along - lower;
        for (int slice = 0; slice < slices; slice++) {
            float angle = 6.2831853f * slice / slices;
            positions.push_back({radius * std::cos(angle), y, radius * std::sin(angle)});
            joints.push_back({std::uint16_t(lower), std::uint16_t(lower + 1), 0, 0});
            weights.push_back({1 - blend, blend, 0, 0});
        }
    }
    for (std::uint32_t row = 0; row < rows; row++)
        for (std::uint32_t slice = 0; slice < slices; slice++) {
            std::uint32_t a = row * slices + slice, b = row * slices + (slice + 1) % slices;
            indices.insert(indices.end(), {a, a + slices, b, b, a + slices, b + slices});
        }
    ponko::mesh_data data(positions, indices);
    data.set_skin(joints, weights);
    return data;
}
// ANCHOR_END: skin_geometry

static void skinned_tentacle(ponko::ctx& ctx) {
    auto green = ctx.default_material(), gold = ctx.default_material();
    // ANCHOR: skin_create
    auto tentacle = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
    ctx.set_skeleton(tentacle, chain());
    auto mesh = ctx.create_skinned_mesh(tube(0, 3.5f, 0.22f), green, tentacle);
    ctx.world().set(tentacle, ponko::model_of(mesh));
    // ANCHOR_END: skin_create
    auto ball = ctx.create_mesh(ponko::mesh_data::sphere(0.15f, 16), green);
    // ANCHOR: skin_parts
    // Clothes: a mesh bound to the same joint names, posed by the tentacle.
    auto band = ctx.create_skinned_mesh(tube(2.2f, 2.5f, 0.25f), gold, tentacle);
    ctx.set_skin_of(ctx.world().spawn(ponko::model_of(band)), tentacle);
    // A ball riding the tip, a little above it.
    auto tip = *ctx.find_joint(tentacle, "joint_7");
    auto rider = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(ball));
    ctx.attach_to_joint(rider, tentacle, tip, ponko::transform3d_at({0, 0.12f, 0}));
    // ANCHOR_END: skin_parts
    // ANCHOR: skin_pose
    float time = float(ctx.time());
    for (std::size_t i = 0; i < ctx.joint_count(tentacle); i++) {
        auto pose = ctx.joint_pose(tentacle, i);
        pose.rotation = ponko::quat_from_rotation_z(std::sin(time * 1.5f - i * 0.45f) * 0.16f);
        ctx.set_joint_pose(tentacle, i, pose);
    }
    // ANCHOR_END: skin_pose
}

[[maybe_unused]] static void loaded_character(ponko::ctx& ctx) {
    // ANCHOR: skin_load
    auto mesh = ctx.load_mesh("models/character.glb");
    auto character = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(mesh));
    ctx.load_skeleton(character, "models/character.glb");
    auto hand = ctx.find_joint(character, "hand_r").value();
    auto pose = ctx.joint_pose(character, hand);
    pose.rotation = ponko::quat_from_rotation_y(0.3f);
    ctx.set_joint_pose(character, hand, pose);
    auto m = ctx.joint_matrix(character, hand); // model space, column-major
    std::printf("Hand at %g %g %g (model space)\n", m[12], m[13], m[14]);
    // ANCHOR_END: skin_load
    // ANCHOR: clip_play
    auto walk = ctx.load_clip("models/character.glb", "walk");
    auto run = ctx.load_clip("models/character.glb", "run");
    float t = float(ctx.time());
    ctx.reset_pose(character);
    ctx.sample_clip(walk, character, std::fmod(t, walk.duration()));
    ctx.sample_clip(run, character, std::fmod(t, run.duration()), 0.5f);
    // ANCHOR_END: clip_play
    // ANCHOR: animator_graph
    ponko::animation_graph_builder b;
    auto moving = b.add_bool();
    auto idle = b.add_state(ponko::motion::of(ctx.load_clip("models/character.glb", "idle")));
    auto walking = b.add_state(ponko::motion::of(walk));
    b.add_condition(b.add_transition(idle, walking, 0.12f), PONKO_CONDITION_BOOL, moving, 1);
    b.add_condition(b.add_transition(walking, idle, 0.12f), PONKO_CONDITION_BOOL, moving, 0);
    auto graph = b.build(idle); // shared: give it to every character
    ctx.set_animator(character, graph);
    // ANCHOR_END: animator_graph
    // ANCHOR: animator_update
    ctx.animator_set_bool(character, moving, true);
    // Or pose it yourself instead of every frame:
    ctx.animator_set_manual(character, true);
    ctx.animator_update(character, 0.016f);
    ctx.animator_crossfade(character, idle, 0.12f);
    while (auto name = ctx.animator_next_event(character)) std::printf("Animation event: %s\n", name->c_str());
    // ANCHOR_END: animator_update
    // ANCHOR: animator_layers
    ponko::animation_graph_builder upper;
    upper.add_state(ponko::motion::of(ctx.load_clip("models/character.glb", "wave")));
    auto waving = upper.build(0);
    // The wave plays on the spine and everything under it, over the walk.
    auto wave = ctx.animator_add_layer(character, waving, ponko::layer_blend::override_pose, "spine");
    ctx.animator_set_layer_weight(character, wave, 0.8f);
    // The walk's own steps move the character, instead of sliding in place.
    ctx.animator_set_root_motion(character, ponko::root_motion::apply, *ctx.find_joint(character, "hips"));
    // ANCHOR_END: animator_layers
}

static void joints_and_dominance(ponko::ctx& ctx) {
    auto world = ctx.world();
    // ANCHOR: joints
    auto a = world.spawn(ponko::transform3d_at({0, 0, 0}), ponko::fixed_body3d(), ponko::sphere_collider(0.1f));
    auto b = world.spawn(ponko::transform3d_at({0, -1, 0}), ponko::dynamic_body3d(), ponko::sphere_collider(0.2f));
    auto joint = ponko::spherical_joint3d(a, b);
    joint = ponko::with_limits(joint, ponko::joint_axis::angular_x, -0.785f, 0.785f);
    joint.swing_cone = 0.524f;
    joint = ponko::with_drive(joint, ponko::joint_axis::angular_x, ponko::spring_drive(0, 100, 20));
    joint.frame_b.position = {0, 1, 0};
    joint.contacts_enabled = false;
    auto connection = world.spawn(joint);
    // Despawn `connection` to detach; either body's despawn also removes the joint.
    // ANCHOR_END: joints
    (void)connection;
    // ANCHOR: dominance
    auto pusher = ponko::with_dominance(ponko::dynamic_body3d(), 1);
    auto pushed = ponko::dynamic_body3d(); // group 0
    // Both remain dynamic: gravity, walls and equal-group bodies still affect them.
    world.spawn(ponko::transform3d_at({3, 0, 0}), pusher, ponko::box_collider({1, 1, 1}));
    world.spawn(ponko::transform3d_at({4, 0, 0}), pushed, ponko::box_collider({1, 1, 1}));
    // ANCHOR_END: dominance
}

// A stick figure on a floor, knocked over and later stood back up.
static void ragdoll(ponko::ctx& ctx) {
    auto world = ctx.world();
    world.spawn(ponko::transform3d_at({0, -0.5f, 0}), ponko::fixed_body3d(), ponko::box_collider({20, 1, 20}));
    auto enemy = world.spawn(ponko::transform3d_at({0, 0, 0}));
    ctx.set_skeleton(enemy, {ponko::joint{"hips", -1, ponko::transform3d_at({0, 1, 0})},
                             ponko::joint{"spine", 0, ponko::transform3d_at({0, 0.6f, 0})},
                             ponko::joint{"head", 1, ponko::transform3d_at({0, 0.4f, 0})},
                             ponko::joint{"arm", 1, ponko::transform3d_at({0.5f, 0.2f, 0})},
                             ponko::joint{"hand", 3, ponko::transform3d_at({0.5f, 0, 0})}});
    ponko::vec3 hit{1, 0, 0};
    // ANCHOR: ragdoll
    // A hit knocks `enemy` over: physics takes over its skeleton.
    ctx.start_ragdoll(enemy, {
        ponko::ragdoll_bone("hips", 0.12f),
        ponko::ragdoll_bone("spine", 0.1f).swing(0.5f),
        ponko::ragdoll_bone("head", 0.1f),
        ponko::ragdoll_bone("arm", 0.05f).swing(1.2f),
    });
    auto spine = ctx.ragdoll_body(enemy, *ctx.find_joint(enemy, "spine"));
    ctx.apply_impulse3d(*spine, {hit.x * 40, hit.y * 40, hit.z * 40});
    // When it gets up: bodies gone, half a second back to the animation.
    ctx.stop_ragdoll(enemy, 0.5f);
    // ANCHOR_END: ragdoll
}

// Platforms you pass up through, a belt, and a pair the physics step ignores.
static void contacts(ponko::ctx& ctx) {
    auto world = ctx.world();
    // ANCHOR: contacts
    // A platform to jump up through: solid only from above.
    world.spawn(ponko::transform3d_at({0, 3, 0}), ponko::fixed_body3d(), ponko::box_collider({4, 0.2f, 4}),
                ponko::one_way_modifier3d({0, 1, 0}));
    // A belt carrying whatever stands on it along +X at 2 m/s.
    world.spawn(ponko::transform3d_at({10, 0, 0}), ponko::fixed_body3d(), ponko::box_collider({8, 0.2f, 1}),
                ponko::conveyor_modifier3d({2, 0, 0}));
    // A hook for the rest: the ghost walks through the hooked wall.
    auto ghost = world.spawn(ponko::transform3d_at({-10, 1, 0}), ponko::dynamic_body3d(), ponko::sphere_collider(0.5f));
    world.spawn(ponko::transform3d_at({-10, 1, 3}), ponko::fixed_body3d(), ponko::box_collider({4, 2, 0.2f}),
                ponko::hook_modifier3d());
    ctx.set_contact_hook3d([ghost](ponko::contact_pair3d& pair) {
        if (pair.a() == ghost || pair.b() == ghost) pair.ignore();
    });
    // ANCHOR_END: contacts
}

// ANCHOR: skin_ik
// Register this with ponko::game(...).late_system(procedural_pose).
[[maybe_unused]] static void procedural_pose(ponko::ctx& ctx, ponko::entity character) {
    ponko::vec3 target{0.7f, 0.6f, 0.2f};
    ctx.two_bone_ik(character, 0, 1, 2, target, {0, 0, 1});
    // Apply parent first when distributing aim across spine/neck/head.
    ctx.look_at_joint(character, 5, {0, 1, 0}, target, 0.3f, 0.2f);
    ctx.look_at_joint(character, 6, {0, 1, 0}, target, 0.5f, 0.5f);
    ctx.look_at_joint(character, 7, {0, 1, 0}, target, 0.8f, 1.0f);
}

[[maybe_unused]] static void debug_bones(ponko::ctx& ctx, ponko::entity character) {
    ctx.draw_skeleton(character, ponko::colors::red);
}
// ANCHOR_END: skin_ik

// ANCHOR: morph
// System or setup: everything here runs headless too.
static void morphed_sheet(ponko::ctx& ctx) {
    // One target, named to find it again: at full weight its third corner
    // sinks 0.4 into the sheet. Leave the normals empty to keep the
    // shading, or give one offset per corner to bend the light along.
    ponko::morph_target dent{"dent", std::vector<ponko::vec3>(4), {}};
    dent.positions[2] = {0, 0, -0.4f};
    auto sheet = ponko::mesh_data::plane(1.0);
    sheet.add_morph_target(dent);
    auto halfway = dent;
    halfway.name = "dent_half";
    halfway.positions[2] = {0.1f, 0, -0.15f};
    sheet.add_morph_target(halfway);
    sheet.add_morph_frame(0, 1, 0.5f);
    sheet.add_morph_frame(0, 0, 1.0f);

    auto mesh = ctx.create_mesh(sheet, ctx.default_material());
    auto at = ctx.morph_target(mesh, "dent").value();
    auto panel = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(mesh));
    ctx.set_morph_weight(panel, at, 0.5f); // authored halfway frame
    ctx.set_skeleton(panel, {}); // morph-only clips need no joints
    ponko::clip expression("expression");
    expression.set_morph_track("dent", {0, 1}, {0, 1});
    ctx.sample_clip(expression, panel, 0.5f);
}
// ANCHOR_END: morph

// ANCHOR: sound3d
static void sound3d(ponko::ctx& ctx) {
    float samples[4410];
    for (int i = 0; i < 4410; i++) samples[i] = 0.1f * std::sin(i * 220.0f * 6.2831853f / 44100.0f);
    auto hum = ctx.create_sound(44100, samples, 4410);
    auto settings = ponko::audio_settings3d_defaults();
    settings.sound.volume = 0.4f;
    settings.sound.looping = true;
    settings.min_distance = 2;
    settings.max_distance = 60;
    settings.rolloff = PONKO_ROLLOFF_LINEAR;
    auto e = ctx.world().spawn(ponko::transform3d_at({4, 0, 0}));
    ctx.set_audio_source3d(e, hum, settings);
    ctx.play_sound_at(hum, {-4, 0, 0});
}
// ANCHOR_END: sound3d

// ANCHOR: audio_mixer
static void buses_and_effects(ponko::ctx& ctx, ponko::sound sound) {
    auto music = ctx.audio_bus("music"); // Save this handle; names are looked up once.
    ctx.set_audio_bus_volume(music, 0.6f);
    auto params = ponko::sound_defaults();
    params.looping = true;
    auto voice = ctx.play_sound_on(sound, music, params, {4000, 0.2f});
    ctx.set_sound_effects(voice, {800, 0.5f});
    ctx.mute_audio_bus(music, true);
    ctx.mute_audio_bus(music, false); // Restores the configured gain.
    auto settings = ponko::audio_settings3d_defaults();
    settings.bus = music;
    settings.effects = {2000, 0.3f};
    ctx.play_sound_at(sound, {3, 0, 0}, settings);
}
// ANCHOR_END: audio_mixer

// ANCHOR: audio_stream
[[maybe_unused]] static void streaming_music(ponko::ctx& ctx) {
    auto music = ctx.audio_bus("music");
    auto song = ctx.load_stream("music.ogg");
    auto params = ponko::sound_defaults();
    params.looping = true;
    ctx.play_sound_on(song, music, params);
}
// ANCHOR_END: audio_stream

static bool messages_round_trip() {
    // ANCHOR: messages
    // The game owns the endpoint. In a game, poll it and read its events in
    // a frame system, including frames where the simulation is paused.
    auto host = ponko::net_messages::host("127.0.0.1:0", 8);
    auto client = ponko::net_messages::join(("127.0.0.1:" + std::to_string(host.local_port())).c_str());
    bool received = false;
    for (int frame = 0; frame < 5000 && !received; frame++) {
        client.poll();
        while (auto e = client.next_event()) {
            if (e->kind == PONKO_MESSAGE_CONNECTED) client.send(0, PONKO_LANE_RELIABLE, std::string("hello"));
            if (e->kind == PONKO_MESSAGE_DISCONNECTED) std::fprintf(stderr, "%s\n", e->reason.c_str());
        }
        host.poll();
        while (auto e = host.next_event()) {
            if (e->kind != PONKO_MESSAGE) continue;
            std::printf("received: %.*s\n", int(e->bytes.size()), reinterpret_cast<const char*>(e->bytes.data()));
            host.send(e->peer, PONKO_LANE_UNRELIABLE, std::string("42"));
            received = true;
        }
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
    }
    // ANCHOR_END: messages
    return received;
}

static bool replication_snippets() {
    // ANCHOR: snapshots
    // One stream per remote entity: it holds the snapshots received and learns
    // the sender's clock from their timestamps. The wire format is yours.
    ponko::snapshot_stream car; // shown 100 ms in the past
    ponko::latest_wins newest;  // drops duplicate and late packets
    for (std::uint16_t n = 0; n < 20; n++) {
        // In a game this is a decoded packet from net_messages::next_event():
        std::uint64_t entity = 7;
        double sender_time = 5000.0 + n * 0.05; // on the sender's clock
        double arrived = n * 0.05 + 0.03;       // on yours, e.g. ctx.time()
        auto pose = ponko::moving(ponko::pose_at({float(n), 0, 0}), {20, 0, 0});
        if (newest.accept(entity, ponko::update_stamp{0, n})) car.push(sender_time, arrived, pose);
    }
    // Every frame: where to draw it. Between snapshots it is interpolated;
    // when packets are late it is extrapolated for 250 ms, then held.
    auto shown = car.sample(1.0);
    std::printf("x = %.1f\n", shown->value.position.x);
    // ANCHOR_END: snapshots
    bool ok = shown && shown->state == ponko::snapshot_state::interpolated && std::fabs(shown->value.position.x - 17.4f) < 0.05f;

    // ANCHOR: authority
    const std::uint64_t host = 0, guest = 1;
    ponko::authority vehicle(host);               // the host owns it...
    std::uint16_t epoch = vehicle.handoff(guest);  // ...until the guest takes the wheel.
    // Announce (guest, epoch) on a reliable lane; every other peer calls
    // vehicle.adopt(guest, epoch) when it arrives. State updates carry the
    // epoch they were sent under, and the receiver checks it:
    bool late = vehicle.check(host, 0) == ponko::authority_check::stale; // from before the handoff
    bool current = vehicle.check(guest, epoch) == ponko::authority_check::accept;
    bool wraps = ponko::seq_newer(2, 65535); // sequence numbers wrap around
    // ANCHOR_END: authority
    ok = ok && late && current && wraps;

    // ANCHOR: interest
    // Near entities every tick, mid ones every 4th, far ones never.
    ponko::interest interest;
    const std::uint64_t peer = 0; // from PONKO_MESSAGE_CONNECTED; the host is 0
    std::vector<ponko::interest_viewer> viewers{{peer, {0, 0, 0}}};
    std::vector<ponko::interest_entity> world{{1, {10, 0, 0}}, {2, {500, 0, 0}}};
    int sent = 0;
    for (std::uint64_t tick = 0; tick < 8; tick++) {
        interest.update(tick, viewers, world);
        for (auto& change : interest.changes())
            std::printf("entity %llu %s\n", (unsigned long long)change.entity,
                        change.kind == PONKO_INTEREST_ENTERED ? "entered" : "left");
        for (auto& entry : interest.entries(peer))
            if (entry.send) sent++; // send entity entry.entity to this peer now
    }
    // ANCHOR_END: interest
    return ok && sent == 8;
}

static void campfire(ponko::ctx& ctx) {
    // ANCHOR: particles
    auto fire = ponko::particle_settings()
                    .rate(120)
                    .burst(40)
                    .lifetime(0.7f, 1.4f)
                    .speed(2, 4)
                    .cone(0.45f)
                    .gravity({0, -3, 0})
                    .drag(0.2f)
                    .size(0.05f, 0.13f)
                    .rotation(0, 6.2831853f)
                    .size_over_life({{0, 0.5f}, {0.2f, 1}, {1, 0}})
                    .color_over_life({{0, {1, 0.7f, 0.2f, 1}}, {1, {1, 0.1f, 0, 0}}})
                    .blend(ponko::particle_blend::additive)
                    .max_particles(512)
                    .seed(42);
    auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
    ctx.set_particle_emitter3d(e, fire);
    ctx.particle_burst(e, 20); // more at any time
    // ANCHOR_END: particles
}

static void sparks2d(ponko::ctx& ctx) {
    // ANCHOR: particles2d
    auto sparks = ponko::particle_settings2d()
                      .rate(80)
                      .cone(0.4f)
                      .speed(120, 240)
                      .lifetime(0.6f, 1.2f)
                      .gravity({0, 260})
                      .size(2, 4)
                      .color_over_life({{0, {1, 0.9f, 0.4f, 1}}, {1, {1, 0.3f, 0, 0}}})
                      .blend(ponko::sprite_blend::additive)
                      .z(2);
    auto e = ctx.world().spawn(ponko::transform_at({400, 300}));
    ctx.set_particle_emitter2d(e, sparks);
    ctx.particle_burst2d(e, 30); // more at any time
    // ANCHOR_END: particles2d
}

static void lights2d(ponko::ctx& ctx) {
    std::vector<std::uint8_t> stone(32 * 16 * 4, 200), bumps(32 * 16 * 4, 128);
    const auto tiles = ctx.create_texture(32, 16, stone.data(), stone.size());
    const auto tile_normals = ctx.create_texture(32, 16, bumps.data(), bumps.size());
    const auto crate = tiles;
    const std::uint16_t FLOOR = 0, WALL = 1;
    const ponko::entity room = ctx.world().spawn(ponko::transform_at({0, 0}));
    auto map = ctx.world().add_tilemap(room, ponko::sheet_of(tiles, 16, 16), 20, 15);
    for (std::uint32_t y = 0; y < 15; ++y)
        for (std::uint32_t x = 0; x < 20; ++x)
            map.set(x, y, x == 0 || y == 0 || x == 19 || y == 14 ? WALL : FLOOR);
    // ANCHOR: lights2d
    // Dark blue where no light reaches; this turns 2D lighting on.
    ctx.set_ambient_light2d(ponko::rgb(0.1f, 0.11f, 0.18f));
    // A warm torch whose light the walls and the crate block.
    auto torch = ponko::point_light2d_default();
    torch.color = ponko::rgb(1, 0.6f, 0.3f);
    torch.radius = 250;
    torch.shadows = true;
    ctx.world().spawn(ponko::transform_at({160, 120}), torch);
    // A lantern: a cone along its rotation (here, to the right).
    auto lantern = ponko::spot_light2d_default();
    lantern.angle = 0.4f;
    ctx.world().spawn(ponko::transform_at({60, 60}), lantern);
    // The walls (tile 1) of the map cast shadows; so does a crate.
    ctx.set_tilemap_occluder(room, {WALL});
    const auto box = ctx.world().spawn(ponko::transform_at({220, 150}), ponko::sprite_of(crate));
    ctx.set_occluder2d_rect(box, {16, 16});
    // Bumpy tiles: light shades their bevels.
    ctx.set_normal_map(tiles, tile_normals);
    // ANCHOR_END: lights2d
    if (ctx.occluder2d_edge_count(room) != 8 || ctx.occluder2d_edge_count(box) != 4 || !ctx.ambient_light2d() ||
        ctx.normal_map(tiles)->id != tile_normals.id) {
        std::fprintf(stderr, "2D lights guide\n");
        std::abort();
    }
}

static void ribbons(ponko::ctx& ctx) {
    auto white = ctx.default_material();
    auto floor_data = ponko::mesh_data::plane(12);
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(ctx.create_mesh(floor_data, white)));

    // ANCHOR: trail
    // A glowing tracer behind a flying ball: a quarter second long, thin at
    // the end and fading out.
    auto tracer = ponko::trail3d(0.25f, 0.12f)
                      .color_over_age({{0, {1, 0.8f, 0.3f, 1}}, {1, {1, 0.3f, 0, 0}}})
                      .blend(ponko::particle_blend::additive);
    auto ball = ctx.create_mesh(ponko::mesh_data::sphere(0.15f, 16), white);
    auto flyer = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(ball));
    ctx.set_trail3d(flyer, tracer);
    // After a teleport: ctx.clear_trail3d(flyer);
    // ANCHOR_END: trail

    // ANCHOR: line
    // A laser beam 5 m long, from the entity forward.
    auto beam = ponko::line3d({{0, 0, 0}, {0, 0, -5}}, 0.03f, ponko::colors::red)
                    .blend(ponko::particle_blend::additive);
    auto emitter = ctx.world().spawn(ponko::transform3d_at({-4, 1.5f, 3}));
    ctx.set_line3d(emitter, beam);
    // ANCHOR_END: line

    // ANCHOR: decal
    // A 1 m scorch mark on the floor at (1, 0, -1): the box is 1 x 1 m wide
    // and 0.4 m deep, its +Z turned up, so the image is projected down.
    // The projector's 16 floats go column after column.
    std::array<float, 16> projector = {1, 0, 0, 0,   0, 0, -1, 0,   0, 0.4f, 0, 0,   1, 0, -1, 1};
    auto mark = floor_data.decal(projector, 0.003f);
    if (mark.view().index_count > 0) {
        auto look = ponko::material_of({0.05f, 0.04f, 0.03f, 0.8f});
        look.blend = 1; // transparent
        auto scorch = ctx.create_mesh(mark, ctx.create_material(look));
        // Drawn where the floor is.
        ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(scorch));
    }
    // A mesh from a glTF file works the same: ctx.load_mesh_data("models/wall.glb").decal(..)
    // ANCHOR_END: decal
}

static void navigation_guide() {
    ponko::vec3 destination{8, 0, 0}, start{};
    auto world = ponko::game("Navigation server", 1, 1)
        .fixed_hz(60)
        // ANCHOR: nav_agents
        .navmesh3d() // moves agents every simulation tick
        .run_headless(600, [&](ponko::ctx& ctx) {
            // ANCHOR_END: nav_agents
            // ANCHOR: nav_bake
            ponko::nav_sources sources;
            sources.add_mesh(ponko::mesh_data::plane(30), ponko::transform3d_at({0, 0, 0}));
            sources.add_mesh(ponko::mesh_data::cube(4), ponko::transform3d_at({0, 2, 0}));
            auto settings = ponko::nav_bake_defaults();
            settings.carving = true; // keep layers so carving obstacles can cut holes
            ctx.bake_navmesh(sources, settings);
            // ANCHOR_END: nav_bake
            // ANCHOR: nav_query
            start = ctx.navmesh_sample_position({-8, 1, 0}, 2).value();
            auto corners = ctx.navmesh_find_path(start, destination);
            if (corners.size() < 3) std::abort();
            // ANCHOR_END: nav_query
            // ANCHOR: nav_agent_spawn
            auto agent = ctx.world().spawn(ponko::transform3d_at(start));
            ctx.set_nav_agent(agent, 3);
            // ANCHOR: nav_avoidance
            ctx.set_nav_avoidance(agent, true, 2.0f, 3.6f);
            // ANCHOR_END: nav_avoidance
            ctx.set_nav_destination(agent, destination);
            // A world-space box; move it or remove it to reopen polygons.
            ctx.set_nav_obstacle(ctx.world().spawn(), {0, 1, 8}, {1, 1, 1});
            // ANCHOR_END: nav_agent_spawn
            // ANCHOR: nav_carve
            ctx.set_nav_carve_obstacle(ctx.world().spawn(), {6, 1, 6}, {1, 1, 1});
            // Simulation gathers components and rebuilds one cached layer per tick.
            // After a tick, optional extra work reports steps and pending updates:
            auto carving = ctx.navmesh_update_carving(0);
            (void)carving;
            // ANCHOR_END: nav_carve
        });
    (void)world;
}

// ---- Retained UI --------------------------------------------------------------
static void build_shop(ponko::ctx& ctx) {
    // ANCHOR: shop_tree
    auto tree = ctx.ui_retained();
    auto root = tree.add_panel(ui::none, ui::fill().padding(24).gap(12));
    tree.set_name(tree.add_label(root, "General store — choose an item", ui::fixed(400, 32)), "title");
    tree.add_text_input(root, "Your order note", ui::fixed(360, 36));
    auto scroll = tree.add_scroll(root, ui::fill().height(ui::points(360)));
    // 95% wide, so the scrollbar doesn't cover the last column.
    auto grid = tree.add_panel(scroll, ui::layout().grid({ui::fr(1), ui::fr(1), ui::fr(1)})
        .auto_rows({ui::track_points(76)}).size(ui::percent(0.95f), ui::automatic()).gap(8).shrink(0));
    for (int i = 1; i <= 36; ++i) {
        auto label = "Item " + std::to_string(i) + "\n" + std::to_string(i * 3) + " coins";
        auto item = tree.add_button(grid, label.c_str(), ui::layout().padding(8));
        tree.set_paint(item, ui::paint().wrap().border_width(1));
        tree.set_name(item, ("item." + std::to_string(i)).c_str());
    }
    tree.add_toggle(root, "Gift wrap", false, ui::fixed(220, 28));
    auto buy = tree.add_button(root, "Buy selected item", ui::fixed(220, 40));
    tree.set_enabled(buy, false);
    tree.set_name(buy, "buy");
    tree.push_screen(root);
    // ANCHOR_END: shop_tree

    // ANCHOR: ui_grid_areas
    auto dashboard = tree.add_panel(ui::none, ui::layout()
        .grid({ui::repeat(2, ui::fr(1))})
        .grid_rows({ui::track_points(40), ui::fr(1)})
        .areas({"header header", "side main"}).size(600, 400));
    tree.add_label(dashboard, "Dashboard", ui::layout().area("header"));
    tree.add_label(dashboard, "Navigation", ui::layout().area("side"));
    tree.add_label(dashboard, "Content", ui::layout().area("main"));
    // ANCHOR_END: ui_grid_areas
    tree.remove(dashboard);

    // ANCHOR: ui_named_span
    // Four 100-point columns; lines 3 and 5 are both called "edge".
    auto strip = tree.add_panel(ui::none, ui::layout().grid({ui::track_points(100),
        ui::track_points(100), ui::track_points(100), ui::track_points(100)}).size(400, 40));
    tree.set_grid_line_names(strip, 0, {"", "", "edge", "", "edge"});
    // From line 1 to the second "edge" line (CSS `span 2 edge`): all four columns.
    auto banner = tree.add_label(strip, "Sale", ui::layout().grid_column(ui::line(1)));
    tree.set_grid_named_span(banner, 0, true, "edge", 2);
    // ANCHOR_END: ui_named_span
    tree.remove(strip);

    // ANCHOR: ui_theme
    auto theme = tree.theme();
    theme.accent = {1.0f, 0.6f, 0.1f, 1.0f};
    theme.title_size = 32;
    tree.set_theme(theme); // nodes keep the paint set on them
    // ANCHOR_END: ui_theme

    // ANCHOR: shop_dialog
    // A detached screen can be reused after it is popped: 180 points from the
    // top here (without `top`, in the middle of the window).
    auto dialog = tree.add_panel(ui::none, ui::centered(360, 180).top(180).padding(20).gap(12));
    tree.set_name(dialog, "dialog");
    tree.set_name(tree.add_label(dialog, "", ui::fixed(320, 40)), "message");
    tree.set_name(tree.add_button(dialog, "Confirm purchase", ui::fixed(240, 36)), "confirm");
    tree.set_name(tree.add_button(dialog, "Back", ui::fixed(240, 30)), "cancel");
    // ANCHOR_END: shop_dialog

    // ANCHOR: ui_access
    // A slider has no text of its own to be named by.
    auto volume = tree.add_slider(root, 0.5f, ui::fixed(240, 24));
    tree.set_accessible_name(volume, "Volume");
    tree.set_accessible_description(buy, "Buys the highlighted item");
    // A change to a live region is announced while focus is elsewhere.
    auto status = tree.add_label(root, "Ready", ui::fixed(400, 24));
    tree.set_live_region(status, ui::live::polite);
    // A request from assistive technology; the next frame applies it.
    tree.access_action(ui::access_action_kind::increment, volume);
    // What changed since the last look, for a backend of your own.
    ui::access access;
    auto changes = access.update(tree);
    for (auto node : changes.changed) (void)access.name(node);
    // ANCHOR_END: ui_access
}

// ANCHOR: shop_events
static void shop_events(ponko::ctx& ctx) {
    // The UI ran before this system. Nodes are found by name.
    auto tree = ctx.ui_retained();
    auto clicked = tree.take_click();
    const std::string name = clicked ? tree.name(*clicked) : "";
    const ui::id buy = *tree.find("buy");
    if (name.rfind("item.", 0) == 0) {
        const std::string item = name.substr(5);
        tree.set_enabled(buy, true);
        tree.set_text(buy, ("Buy item " + item).c_str());
        tree.set_text(*tree.find("message"), ("Buy item " + item + "?").c_str());
    } else if (name == "buy") {
        tree.push_screen(*tree.find("dialog"), true);
    } else if (name == "confirm" || name == "cancel") {
        tree.pop_screen();
        if (name == "confirm") tree.set_text(*tree.find("title"), "Purchase complete!");
    }
    // Tab/arrows or d-pad navigate; Enter/South confirms; Escape/East pops the
    // top screen. A modal screen also owns the pointer outside it.
    if (!tree.wants_pointer()) { /* world pointer actions */ }
    if (!tree.wants_keyboard()) { /* world keyboard/gamepad actions */ }
}
// ANCHOR_END: shop_events

/// Clicks the middle of the node named `name`, the way a mouse would.
static void click_node(ponko::ctx& ctx, const char* name) {
    auto tree = ctx.ui_retained();
    const ponko::vec2 window{900, 700};
    tree.layout_for(window);
    const auto r = tree.rect(*tree.find(name));
    const ponko::vec2 at = (r.min + r.max) * 0.5f;
    auto input = ui::default_input();
    input.pointer = at;
    input.pressed = input.held = true;
    tree.frame(window, input);
    input.pressed = input.held = false;
    input.released = true;
    tree.frame(window, input);
}

static void shop_guide(ponko::ctx& ctx) {
    build_shop(ctx);
    auto tree = ctx.ui_retained();
    click_node(ctx, "item.3");
    shop_events(ctx);
    click_node(ctx, "buy");
    shop_events(ctx);
    if (tree.top_screen() != tree.find("dialog") || tree.text(*tree.find("message")) != "Buy item 3?") std::abort();
    tree.layout_for({900, 700});  // the dialog was pushed after the last layout
    const auto dialog = tree.rect(*tree.find("dialog"));
    if (dialog.min.x != 270 || dialog.min.y != 180) std::abort();
    click_node(ctx, "confirm");
    shop_events(ctx);
    if (tree.text(*tree.find("title")) != "Purchase complete!" || tree.top_screen() == tree.find("dialog")) std::abort();
}

// ---- UI on a surface in the world ---------------------------------------------
static void panel_guide(ponko::ctx& ctx) {
    // ANCHOR: panel_create
    // A 1.2 x 0.8 m terminal, 600 x 400 pixels, with a button.
    auto terminal = ctx.world().spawn(ponko::transform3d_at({0, 1.5f, -2}));
    ctx.create_ui_panel(terminal, 600, 400, {1.2f, 0.8f});
    auto panel = ctx.panel_ui(terminal);
    auto root = panel.add_panel(ui::none, ui::fill().padding(20).gap(12));
    panel.set_name(panel.add_button(root, "Buy", ui::fixed(200, 60)), "buy");
    panel.push_screen(root);
    // ANCHOR_END: panel_create

    // Click it by hand, as a headless test would (a window's mouse does this itself).
    panel.layout_for({600, 400});
    auto input = ui::default_input();
    input.pointer = {100, 50};
    input.pressed = input.held = true;
    panel.frame({600, 400}, input);
    input.pressed = input.held = false;
    input.released = true;
    panel.frame({600, 400}, input);

    // ANCHOR: panel_events
    // Aim with the middle of the screen instead of the mouse (for a locked
    // cursor, or a controller in VR).
    auto eye = ctx.camera3d();
    ctx.set_ui_pointer_ray(eye.position, ponko::forward(ponko::transform3d_at(eye.position, eye.rotation)));
    if (auto clicked = panel.take_click(); clicked && panel.name(*clicked) == "buy")
        panel.set_text(*clicked, "Bought!");
    if (auto pixel = ctx.ui_panel_pointer(terminal)) {
        // The aim is over the panel, at this pixel of its 600 x 400.
        (void)pixel;
    }
    // ANCHOR_END: panel_events
    if (panel.text(*panel.find("buy")) != "Bought!") std::abort();
    ctx.clear_ui_pointer_ray();
}

// ANCHOR: text3d
static void world_text_snippet() {
    ponko::game("World text", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 2.5f, 0}));
        ponko::text3d label("OPEN");
        label.size = .3f;
        label.outline = {0, 0, 0, 1};
        label.outline_width = .06f;
        label.billboard = true;
        ctx.set_text3d(e, label);
    });
}
// ANCHOR_END: text3d

// ANCHOR: localized_text
static void localized_text_snippet() {
    ponko::game("Localized text", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        // One font per script the game is translated into, tried in order
        // for characters the built-in font lacks.
        std::vector<ponko::font> fallbacks;
        for (const char* path : {"fonts/NotoSansArabic-Subset.ttf", "fonts/NotoSansJP-Subset.otf"}) {
            if (auto f = ctx.try_load_font(path)) fallbacks.push_back(*f);
        }
        const ponko::font builtin{0};
        ctx.set_font_fallbacks(builtin, fallbacks);
        ctx.set_text_shaping(ponko::text_shaping::automatic);  // the default
        // "Level 3: " and an Arabic word, in UTF-8 like every string the API takes.
        const ponko::vec2 size = ctx.measure_text("Level 3: \xD9\x85\xD8\xB1\xD8\xAD\xD8\xA8\xD8\xA7", builtin, 24);
        // Installed fonts need the library's system-fonts feature.
        if (auto system = ctx.try_load_system_font("Noto Sans")) fallbacks.push_back(*system);
        (void)size;
    });
}
// ANCHOR_END: localized_text

// ANCHOR: gpu_buffers
static void gpu_particles(ponko::ctx& ctx) {
    auto items = ctx.create_gpu_buffer(128);
    auto update = ctx.create_compute("particles", "fn update(i: u32) { items[i].y += frame.time.y; }");
    ctx.set_compute_passes({ponko::buffer_compute_pass(update, items)});
    auto shader = ctx.create_shader("particles", "fn vertex(v: Vertex) -> Vertex { var o = v; o.position += items[v.instance_index].xyz; return o; }");
    auto desc = ponko::material_of(ponko::colors::white);
    desc.has_shader = true;
    desc.shader = shader;
    auto material = ctx.create_material(desc);
    auto mesh = ctx.create_mesh(ponko::mesh_data::cube(0.05f), material);
    ctx.set_mesh_gpu_buffer(mesh, items, 128);
    // The emitter's extent around its entity, for culling.
    ctx.set_mesh_gpu_bounds(mesh, {0, 0, 0}, 20);
    // Each particle's own motion for TAA: the buffer keeps its last frame.
    ctx.set_gpu_buffer_motion(items, true);
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(mesh));
}
// ANCHOR_END: gpu_buffers

// ANCHOR: gpu_instances
void persistent_instances(ponko::ctx& ctx, ponko::mesh mesh) {
    std::vector<std::array<float,16>> transforms;
    for (unsigned i=0; i<100000; ++i) {
        transforms.push_back({1,0,0,0, 0,1,0,0, 0,0,1,0,
            float(i%1000),0,float(i/1000),1});
    }
    auto batch = ctx.create_instance_batch(mesh, transforms, {32.0f,500.0f,true,ponko::colors::white});
    ctx.update_instance_batch(batch, transforms);
    ctx.free_instance_batch(batch);
}
// ANCHOR_END: gpu_instances

// ANCHOR: batch_culling
void choose_batch_culling(ponko::ctx& ctx, ponko::instance_batch batch) {
    // 0 = automatic, 1 = prefer GPU, 2 = prefer CPU.
    ctx.set_instance_batch_culling(batch, 1);
}
// ANCHOR_END: batch_culling

// ANCHOR: instance_levels
void leveled_instances(ponko::ctx& ctx, ponko::mesh near_mesh, ponko::mesh far_mesh) {
    std::vector<ponko::instance_level> levels = {
        {near_mesh, 0, 25, true, false},
        {far_mesh, 25, 100, true, true},
        {far_mesh, 0, 25, false, true},
    };
    std::vector<std::array<float,16>> transforms = {{
        1,0,0,0, 0,1,0,0, 0,0,1,0, 0,0,0,1
    }};
    auto batch = ctx.create_instance_batch_levels(levels, transforms, {ponko::colors::red});
    ctx.update_instance_batch_colors(batch, transforms, {ponko::colors::blue});
    ctx.free_instance_batch(batch);
}
// ANCHOR_END: instance_levels

// ANCHOR: handles
void stream_out(ponko::ctx& ctx) {
    auto before = ctx.live_counts();
    const std::uint8_t brick[4] = {200, 80, 60, 255};
    auto bricks = ctx.create_texture(1, 1, brick, sizeof brick);
    ponko::material_desc desc = ponko::material_of(ponko::colors::white);
    desc.texture = bricks;
    desc.has_texture = true;
    auto wall = ctx.create_material(desc);
    auto block = ctx.create_mesh(ponko::mesh_data::cube(1.0f), wall);
    ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(block));
    // Going out of range: free what the cell made. The model still draws
    // (untextured), until its entity is despawned too.
    ctx.free(bricks);
    ctx.free(wall);
    ctx.free(block);
    bool dead = !ctx.is_live(bricks) && !ctx.is_live(wall) && !ctx.is_live(block);
    bool none = !ctx.is_live(ponko::none<ponko::texture>());
    if (!dead || !none || ctx.live_counts().meshes != before.meshes) std::abort();
}
// ANCHOR_END: handles

// ANCHOR: asset_leaks
void find_leaks(ponko::ctx& ctx) {
    const auto before = ctx.asset_count(ponko::asset_class::material);
    for (int level = 0; level < 3; level++) {
        auto house = ctx.load_mesh("models/house.glb");
        ctx.free(house);  // keeps the file's materials and textures for next time
    }
    // Loading the same file again reuses them: only the first load grew.
    const auto grown = ctx.asset_count(ponko::asset_class::material);
    std::printf("materials: %zu more\n", grown.live - before.live);
    // Which textures are there, how old, and who made them?
    for (const auto& r : ctx.asset_handles(ponko::asset_class::texture)) {
        if (r.owner == static_cast<std::uint32_t>(ponko::asset_owner::file))
            std::printf("texture %u: %llu bytes, %llu frames old, from %s\n", r.id,
                        static_cast<unsigned long long>(r.bytes), static_cast<unsigned long long>(r.age_frames),
                        ctx.asset_owner_name(ponko::asset_class::texture, r.id).c_str());
    }
    // Done with the model for good: free its materials and textures too.
    ctx.unload_mesh("models/house.glb");
    if (ctx.asset_count(ponko::asset_class::material).live != before.live) std::abort();
}
// ANCHOR_END: asset_leaks

// ANCHOR: streaming
void streaming_setup(ponko::ctx& ctx) {
    // Memory caps for what loads by name, and bytes sent to the GPU per frame.
    ctx.set_residency_budget(ponko::residency_kind::texture, 256u << 20);
    ctx.set_residency_budget(ponko::residency_kind::mesh, 512u << 20);
    ctx.set_upload_budget(8u << 20);
}

void draw_crab(ponko::ctx& ctx) {
    // Ask every frame you need it: never waits. Empty until a small
    // stand-in is up, then the same texture, sharper each frame.
    if (auto crab = ctx.stream_texture("crab.png")) ctx.draw(*crab, {0, 0});
    if (ctx.asset_state(ponko::residency_kind::texture, "crab.png") == ponko::asset_state::failed) {
        std::fprintf(stderr, "no crab: %s\n", ctx.asset_error(ponko::residency_kind::texture, "crab.png").c_str());
    }
}
// ANCHOR_END: streaming

// ANCHOR: shader_errors
/// Prints every shader whose last compile or edit didn't take effect, with
/// where in its WGSL the error is; returns how many there are.
static std::size_t report_shader_errors(ponko::ctx& ctx) {
    std::size_t broken = 0;
    for (const auto& s : ctx.shader_status()) {
        if (s.ok()) continue;
        ++broken;
        std::fprintf(stderr, "%s\n", s.errors.c_str());   // water.wgsl:3:12: error: ...
    }
    return broken;
}
// ANCHOR_END: shader_errors

// ANCHOR: declared_access
struct Speed { float value; };
static void move_forward(ponko::ctx& ctx) {
    const float dt = ctx.dt();
    ctx.world().each([&](Pos& p, const Speed& s) { p.y += s.value * dt; });
}

static void declared_access_guide() {
    auto world = ponko::game("declared", 320, 240)
        .add(ponko::phase::after_physics,
             ponko::access().write<Pos>().read<Speed>().read_service(ponko::service::time),
             move_forward, "move_forward")
        .run_headless(2, [](ponko::ctx& ctx) { ctx.world().spawn(Pos{0}, Speed{60}); });
    if (world.size() != 1) { std::fprintf(stderr, "declared access guide failed\n"); std::abort(); }
}
// ANCHOR_END: declared_access

// ANCHOR: parallel_systems
static void parallel_systems_guide() {
    auto game = ponko::game("declared", 320, 240);
    game.parallel_systems()
        .add(ponko::phase::after_physics,
             ponko::access().write<Pos>().read<Speed>().read_service(ponko::service::time),
             move_forward, "move_forward");
    // A C++ system takes the whole ctx, so it runs alone:
    //   AfterPhysics
    //     move_forward (alone: takes `&mut Ctx`)
    const std::string plan = game.schedule_plan();
    if (plan.find("move_forward (alone") == std::string::npos) {
        std::fprintf(stderr, "parallel systems guide failed\n");
        std::abort();
    }
}
// ANCHOR_END: parallel_systems

// ANCHOR: action_ids
struct player_controls { ponko::action_id jump, fire; };

// Once, after set_actions: an unknown name throws here, not every frame.
static player_controls resolve_controls(ponko::ctx& ctx) {
    auto [jump, fire] = ctx.action_ids({"jump", "fire"});
    return {jump, fire};
}

static bool shoots(ponko::ctx& ctx, const player_controls& c) {
    return ctx.action_pressed(c.fire) && !ctx.action_down(c.jump);
}
// ANCHOR_END: action_ids

static void action_ids_run() {
    ponko::game("ids", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ponko::action_map bindings;
        bindings.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_SPACE)}));
        bindings.set("fire", ponko::action::button({ponko::action_binding::key(PONKO_KEY_F)}));
        ctx.set_actions(bindings);
        const player_controls c = resolve_controls(ctx);
        ctx.feed_input(ponko::input_event::key(PONKO_KEY_F, true));
        if (!shoots(ctx, c)) { std::fprintf(stderr, "action ids\n"); std::abort(); }
    });
}

// ANCHOR: input_scenario
static void jump_once() {
    using ponko::scenario_check;
    ponko::input_scenario scenario;
    scenario.named("jump-once")
        .at(10, ponko::input_event::key(PONKO_KEY_SPACE, true)) // pressed on tick 10, held after
        .expect(10, scenario_check::action("jump").pressed(true).down(true))
        .at(12, ponko::input_event::key(PONKO_KEY_SPACE, false))
        .expect(12, scenario_check::action("jump").released(true));
    auto run = ponko::game("test", 1, 1).run_headless_scenario(scenario, [](ponko::ctx& ctx) {
        ponko::action_map bindings;
        bindings.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_SPACE)}));
        ctx.set_actions(bindings);
    });
    if (!run.report.passed()) throw std::runtime_error(run.world.scenario_report_json());
}
// ANCHOR_END: input_scenario

// ANCHOR: headless_frames
static void menu_opens_settings() {
    using ponko::scenario_check;
    ponko::input_scenario scenario;
    scenario.expect(0, scenario_check::ui("menu.settings").shown(true).text("Settings"))
        .click(2, "menu.settings")
        .expect(2, scenario_check::ui("settings").shown(true))
        .expect(2, scenario_check::ui("menu.settings").shown(false));
    auto run = ponko::game("menu test", 800, 600)
        .headless_frames(true) // frames too: the UI and frame systems run
        .system([](ponko::ctx& ctx) {
            auto ui = ctx.ui_retained();
            if (auto clicked = ui.take_click(); clicked && clicked == ui.find("menu.settings")) {
                ui.pop_screen();
                ui.push_screen(*ui.find("settings"));
            }
        })
        .run_headless_scenario(scenario, [](ponko::ctx& ctx) {
            namespace u = ponko::ui;
            auto ui = ctx.ui_retained();
            auto menu = ui.add_panel(u::none, u::fill());
            ui.set_name(ui.add_button(menu, "Settings", u::fixed(200, 40)), "menu.settings");
            ui.set_name(ui.add_panel(u::none, u::fill()), "settings");
            ui.push_screen(menu);
        });
    if (!run.report.passed()) throw std::runtime_error(run.world.scenario_report_json());
}
// ANCHOR_END: headless_frames

// ANCHOR: asset_sources
static void embedded_assets(ponko::ctx& ctx) {
    // Compiled into the executable: nothing to ship next to it.
    static const char tuning[] = R"({"speed": 30})";
    ctx.add_asset_source(ponko::memory_source("embedded")
        .with("tuning.json", std::string(tuning))
        .with("level.txt", std::string("#..#\n#..#\n")));

    // Your own source: bytes for the names it has, nullopt for the rest.
    ctx.add_asset_source("generated", [](const char* name) -> std::optional<std::vector<std::uint8_t>> {
        if (std::string(name) == "noise.bin") return std::vector<std::uint8_t>(64, 7);
        return std::nullopt; // the next source is asked
    });

    // Your own asset type: decode the bytes, keep the result.
    auto walls = ctx.load_asset_with("level.txt", [](const std::uint8_t* data, std::size_t len) {
        return std::count(data, data + len, '#');
    });
    // ANCHOR_END: asset_sources
    if (walls != 4 || ctx.load_asset_with("noise.bin", [](const std::uint8_t*, std::size_t len) { return len; }) != 64) {
        std::fprintf(stderr, "asset sources\n");
        std::abort();
    }
}

// ANCHOR: resource_scope
struct Total { std::uint32_t value; };
struct Coins { std::uint32_t value; };

static void count_coins(ponko::world_view world) {
    world.set_resource(Total{0});
    // The resource and the rest of the world together; Total is back afterwards.
    world.resource_scope<Total>([](ponko::world_view& w, Total& total) {
        w.each([&](const Coins& coins) { total.value += coins.value; });
    });
}
// ANCHOR_END: resource_scope

static void resource_scope_run() {
    ponko::world world;
    world.spawn(Coins{3});
    world.spawn(Coins{4});
    count_coins(world);
    if (world.resource<Total>().value != 7) { std::fprintf(stderr, "resource scope\n"); std::abort(); }
}

// ANCHOR: try_load
static ponko::texture skin_or_default(ponko::ctx& ctx) {
    auto skin = ctx.try_load_texture("skins/gold.png");
    if (!skin) {
        const ponko::error& e = skin.error();
        std::fprintf(stderr, "%s (%s): %s\n", e.path().c_str(), ponko::kind_name(e.kind()), e.what());
    }
    return skin.value_or(ponko::texture{});
}
// ANCHOR_END: try_load

// Snippets of chapters that are mostly about Rust or tools: compiled here so
// their C++ side can't fall behind the header.
namespace book {
struct Health { int value; };
struct Died { ponko::entity who; };
static void hurt(ponko::ctx& ctx) {
    ctx.world().each([&](ponko::entity e, Health& h) { if (--h.value == 0) { ctx.send(Died{e}); ctx.despawn_later(e); } });
}

static void camera_controls_builder() {
    // ANCHOR: camera_controls
    ponko::game("Colony", 1280, 720)
        .camera_controls()       // right/middle drag pans, wheel zooms
    // ANCHOR_END: camera_controls
        .run_headless(1);
}

static void budget_check(const ponko::perf_report& report) {
    // ANCHOR: budget_check
    std::string broken = report.check(R"({
        "frame_p99_ms": 4, "allocs_per_frame": 0,
        "systems": {"steering": 0.5},
        "scopes": {"steering/neighbors": 0.3}
    })");
    if (!broken.empty()) throw std::runtime_error(broken);
    // ANCHOR_END: budget_check
}

static void named_access() {
    auto game = ponko::game("named", 1, 1);
    // ANCHOR: named_access
    // ponko_access_new / ponko_access_write_component("Health") /
    // ponko_access_send_events("Died") / ponko_access_commands, then
    // ponko_game_add_system_with_access. In C++:
    game.add(ponko::phase::after_physics,
             ponko::access().write<Health>().send_events<Died>().commands(),
             hurt, "hurt");
    // ANCHOR_END: named_access
    auto world = game.run_headless(2, [](ponko::ctx& ctx) { ctx.world().spawn(Health{1}); });
    if (world.count<Health>() != 0) { std::fprintf(stderr, "named access guide failed\n"); std::abort(); }
}

// ANCHOR: batch_limits
static void check_batches(ponko::ctx& ctx) {
    // The device's limits are in the capability report's JSON
    // (`instance_batches`); nothing before a GPU opens.
    if (auto caps = ctx.gpu_capabilities()) std::printf("GPU report: %zu bytes\n", caps->size());
    // The per-frame counts (`cpu_over_limit`, `dropped`, `views_skipped`)
    // aren't in the C API yet: read the render inspector's
    // `counts.instance_batches` (`perf.get` on the control endpoint).
}
// ANCHOR_END: batch_limits

// Compiled only: running it would open a local port.
[[maybe_unused]] static void control_endpoint_on() {
    // ANCHOR: control_on
    ponko::game("Colony", 1280, 720).control_endpoint(true)
    // ANCHOR_END: control_on
        .run_headless(0);
}

struct Vitals { float current, max; };
static void described_schema(ponko::ctx& ctx) {
    // ANCHOR: describe_component
    ctx.world().reflect<Vitals>("Vitals", {ponko::field("current", &Vitals::current), ponko::field("max", &Vitals::max)});
    // Fields from reflect() already give a schema; describe_component gives
    // any other shape, encoded as Rust's ValueSchema:
    ctx.world().describe_component("Vitals", R"({"Object":[["current","Number"],["max","Number"]]})");
    std::string answer = ctx.capabilities_json(); // the same `capabilities` answer tools get
    // ANCHOR_END: describe_component
    if (answer.find("\"Vitals\"") == std::string::npos) { std::fprintf(stderr, "capabilities lack Vitals\n"); std::abort(); }
}

// ANCHOR: services_flat
// C++ keeps one flat ctx: the call names Rust spells with a service.
static void flat_calls(ponko::ctx& ctx) {
    const float dt = ctx.dt();                       // ctx.time.dt()
    const double t = ctx.sim_time();                 // ctx.time.sim_time()
    if (ctx.key_pressed(PONKO_KEY_P)) ctx.pause();   // ctx.input..., ctx.time.pause()
    const ponko::vec2 mouse = ctx.mouse_pos();       // ctx.input.mouse_pos()
    auto ui = ctx.ui_retained();                     // ctx.ui
    const bool menu_open = ui.wants_keyboard();      // ctx.ui.wants_keyboard()
    ctx.set_perf_hud(false);                         // ctx.perf.set_hud(false)
    (void)dt; (void)t; (void)mouse; (void)menu_open;
}
// ANCHOR_END: services_flat

static void run_all() {
    camera_controls_builder();
    named_access();
    ponko::game("book snippets", 1, 1).run_headless(1, [](ponko::ctx& ctx) {
        check_batches(ctx);
        described_schema(ctx);
        flat_calls(ctx);
        bool over = false; // nothing was profiled, so every limit reports "nothing measured"
        try { budget_check(ctx.perf_report()); } catch (const std::runtime_error&) { over = true; }
        if (!over) { std::fprintf(stderr, "budget check passed without data\n"); std::abort(); }
    });
}
}  // namespace book

// ANCHOR: scene_compute
static void light_meter(ponko::ctx& ctx) {
    // A running average of the frame's light, in one texel.
    auto meter = ctx.create_compute("meter", R"(
        fn compute(c: Cell) -> vec4<f32> {
            let now = dot(scene_color(frame.screen.xy * 0.5), vec3<f32>(0.2126, 0.7152, 0.0722));
            return vec4<f32>(mix(c.value.r, now, 0.05), 0.0, 0.0, 1.0);
        })");
    auto average = ctx.create_render_target(1, 1);
    auto passes = ctx.scene_compute_passes();
    passes.push_back(ponko::compute_pass_of(meter, average));
    ctx.set_scene_compute_passes(passes);
    // Screen effects (and target effects) read this frame's value; materials
    // and the compute stage read last frame's.
}
// ANCHOR_END: scene_compute

// ANCHOR: lod_forest
// A forest of one tree as a static batch, levels picked where it is culled.
static ponko::instance_batch forest(ponko::ctx& ctx, const std::vector<ponko::lod_level>& tree, float tree_size,
                                    const std::vector<std::array<float, 16>>& transforms) {
    // The copies' largest scale (1 here), the camera's field of view and
    // ctx.lod()'s bias fix the switch distances.
    auto levels = ponko::lod_instance_levels(tree_size, tree, 1.0f, ctx.camera3d().fov_y, ctx.lod());
    if (!levels) throw std::runtime_error("no center offset, renderer offsets or tints");
    return ctx.create_instance_batch_levels(*levels, transforms);
}
// ANCHOR_END: lod_forest

// ANCHOR: ime_own_widget
// A chat box drawn by the game itself: it asks for IME while it is open,
// tells the window where its caret is, and shows the composition there.
struct Chat {
    bool open = false;
    std::string text;
};
static void chat_box(ponko::ctx& ctx, Chat& chat) {
    ctx.set_ime_allowed(chat.open);
    if (!chat.open) return;
    const float caret_x = 20 + ctx.measure_text(chat.text.c_str(), 18).x;
    ctx.set_ime_cursor_area(ponko::rect{{caret_x, 400}, {caret_x + 1, 418}});
    chat.text += ctx.typed_text();          // committed text, IME or not
    const std::string composing = ctx.preedit(); // shown after the text, underlined
    ctx.draw_text((chat.text + composing).c_str(), {20, 400}, 18, ponko::colors::white);
}
// ANCHOR_END: ime_own_widget

int main() {
    builder_setup_runs_headless();
    ponko::game("ime guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        Chat chat{true, "Hi "};
        ctx.feed_ime(ponko::ime_preedit("にほ", 6, 6));
        chat_box(ctx, chat);
        if (!ctx.ime_allowed() || ctx.preedit() != "にほ" || !ctx.ime_cursor_area()) std::abort();
        ctx.feed_ime(ponko::ime_commit("日本"));
        chat_box(ctx, chat);
        if (chat.text != "Hi 日本") { std::fprintf(stderr, "chat: %s\n", chat.text.c_str()); std::abort(); }
    });
    sprite_looks_run();
    action_ids_run();
    jump_once();
    menu_opens_settings();
    resource_scope_run();
    ponko::game("asset source guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        embedded_assets(ctx);
        if (skin_or_default(ctx).id != 0) std::abort();
    });
    ponko::game("filter guide", 1, 1).run_headless(0, ecs_filters);
    ponko::game("query cache guide", 1, 1).run_headless(0, ecs_query_cache);
    plugins_run_headless();
    declared_access_guide();
    parallel_systems_guide();
    steering_stays_within_budget();
    navigation_guide();
    ponko::game("shop guide", 900, 700).run_headless(0, shop_guide);
    ponko::game("panel guide", 960, 640).run_headless(0, panel_guide);
    ponko::game("particle guide", 1, 1).run_headless(0, campfire);
    ponko::game("ribbon guide", 1, 1).run_headless(0, ribbons);
    ponko::game("particle 2d guide", 1, 1).run_headless(0, sparks2d);
    ponko::game("2d lights guide", 1, 1).run_headless(0, lights2d);
    if (!messages_round_trip()) { std::fprintf(stderr, "no message\n"); std::abort(); }
    if (!replication_snippets()) { std::fprintf(stderr, "replication snippets failed\n"); std::abort(); }
    ponko::game("sound guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        sound3d(ctx);
        float silence[16] = {};
        buses_and_effects(ctx, ctx.create_sound(44100, silence, 16));
    });
    ponko::game("shader guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        try {
            ctx.create_shader("water.wgsl", "// still\nfn surface(s: Surface) -> Surface {\n    return 1.0;\n}");
        } catch (const std::exception&) {}
        if (report_shader_errors(ctx) != 1 || ctx.shader_status()[0].info.line != 3) std::abort();
    });
    ponko::game("level guide", 1, 1).physics({0, 700}).data_dir("guide-storage").run_headless(0, levels_guide);
    ponko::game("skinning guide", 1, 1).run_headless(0, skinned_tentacle);
    ponko::game("morph guide", 1, 1).run_headless(0, morphed_sheet);
    ponko::game("joint guide", 1, 1).physics3d({0, 0, 0}).run_headless(5, joints_and_dominance);
    ponko::game("ragdoll guide", 1, 1).physics3d({0, -9.81f, 0}).run_headless(5, ragdoll);
    ponko::game("contacts guide", 1, 1).physics3d({0, -9.81f, 0}).run_headless(5, contacts);
    ponko::game("light guide", 64, 64).run_headless(0, punctual_lights);
    ponko::game("atlas guide", 64, 64).run_headless(0, [](ponko::ctx& ctx) {
        street_lights(ctx);
        if (ctx.lighting().light_shadow_atlas != 8192) { std::fprintf(stderr, "atlas\n"); std::abort(); }
    });
    ponko::game("probe guide", 64, 64).run_headless(0, reflection_probes);
    ponko::game("decal guide", 64, 64).run_headless(0, projected_decals);
    the_ripples_run_on_a_render_target();
    the_pond_mirrors_the_camera();
    the_hills_are_nine_tiles();
    tiles_are_solid_only_near_the_player_and_the_car();
    the_ground_streams_in_and_out();
    the_trees_stand_on_the_ground();
    the_dive_adds_one_effect();
    the_beach_has_five_palms();
    storage_keeps_files_between_runs();
    old_saves::old_saves_still_load();
    a_cart_comes_and_goes_whole();
    the_village_has_five_spinning_houses();
    things_fall_ten_units_per_second();
    turrets_ride_on_tanks();
    crowds_reach_the_goal();
    the_dead_are_buried();
    shots_arrive_before_the_next_system();

    // Run the other snippets headless once, so they're exercised, not only compiled.
    ponko::sound click{};
    ponko::entity map = 0;
    ponko::entity ordered = 0;
    std::uint32_t most = 0;
    auto world = ponko::game("guide", 1, 1)
        .spatial_grid(10)
        .sim_system(die, "die")
        .sim_system(resize_health_bars, "health bars")
        .sim_system(count_neighbors, "neighbors")
        .sim_system(wander, "wander")
        .sim_system(Spawner{2}, "spawner")
        .sim_system(think, "think")
        .sim_system(send_orders, "orders")
        .sim_system(obey_orders, "obey")
        .system(Hud{}, "hud")
        .sim_system(move_players, "players")   // no pads headless: nothing moves
        .camera_controls()
        .sim_system([&](ponko::ctx& ctx) {
            ctx.world().each([&](const Neighbors& n) { most = std::max(most, n.count); });
        }, "check")
        .run_headless(8, [&](ponko::ctx& ctx) {
            ecs(ctx);
            auto w = ctx.world();
            for (int i = 0; i < 16; ++i) w.spawn(Brain{0});
            ordered = w.spawn(ponko::transform_at({0, 0}), Owner{0});
            click = beep(ctx);
            std::vector<std::uint8_t> pixels(64 * 40 * 4, 255);
            const auto sheet = ponko::sheet_of(ctx.create_texture(64, 40, pixels.data(), pixels.size()), 16, 16);
            animate(w, sheet, {0, 0});
            map = make_map(w, sheet, 50);
            paint(ctx, map);   // nobody holds the mouse headless: nothing changes
            ctx.play_sound(click);   // silent: headless
            for (int i = 0; i < 100; ++i)
                w.spawn(ponko::transform_at({float(i), 0}), Neighbors{0}, Heading{0}, Health{float(i % 2)});
        });
    {
        auto tiles = world.tilemap(map);
        std::size_t stone = 0;
        for (std::size_t i = 0; i < std::size_t(tiles.width()) * tiles.height(); ++i) stone += tiles.tiles()[i] == STONE;
        if (tiles.width() != 50 || stone != 500 || tiles.get(0, 0) != STONE || tiles.tile_at({0, 0}, {-1, 5})) {
            std::fprintf(stderr, "tilemap: %u wide, %zu stone\n", tiles.width(), stone);
            return 1;
        }
    }
    // Over 8 ticks every brain gets exactly one turn.
    std::uint32_t plans = 0;
    world.each([&](const Brain& b) { plans += b.plans; });
    if (plans != 16 || std::fabs(click.seconds - 0.1f) > 1e-3f) {
        std::fprintf(stderr, "unexpected: %u plans, %f s click\n", plans, double(click.seconds));
        return 1;
    }
    // 100k from ecs() + 16 brains + 50 survivors + 4 spawned by Spawner{2} in 8 ticks + 1 animated + 1 map + 1 ordered.
    if (world.size() != 100000 + 16 + 50 + 4 + 3 || most < 10) {
        std::fprintf(stderr, "unexpected: %zu entities, %u neighbors\n", world.size(), most);
        return 1;
    }
    // Reflected components survive a save and load with their ids.
    {
        auto w = ponko::game("save", 1, 1).run_headless(0, [](ponko::ctx&) {});
        reflect_units(w);
        const ponko::entity a = w.spawn(Unit{3, 10, {1, 2}, 0, false});
        const ponko::entity b = w.spawn(Unit{5, 10, {0, 0}, a, true});
        const std::string before = w.save();
        w.set(b, Unit{0, 0, {0, 0}, 0, false});
        save_and_load(w);   // saves the changed world and loads it straight back
        w.load(before);
        const Unit u = w.get<Unit>(b);
        if (u.current != 5 || u.target != a || !u.angry || before.find("\"angry\":true") == std::string::npos) {
            std::fprintf(stderr, "save/load lost data: %s\n", before.c_str());
            return 1;
        }
    }
    // Resources: plain structs by type, saved when reflected.
    {
        auto w = ponko::game("resources", 1, 1).run_headless(0, [](ponko::ctx&) {});
        reflect_teams(w);
        w.set_resource(Teams{2, 3});
        if (w.save().find("\"Teams\":{\"blue\":3,\"red\":2}") == std::string::npos) {
            std::fprintf(stderr, "resource not saved: %s\n", w.save().c_str());
            return 1;
        }
    }
    if (!(world.get<ponko::transform2d>(ordered).position == ponko::vec2{100, 50})) {
        std::fprintf(stderr, "offline orders were not applied\n");
        return 1;
    }
    if (!physics_snippets()) {
        std::fprintf(stderr, "physics snippets failed\n");
        return 1;
    }
    if (!physics3d_snippets()) {
        std::fprintf(stderr, "3D physics snippets failed\n");
        return 1;
    }
    if (!vehicle3d_snippets()) {
        std::fprintf(stderr, "vehicle snippets failed\n");
        return 1;
    }
    (void)&multiplayer;
    (void)&score_point;
    (void)&my_game;
    (void)&controls;
    ponko::game("gpu buffers", 1, 1).run_headless(0, gpu_particles);
    ponko::game("handles", 1, 1).run_headless(0, stream_out);
    ponko::game("asset leaks", 1, 1).run_headless(0, find_leaks);
    ponko::game("streaming", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        streaming_setup(ctx);
        draw_crab(ctx);
        ctx.finish_uploads();
        draw_crab(ctx);
        if (ctx.asset_state(ponko::residency_kind::texture, "crab.png") != ponko::asset_state::resident) std::abort();
    });
    world_text_snippet();
    localized_text_snippet();
    action_map_snippet();
    pressed_this_frame_snippet();
    ponko::game("forest guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        auto look = ctx.create_material(ponko::material_of(ponko::colors::white));
        auto trunk = ctx.create_mesh(ponko::mesh_data::cube(1), look);
        auto card = ctx.create_mesh(ponko::mesh_data::plane(1), look);
        const std::vector<ponko::lod_level> tree = {{0.2f, 0, {ponko::lod_renderer_of(trunk)}, false, {}},
                                                    {0.02f, 0, {ponko::lod_renderer_of(card)}, false, {}}};
        auto batch = forest(ctx, tree, 4.0f, {{1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1}});
        ctx.free_instance_batch(batch);
    });
    ponko::game("scene compute guide", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        light_meter(ctx);
        if (ctx.scene_compute_passes().size() != 1 || !ctx.compute_passes().empty()) throw std::runtime_error("no scene compute pass");
    });
    replace_builtins();
    {
        ponko::game agents("Agents", 1, 1);
        add_navigation(agents);
        if (agents.schedule_plan().find("step_navmesh3d") == std::string::npos) throw std::runtime_error("no navigation step");
        if (!beside_navigation(ponko::access()) || beside_navigation(ponko::access().write_service(ponko::service::time)))
            throw std::runtime_error("navigation declares the wrong access");
    }
    ponko::game("static instances",1,1).run_headless(0,[](ponko::ctx& ctx) {
        auto material=ctx.create_material(ponko::material_of(ponko::colors::white));
        persistent_instances(ctx,ctx.create_mesh(ponko::mesh_data::cube(1.0f),material));
    });
    book::run_all();
    std::puts("guide snippets ok");
}

// Compiled for the guide, called only by an application with Steam running.
[[maybe_unused]] static void steam_messages_setup(std::uint64_t lobby_id) {
    // ANCHOR: steam_messages
    auto net = lobby_id ? ponko::net_messages::steam_join(480, lobby_id)
                        : ponko::net_messages::steam_host(480, 3);
    net.poll(); // every frame; drain net.next_event() afterwards
    if (!lobby_id && net.steam_lobby_id()) {
        net.steam_set_metadata("version", "1");
        net.steam_invite(); // normally in response to the player's invite button
    }
    // ANCHOR_END: steam_messages
}
[[maybe_unused]] static void general_steam_lobby() {
    // ANCHOR: general_steam_lobby
    auto lobby = ponko::steam_lobby::create(480, ponko::steam_lobby_visibility::public_lobby, 4);
    lobby.poll(); // each frame until lobby.id() != 0
    if (lobby.id()) {
        lobby.set_metadata("version", "1");
        auto members = lobby.members(); // Steam ids
        auto version = lobby.metadata("version");
        (void)members; (void)version;
        lobby.invite();
        lobby.leave();
    }
    // ANCHOR_END: general_steam_lobby
}
