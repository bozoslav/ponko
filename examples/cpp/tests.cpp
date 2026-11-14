#include "ponko.hpp"
#include <algorithm>
#include <chrono>
#include <cmath>
#include <atomic>
#include <cstdlib>
#include <filesystem>
#include <future>
#include <fstream>
#include <iostream>
#include <optional>
#include <string>
#include <thread>
#include <type_traits>
#include <vector>
#define CHECK(...) do { if (!(__VA_ARGS__)) { std::cerr << "failed: " #__VA_ARGS__ << " (line " << __LINE__ << ")\n"; std::abort(); } } while (false)
struct Pos { float x, y; };
struct Vel { float x, y; };
struct Tag {};
namespace game { struct Health { float value; }; }
namespace other { struct Health { int value; }; }
PONKO_COUNT_CPP_ALLOCATIONS
// The wrapper throws the engine's message; an argument the engine refused says so
// (a panic would say "panicked").
template<class F> void throws_invalid(F f, const char* what) {
    bool caught = false;
    try { f(); } catch (const std::exception& e) {
        std::string message = e.what();
        caught = message.find(what) != std::string::npos && message.find("panic") == std::string::npos;
    }
    CHECK(caught);
}
template<class F> void throws(F f) { bool caught = false; try { f(); } catch (const std::exception&) { caught = true; } CHECK(caught); }
template<class F> void throws_kind(F f, PonkoErrorKind kind) { bool caught = false; try { f(); } catch (const ponko::error& e) { caught = e.kind() == kind; } CHECK(caught); }

// ponko.hpp checks the library's ABI version against the header's macro when the
// first game or world is made, and says so clearly when they differ.
static void abi_version_is_checked() {
    CHECK(ponko_abi_version() == PONKO_ABI_VERSION);
    ponko::detail::check_abi_version(PONKO_ABI_VERSION, ponko_abi_version());
    ponko::detail::check_abi_version();
    bool caught = false;
    try { ponko::detail::check_abi_version(PONKO_ABI_VERSION, PONKO_ABI_VERSION + 1); }
    catch (const ponko::error& e) {
        const std::string message = e.what();
        caught = e.kind() == PONKO_ERROR_UNSUPPORTED && message.find("ABI version " + std::to_string(PONKO_ABI_VERSION + 1)) != std::string::npos &&
                 message.find("expects version " + std::to_string(PONKO_ABI_VERSION)) != std::string::npos;
    }
    CHECK(caught);
    // The real constructors pass the same check.
    ponko::world w;
    ponko::game g("abi", 64, 64);
}

static void worlds() {
    static_assert(!std::is_copy_constructible<ponko::world>::value, "world owns its handle");
    ponko::world a;
    auto e = a.spawn(Pos{1, 2}, Vel{3, 4});
    CHECK(a.size() == 1 && a.alive(e));
    a.each([&](ponko::entity id, Pos& p, const Vel& v) { p.x += v.x; CHECK(id == e); });
    CHECK(a.get<Pos>(e).x == 4);
    a.query<Pos, Vel>([&](auto chunk) {
        CHECK(chunk.size() == 1);
        auto [ps, vs] = chunk.columns();
        ps[0].y += vs[0].y;
        CHECK(chunk.entity(0) == e);
        throws([&] { (void)chunk.entity(1); });
    });
    CHECK(a.get<Pos>(e).y == 6);
    a.set(e, Pos{9, 8}); CHECK(a.get<Pos>(e).y == 8);
    // try_get, has, add and remove.
    CHECK(!a.has<Tag>(e) && !a.try_get<Tag>(e));
    a.set(e, Tag{}); CHECK(a.has<Tag>(e));
    CHECK(a.count<Pos>() == 1 && a.count<Pos, Tag>() == 1 && a.count<Vel, Tag>() == 1);
    a.remove<Tag>(e); CHECK(!a.has<Tag>(e) && a.count<Pos, Tag>() == 0);
    throws([&] { (void)a.get<Tag>(e); });
    // A function object works as well as a lambda.
    struct Move { void operator()(Pos& p, const Vel& v) const { p.x += v.x; } };
    a.each(Move{}); CHECK(a.get<Pos>(e).x == 12);
    // Exceptions from callbacks come out unchanged; world calls inside a loop throw.
    bool original_exception = false;
    try { a.each([](Pos&) { throw std::runtime_error("callback"); }); }
    catch (const std::runtime_error& error) { original_exception = std::string(error.what()) == "callback"; }
    CHECK(original_exception);
    throws([&] { a.each([&](Pos&) { (void)a.size(); }); });
    throws([&] { a.query<Pos, Pos>([](auto) {}); });
    // Moving worlds keeps entities and component ids.
    ponko::world c(std::move(a)); CHECK(c.get<Pos>(e).x == 12);
    throws([&] { (void)a.size(); });
    ponko::world b;
    b = std::move(c); CHECK(b.get<Pos>(e).x == 12);
    b.remove<Vel>(e); throws([&] { (void)b.get<Vel>(e); });
    b.despawn(e); CHECK(!b.alive(e)); throws([&] { b.despawn(e); });
    CHECK(!b.try_get<Pos>(e));
}

static void names_and_reflection() {
    // Components are named after their C++ type, namespaces included.
    CHECK(ponko::detail::type_name<game::Health>() == "game::Health");
    CHECK(ponko::detail::type_name<Pos>() == "Pos");
    ponko::world w;
    w.reflect<game::Health>({ponko::field("value", &game::Health::value)});
    auto e = w.spawn(game::Health{5}, other::Health{7});
    CHECK(w.get<game::Health>(e).value == 5 && w.get<other::Health>(e).value == 7);
    const std::string saved = w.save();
    CHECK(saved.find("\"game::Health\":{\"value\":5.0}") != std::string::npos);
    w.set(e, game::Health{1});
    w.load(saved);
    CHECK(w.get<game::Health>(e).value == 5);
    // A chosen name must come before first use, and can't be taken twice.
    ponko::world n;
    n.reflect<Vel>("Velocity", {ponko::field("x", &Vel::x), ponko::field("y", &Vel::y)});
    n.spawn(Vel{1, 2});
    CHECK(n.save().find("\"Velocity\":{\"x\":1.0,\"y\":2.0}") != std::string::npos);
    throws([&] { n.reflect<Pos>("Velocity", {ponko::field("x", &Pos::x)}); });
    n.spawn(Pos{});
    throws([&] { n.reflect<Pos>("Position", {ponko::field("x", &Pos::x)}); });
}

namespace game { struct Score { std::uint32_t red, blue; }; }
static void resources() {
    ponko::world w;
    CHECK(!w.try_resource<game::Score>());
    throws([&] { w.resource<game::Score>(); });
    w.set_resource(game::Score{1, 2});
    CHECK(w.resource<game::Score>().red == 1 && w.resource<game::Score>().blue == 2);
    w.set_resource(game::Score{3, 2});
    CHECK(w.resource<game::Score>().red == 3);
    // Described, it's saved with the entities and loaded back.
    w.reflect_resource<game::Score>({ponko::field("red", &game::Score::red), ponko::field("blue", &game::Score::blue)});
    const std::string saved = w.save();
    CHECK(saved.find("\"game::Score\":{\"blue\":2,\"red\":3}") != std::string::npos);
    w.set_resource(game::Score{0, 0});
    w.load(saved);
    CHECK(w.resource<game::Score>().red == 3);
    CHECK(w.remove_resource<game::Score>());
    CHECK(!w.remove_resource<game::Score>());
    CHECK(!w.try_resource<game::Score>());
}

namespace game { struct Tuning { float speed; std::uint32_t waves; }; }
static void data_files() {
    const auto file = std::filesystem::temp_directory_path() / "ponko-cpp-tuning.json";
    std::ofstream(file) << R"({"speed": 2.5, "waves": 3})";
    const std::string path = file.string();
    auto w = ponko::game("data", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto world = ctx.world();
        world.reflect_resource<game::Tuning>({ponko::field("speed", &game::Tuning::speed), ponko::field("waves", &game::Tuning::waves)});
        ctx.load_json<game::Tuning>(path.c_str());
        world.reflect<game::Tuning>({ponko::field("speed", &game::Tuning::speed), ponko::field("waves", &game::Tuning::waves)});
        const ponko::entity e = world.spawn(game::Tuning{0, 9});
        ctx.load_json<game::Tuning>(path.c_str(), e);
        CHECK(world.get<game::Tuning>(e).speed == 2.5f);
    });
    CHECK(w.resource<game::Tuning>().speed == 2.5f && w.resource<game::Tuning>().waves == 3);
    std::filesystem::remove(file);
}

namespace game { struct Armor { float rating; }; }
// A schema described from C++ is what the capabilities answer reports.
static void described_schemas_reach_capabilities() {
    ponko::game("described", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto world = ctx.world();
        world.reflect<game::Armor>("DescribedInCpp", {ponko::field("rating", &game::Armor::rating)});
        const auto before = ctx.capabilities_json();
        CHECK(before.find("\"DescribedInCpp\"") != std::string::npos);
        CHECK(before.find("\"MadeInCpp\"") == std::string::npos);
        world.describe_component("DescribedInCpp", R"({"Object":[["rating","Number"],["kind",{"Enum":["MadeInCpp"]}]]})");
        CHECK(ctx.capabilities_json().find("\"MadeInCpp\"") != std::string::npos);
        throws([&] { world.describe_component("NotReflected", R"("Number")"); });
    });
}

// Saved under the executable's assets/ copy (build output), so nothing to clean.
static void scenes_and_prefabs() {
    ponko::entity first = 0;
    std::vector<ponko::entity> palms;
    auto w = ponko::game("scenes", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto world = ctx.world();
        world.reflect<game::Health>({ponko::field("value", &game::Health::value)});
        // Sprites are saved by their image's name, and spawning loads it.
        auto model = world.spawn(ponko::transform_at({70, 80}), game::Health{9}, ponko::sprite_of(ctx.load_texture("crab.png"), ponko::colors::white, 4));
        ctx.save_prefab(model, "test-cpp-prefab.json");
        world.despawn(model);
        for (float x : {0.0f, 10.0f}) palms.push_back(ctx.spawn_prefab("test-cpp-prefab.json", {x, 100}));
        throws([&] { ctx.spawn_prefab("test-cpp-missing.json", {0, 0}); });
        throws([&] { ctx.load_scene("test-cpp-missing.json"); });
        throws([&] { ctx.save_prefab(model, "test-cpp-dead.json"); });
        first = palms[0];
        ctx.save_scene("test-cpp-scene.json");
    });
    CHECK(w.size() == 2);
    CHECK(w.get<ponko::transform2d>(palms[1]).position.x == 10 && w.get<game::Health>(palms[1]).value == 9);
    CHECK(w.get<ponko::sprite>(palms[1]).z == 4);
    auto loaded = ponko::game("scenes", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        ctx.world().reflect<game::Health>({ponko::field("value", &game::Health::value)});
        ctx.world().spawn(Tag{});   // replaced by the scene
        ctx.load_scene("test-cpp-scene.json");
    });
    CHECK(loaded.size() == 2 && loaded.get<game::Health>(first).value == 9);
    // A prefab with children: the root comes back, its parts are listed,
    // and despawning the root takes them too.
    ponko::game("scenes", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto cart = ctx.spawn_prefab("prefabs/cart.json", {50, 60});
        auto parts = ctx.prefab_parts(cart);
        CHECK(parts.size() == 4 && ctx.world().size() == 5);
        CHECK(ctx.world().get<ponko::transform2d>(cart).position.x == 50);
        for (auto part : parts) CHECK(ctx.prefab_parts(part).empty());   // parts aren't roots
        CHECK(ctx.despawn_prefab(cart) == 5 && ctx.world().size() == 0);
    });
    // Offline, a lockstep load is a plain load; a failed one changes nothing.
    auto offline = ponko::game("scenes", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        ctx.world().reflect<game::Health>({ponko::field("value", &game::Health::value)});
        ctx.world().spawn(Tag{});
        ctx.load_scene_lockstep("test-cpp-scene.json");
        throws([&] { ctx.load_scene_lockstep("test-cpp-missing.json"); });
    });
    CHECK(offline.size() == 2 && offline.get<game::Health>(first).value == 9);
}

static void three_d() {
    auto probes = ponko::game("probes", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::reflection_probe3d_default());
        ctx.bake_reflection_probe(e);
        auto p = ctx.world().get<ponko::reflection_probe3d>(e);
        CHECK(p.content == 1 && p.box_projection);
        ctx.bake_reflection_probe(e);
        CHECK(ctx.world().get<ponko::reflection_probe3d>(e).content == 1);
    });
    CHECK(probes.size() == 1);
    auto decals = ponko::game("decals", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        std::vector<std::uint8_t> pixels(2 * 2 * 4, 255);
        auto colors = ctx.create_texture_array(2, 2, pixels, ponko::texture_kind::color);
        throws([&] { ctx.set_decal_textures(colors); }); // decals want TextureKind::Data arrays
        auto images = ctx.create_texture_array(2, 2, pixels, ponko::texture_kind::data);
        ctx.set_decal_textures(images);
        CHECK(!ctx.supports_decals()); // no GPU in a headless run
        auto mark = ponko::decal3d_default();
        mark.size = {2, 2, 1};
        mark.order = 3;
        mark.normal_image = ponko::decal_no_image;
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 1, 0}), mark, ponko::receives_decals{0});
        CHECK(ctx.world().get<ponko::decal3d>(e).order == 3 && ctx.world().get<ponko::decal3d>(e).size.x == 2);
        CHECK(ctx.world().get<ponko::receives_decals>(e).layers == 0);
        ctx.clear_decal_textures();
    });
    CHECK(decals.size() == 1);
    ponko::world lights;
    auto lamp = lights.spawn(ponko::transform3d_at({1, 2, 3}), ponko::point_light3d{ponko::colors::red, 20, 12, false});
    auto beam = lights.spawn(ponko::transform3d_at({0, 3, 0}), ponko::spot_light3d{ponko::colors::white, 40, 25, .2f, .5f, true, 256});
    CHECK(lights.get<ponko::point_light3d>(lamp).range == 12);
    CHECK(lights.get<ponko::spot_light3d>(beam).inner_angle == .2f);
    CHECK(lights.get<ponko::spot_light3d>(beam).shadows);
    CHECK(lights.get<ponko::spot_light3d>(beam).shadow_resolution == 256);
    CHECK(lights.get<ponko::point_light3d>(lamp).shadow_resolution == 0);
    lights.each([](ponko::point_light3d& p) { p.intensity += 1; });
    CHECK(lights.get<ponko::point_light3d>(lamp).intensity == 21);
    std::vector<ponko::entity> cubes;
    ponko::mesh cube{};
    auto spin = [](ponko::ctx& ctx) {
        ctx.world().each([](ponko::transform3d& t, const ponko::model&) { t.rotation = ponko::quat_from_rotation_y(0.1f) * t.rotation; });
    };
    auto w = ponko::game("3d", 640, 480).sim_system(spin, "spin").run_headless(5, [&](ponko::ctx& ctx) {
        auto stone = ctx.create_material(ponko::material_of(ponko::rgb(.5f, .5f, .5f)));
        cube = ctx.create_mesh(ponko::mesh_data::cube(1), stone);
        for (int i = 0; i < 3; ++i)
            cubes.push_back(ctx.world().spawn(ponko::transform3d_at({float(i) * 2, 0, 0}), ponko::model_of(cube, ponko::colors::red)));
        throws([&] { ctx.create_mesh(ponko::mesh_data::cube(1), ponko::material{99}); });
        auto shiny = ponko::material_of(ponko::colors::white);
        shiny.metallic = 2;
        throws([&] { ctx.create_material(shiny); });
        throws([] { ponko::mesh_data({{0, 0, 0}, {1, 0, 0}, {0, 1, 0}}, {0, 1}); });
        auto tile = ctx.create_terrain_tile(std::vector<float>(81, 0.0f), 16, stone);
        CHECK(tile.lod_distance == 16 && tile.lods[0].id != tile.lods[3].id);
        ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), tile);
        throws([&] { ctx.create_terrain_tile(std::vector<float>(80, 0.0f), 16, stone); });
        throws([] { ponko::mesh_data::heightfield({0, 0, 0}, 1); });
        auto layers = ctx.create_texture_array(1, 1, std::vector<std::uint8_t>(8, 200), ponko::texture_kind::color);
        CHECK(layers.layers == 2);
        auto weights = ctx.create_texture_array(1, 1, std::vector<std::uint8_t>(4, 255), ponko::texture_kind::data);
        auto ground = ctx.create_terrain_material(ponko::terrain_material_of(layers, weights, {0, 0}, {16, 16}));
        CHECK(ground.id != stone.id);
        throws([&] { ctx.create_texture_array(1, 1, std::vector<std::uint8_t>(6, 0), ponko::texture_kind::color); });
        throws([&] { ctx.create_terrain_material(ponko::terrain_material_of(layers, weights, {0, 0}, {0, 0})); });
        auto grey = ctx.create_effect("grey", "fn effect(p: Pixel) -> vec3<f32> { return vec3<f32>(p.color.g); }");
        ctx.set_effects({ponko::screen_effect_of(grey), ponko::screen_effect_of(grey, stone)});
        CHECK(ctx.effects().size() == 2 && ctx.effects()[1].material.id == stone.id);
        throws([&] { ctx.create_effect("bad", "fn effect(p: Pixel) -> f32 { return 1.0; }"); });
        {
            auto shaders = ctx.shader_status();
            CHECK(shaders.size() >= 2);
            auto& grey_status = shaders[shaders.size() - 2];
            CHECK(grey_status.shader == "grey" && grey_status.ok() && grey_status.errors.empty());
            CHECK(grey_status.info.kind == PONKO_SHADER_EFFECT && grey_status.info.outcome == PONKO_SHADER_COMPILED);
            auto& bad = shaders.back();
            CHECK(bad.shader == "bad" && !bad.ok() && bad.info.outcome == PONKO_SHADER_FAILED);
            CHECK(bad.info.diagnostics == 1 && bad.info.line > 0 && bad.errors.rfind("bad", 0) == 0);
            CHECK(ctx.shader_status_json().find("\"outcome\":\"failed\"") != std::string::npos);
        }
        throws([&] { ctx.set_effects({ponko::screen_effect_of(ponko::effect{99})}); });
        ctx.set_effects({});
        CHECK(ctx.effects().empty());
        auto mirror = ctx.create_render_target(32, 16);
        CHECK(mirror.width == 32 && mirror.height == 16);
        auto eye = ctx.camera3d();
        eye.position = {0, 5, 0};
        auto below = ctx.reflection_camera(eye, mirror, 1);
        CHECK(below.camera.position.y == -3 && below.has_clip_plane && below.clip_plane[3] == -1);
        ctx.set_target_cameras({below, ponko::target_camera_of(eye, mirror)});
        CHECK(ctx.target_cameras().size() == 2 && !ctx.target_cameras()[1].has_clip_plane);
        const std::uint8_t px[4] = {255, 255, 255, 255};
        auto plain = ctx.create_texture(1, 1, px, sizeof px);
        throws([&] { ctx.set_target_cameras({ponko::target_camera_of(eye, plain)}); });
        throws([&] { ctx.create_render_target(0, 4); });
        ctx.set_target_cameras({});
        auto batch_material = ctx.create_material(ponko::material_of(ponko::colors::white));
        auto batch_mesh = ctx.create_mesh(ponko::mesh_data::cube(1.0f), batch_material);
        std::vector<std::array<float,16>> batch_matrices = {{{1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1}}};
        auto batch = ctx.create_instance_batch(batch_mesh,batch_matrices);
        ctx.update_instance_batch(batch,batch_matrices);
        for (std::uint32_t mode=0; mode<3; ++mode) ctx.set_instance_batch_culling(batch,mode);
        throws([&] {ctx.set_instance_batch_culling(batch,3);});
        ctx.free_instance_batch(batch);
        throws([&] {ctx.set_instance_batch_culling(batch,0);});
        throws([&] {ctx.update_instance_batch(batch,batch_matrices);});
        auto buffer = ctx.create_gpu_buffer(65);
        ctx.write_gpu_buffer(buffer, 64, {{1, 2, 3, 4}});
        throws([&] { ctx.write_gpu_buffer(buffer, 65, {{1, 2, 3, 4}}); });
        auto spare = ctx.create_gpu_buffer(2);
        ctx.free_gpu_buffer(spare);
        throws([&] { ctx.free_gpu_buffer(spare); });
        throws([&] { ctx.write_gpu_buffer(spare, 0, {{1, 2, 3, 4}}); });
        ctx.set_gpu_buffer_motion(buffer, true);
        throws([&] { ctx.set_gpu_buffer_motion(spare, true); });
        auto reused = ctx.create_gpu_buffer(3);  // the freed slot comes back under a new id
        CHECK((reused.id & 0x000FFFFFu) == (spare.id & 0x000FFFFFu) && reused.id != spare.id);  // same 20-bit slot
        throws([&] { ctx.write_gpu_buffer(spare, 0, {{1, 2, 3, 4}}); });
        auto update = ctx.create_compute("update", "fn update(i: u32) { items[i].x += frame.time.y; }");
        ctx.set_compute_passes({ponko::buffer_compute_pass(update, buffer)});
        CHECK(ctx.compute_passes()[0].has_buffer && ctx.compute_passes()[0].buffer.id == buffer.id);
        auto fade = ctx.create_compute("fade", "fn compute(c: Cell) -> vec4<f32> { return c.value * 0.9; }");
        ctx.set_compute_passes({ponko::compute_pass_of(fade, mirror), ponko::compute_pass_of(fade, mirror, stone)});
        CHECK(ctx.compute_passes().size() == 2 && ctx.compute_passes()[1].material.id == stone.id);
        throws([&] { ctx.create_compute("bad", "fn compute(c: Cell) -> f32 { return 1.0; }"); });
        throws([&] { ctx.set_compute_passes({ponko::compute_pass_of(fade, plain)}); });
        throws([&] { ctx.set_compute_passes({ponko::compute_pass_of(ponko::compute{9}, mirror)}); });
        ctx.set_compute_passes({});

        auto cam = ctx.camera3d();
        cam.position = {0, 5, 10};
        ctx.set_camera3d(cam);
        ctx.camera3d_look_at({0, 0, 0});
        CHECK(!ctx.occlusion_culling());
        ctx.set_occlusion_culling(true);
        CHECK(ctx.occlusion_culling());
        ctx.set_occlusion_culling(false);
        CHECK(ponko::forward(ponko::looking_at(ponko::transform3d_at(cam.position), {0, 0, 0})).z < 0);
        cam.near = 0;
        throws([&] { ctx.set_camera3d(cam); });
        auto light = ctx.lighting();
        CHECK(light.shadows && light.shadow_cascades == 4);
        light.sun_direction = {-1, -1, 0};
        light.shadow_cascades = 2;
        CHECK(!light.has_equator);
        light.has_equator = true;
        light.equator_color = ponko::rgb(0, 1, 0);
        ctx.set_lighting(light);
        CHECK(ctx.lighting().shadow_cascades == 2);
        CHECK(ctx.lighting().has_equator && ctx.lighting().equator_color.g == 1);
        light.has_equator = false;
        ctx.set_lighting(light);
        CHECK(!ctx.lighting().has_equator);
        {
            auto bounced = light;
            CHECK(!bounced.gi_enabled && bounced.gi_extent.x == 32);
            bounced.gi_enabled = true;
            bounced.gi_spacing = 0;
            throws([&] { ctx.set_lighting(bounced); });
            bounced.gi_spacing = 1.5f;
            ctx.set_lighting(bounced);
            CHECK(ctx.lighting().gi_enabled && ctx.lighting().gi_spacing == 1.5f);
            ctx.set_lighting(light);
        }
        light.shadow_cascades = 0;
        throws([&] { ctx.set_lighting(light); });
        auto sky = ctx.sky();
        CHECK(sky.atmosphere && sky.fog_density == 0);
        sky.fog_density = 0.01f;
        ctx.set_sky(sky);
        CHECK(ctx.sky().fog_density == 0.01f);
        CHECK(sky.fog_end == 0 && sky.fog_sun_focus == 0.7f);
        sky.fog_start = 500;
        sky.fog_end = 1200;
        ctx.set_sky(sky);
        CHECK(ctx.sky().fog_end == 1200);
        sky.fog_sun_focus = 1;
        throws([&] { ctx.set_sky(sky); });
        sky.fog_sun_focus = 0.7f;
        sky.light_shafts = 2;
        throws([&] { ctx.set_sky(sky); });
        sky.light_shafts = 1;
        ctx.set_sky(sky);
        CHECK(ctx.sky().light_shafts == 1);
        sky.light_shafts = 0;
        sky.sun_size = 0;
        throws([&] { ctx.set_sky(sky); });
        CHECK(ponko::sun_direction_at(12).y < -0.8f);
        auto post = ctx.post();
        CHECK(post.tonemap == uint32_t(ponko::tonemap::aces) && post.contrast == 1 && !post.has_lut);
        post.tonemap = uint32_t(ponko::tonemap::neutral);
        post.bloom = 0.2f;
        CHECK(post.anti_aliasing == 0 && !post.ao_enabled && post.ao_radius == 1);
        post.anti_aliasing = uint32_t(ponko::anti_aliasing::smaa);
        post.ao_enabled = true; post.ao_half_resolution = true; post.ao_temporal = true; post.ao_radius = 2; post.ao_intensity = 1.5f;
        post.ao_slices = 4; post.ao_steps = 8;
        ctx.set_post(post);
        CHECK(ctx.post().tonemap == uint32_t(ponko::tonemap::neutral));
        CHECK(ctx.post().ao_half_resolution && ctx.post().ao_temporal && ctx.post().ao_enabled && ctx.post().ao_radius == 2 && ctx.post().ao_steps == 8);
        CHECK(ctx.post().anti_aliasing == uint32_t(ponko::anti_aliasing::smaa));
        auto invalid_post = post; invalid_post.anti_aliasing = 4;
        throws([&] { ctx.set_post(invalid_post); });
        invalid_post = post; invalid_post.taa_sharpness = 2;
        throws([&] { ctx.set_post(invalid_post); });
        invalid_post = post; invalid_post.contact_shadows = -1;
        throws([&] { ctx.set_post(invalid_post); });
        invalid_post = post; invalid_post.ssr = 1.5f;
        throws([&] { ctx.set_post(invalid_post); });
        invalid_post = post; invalid_post.ssr_max_distance = -1;
        throws([&] { ctx.set_post(invalid_post); });
        {
            auto reflective = post;
            reflective.ssr = 0.7f;
            reflective.ssr_max_distance = 15;
            ctx.set_post(reflective);
            CHECK(ctx.post().ssr == 0.7f && ctx.post().ssr_max_distance == 15);
            ctx.set_post(post);
        }
        {
            auto temporal = post;
            temporal.anti_aliasing = uint32_t(ponko::anti_aliasing::taa);
            temporal.taa_sharpness = 0.5f;
            temporal.ao_enabled = true;
            temporal.ao_temporal = true;
            ctx.set_post(temporal);
            ctx.reset_temporal_history();
            auto mover = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
            ctx.reset_motion(mover);
            ctx.world().despawn(mover);
            throws_invalid([&] { ctx.reset_motion(mover); }, "no such entity");
            CHECK(ctx.post().anti_aliasing == 3 && ctx.post().taa_sharpness == 0.5f);
            ctx.set_post(post);
        }
        invalid_post = post; invalid_post.ao_radius = 0;
        throws([&] { ctx.set_post(invalid_post); });
        post.saturation = -1;
        throws([&] { ctx.set_post(post); });

        // Graphics quality presets, switched at run time.
        CHECK(ctx.post().render_scale == 1 && ctx.post().sharpen == 0);
        auto quality = ponko::graphics_quality_preset(ponko::quality_level::medium);
        CHECK(ponko::graphics_quality_level(quality) == ponko::quality_level::medium);
        CHECK(quality.shadow_cascades == 3 && quality.anti_aliasing == uint32_t(ponko::anti_aliasing::fxaa));
        ctx.set_graphics_quality(quality);
        CHECK(ctx.lighting().shadow_cascades == 3 && ctx.lighting().shadow_resolution == 2048);
        CHECK(ctx.graphics_quality().light_shadow_views == 4);
        CHECK(ctx.lighting().light_shadow_atlas == 2048);
        quality = ponko::graphics_quality_preset(ponko::quality_level::low);
        ctx.set_graphics_quality(quality);
        CHECK(ctx.post().render_scale < 1 && ctx.lighting().light_shadow_views == 0);
        quality.shadow_cascades = 3;
        CHECK(ponko::graphics_quality_level(quality) == ponko::quality_level::custom);
        auto bad_quality = quality; bad_quality.render_scale = 4;
        throws([&] { ctx.set_graphics_quality(bad_quality); });
        throws([&] { ponko::graphics_quality_preset(ponko::quality_level::custom); });
        CHECK(ctx.graphics_quality().shadow_cascades == 2);
        // GPU tiers: no GPU headless; tier reports and presets need none.
        CHECK(!ctx.gpu_capabilities() && !ctx.gpu_tier() && !ctx.recommended_graphics_quality());
        CHECK(!ctx.gpu_has(ponko::gpu_feature::compute));
        CHECK(ponko::gpu_tier_report(ponko::gpu_tier::desktop).find("\"tier\":\"desktop\"") != std::string::npos);
        CHECK(!ponko::gpu_tier_has(ponko::gpu_tier::webgl2, ponko::gpu_feature::screen_space_reflections));
        CHECK(ponko::gpu_tier_has(ponko::gpu_tier::baseline, ponko::gpu_feature::compute_culling));
        CHECK(!ponko::gpu_tier_has(ponko::gpu_tier::baseline, ponko::gpu_feature::gpu_skinning));
        CHECK(ponko::graphics_quality_level(ponko::graphics_quality_for_tier(ponko::gpu_tier::baseline)) == ponko::quality_level::medium);
        CHECK(ponko::graphics_quality_for_tier(ponko::gpu_tier::webgl2).anti_aliasing == uint32_t(ponko::anti_aliasing::fxaa));
        throws([&] { ponko::gpu_tier_has(ponko::gpu_tier(7), ponko::gpu_feature::compute); });
        ctx.set_graphics_quality(ponko::graphics_quality_preset(ponko::quality_level::high));
        CHECK(ctx.graphics_quality().ssr == 1 && ctx.post().ssr == 1);
        CHECK(!ctx.graphics_quality().gi && ponko::graphics_quality_preset(ponko::quality_level::ultra).gi);
        auto light_q = ctx.lighting();
        light_q.shadow_resolution = 100;
        throws([&] { ctx.set_lighting(light_q); });

        auto tiles = ctx.create_shader("tiles", R"(
            fn surface(s: Surface) -> Surface {
                var out = s;
                out.albedo *= step(0.5, fract(s.uv.x * 8.0));
                return out;
            })");
        auto look = ponko::material_of(ponko::rgb(1, 1, 1));
        look.has_shader = true;
        look.shader = tiles;
        look.blend = uint32_t(ponko::blend::transparent);
        auto tiled = ctx.create_material(look);
        look.double_sided = true;
        ctx.update_material(tiled, look);
        throws([&] { ctx.create_shader("broken", "fn surface(s: Surface) -> Surface { return 1; }"); });
        // The exception says what kind of failure it was, and is still a runtime_error.
        try {
            ctx.create_shader("broken", "fn surface(s: Surface) -> Surface { return 1; }");
            CHECK(false);
        } catch (const ponko::error& e) {
            CHECK(e.kind() == PONKO_ERROR_SHADER);
            CHECK(std::string(e.what()).rfind("broken:1:", 0) == 0);
        }
        try {
            ctx.load_shader("no-such-shader.wgsl");
            CHECK(false);
        } catch (const std::runtime_error& e) {
            CHECK(ponko_last_error_kind() == PONKO_ERROR_NOT_FOUND);
            CHECK(dynamic_cast<const ponko::error*>(&e) != nullptr);
        }
    });
    const auto t = w.get<ponko::transform3d>(cubes[1]);
    CHECK(t.position.x == 2 && std::abs(t.rotation.y - std::sin(0.25f)) < 1e-5f);
    CHECK(w.get<ponko::model>(cubes[2]).mesh.id == cube.id);
    // Quaternion math matches the engine's.
    const auto q = ponko::quat_from_rotation_y(1.5707964f);
    const auto v = q * ponko::vec3{1, 0, 0};
    CHECK(std::abs(v.z + 1) < 1e-5f);
    auto tri = ponko::mesh_data({{0, 0, 0}, {1, 0, 0}, {0, 1, 0}}, {0, 1, 2});
    CHECK(tri.view().vertex_count == 3 && tri.view().normals == nullptr);
    tri.set_colors({{1, 0, 0, 1}, {0, 1, 0, 1}, {0, 0, 1, 1}});
    throws([&] { tri.set_colors({{1, 1, 1, 1}}); });
}

static void parallel_queries() {
    ponko::world w;
    for (int i = 0; i < 100000; ++i) w.spawn(i);
    std::atomic<long long> rows{0};
    w.query_par<int>([&](auto chunk) {
        auto [col] = chunk.columns();
        for (std::size_t i = 0; i < chunk.size(); ++i) col[i] *= 2;
        rows += (long long)chunk.size();
    });
    CHECK(rows == 100000);
    w.each_par([](int& n) { n += 1; });
    long long sum = 0;
    w.each([&](int n) { sum += n; });
    CHECK(sum == 100000LL * 99999LL + 100000);
    throws([&] { w.each_par([](int&) { throw std::runtime_error("par"); }); });

    // Filling a buffer from all cores: rows are numbered the same way in both queries.
    for (int i = 0; i < 5000; ++i) w.spawn(-1, 'x');
    CHECK(w.count<int>() == 105000 && w.count<int, char>() == 5000);
    std::vector<int> seq(w.count<int>()), par(w.count<int>());
    w.query<int>([&](auto chunk) { auto [n] = chunk.columns(); for (std::size_t i = 0; i < chunk.size(); ++i) seq[chunk.first() + i] = n[i]; });
    w.query_par<int>([&](auto chunk) { auto [n] = chunk.columns(); for (std::size_t i = 0; i < chunk.size(); ++i) par[chunk.first() + i] = n[i]; });
    CHECK(seq == par);
    CHECK(std::count(seq.begin(), seq.end(), -1) == 5000);
}

struct Fuel { int value; };
struct Heat { int value; };

static void change_detection() {
    using list = std::vector<ponko::entity>;
    ponko::world w;
    w.track_changes<Fuel>();
    const auto a = w.spawn(Fuel{1}, Heat{1}), b = w.spawn(Fuel{2}, Heat{2}), c = w.spawn(Fuel{3});
    auto changed = [&] { list out; w.each<ponko::changed<Fuel>>([&](ponko::entity e, const Fuel&) { out.push_back(e); }); return out; };
    auto added = [&] { list out; w.each<ponko::added<Fuel>>([&](ponko::entity e, const Fuel&) { out.push_back(e); }); return out; };
    // Outside a game the reader keeps its own place: look, then move on.
    auto next = [&] { w.set_changes_since(w.advance_change_tick()); };
    CHECK(w.change_tick() == 1 && w.changes_since() == 0);
    CHECK(changed() == list{a, b, c} && added() == list{a, b, c});   // nothing seen yet: everything
    next();
    CHECK(w.change_tick() == 2 && w.changes_since() == 1);
    CHECK(changed().empty() && added().empty() && w.count<ponko::changed<Fuel>>() == 0);

    // Reads change nothing: const&, copies, const columns, get, count.
    w.each([](const Fuel&) {}); w.each([](Fuel) {}); w.each_par([](const Fuel&, const Heat&) {});
    w.query<const Fuel>([](auto chunk) {
        auto [fuel] = chunk.columns();
        static_assert(std::is_same<decltype(fuel), const Fuel*>::value, "const columns are read-only");
    });
    (void)w.get<Fuel>(a); CHECK(w.count<Fuel>() == 3);
    CHECK(changed().empty());

    // Writes do: set, a `&` parameter (every row visited), a column, mark_changed.
    w.set(b, Fuel{20}); CHECK(changed() == list{b} && added().empty());
    next();
    w.each([](Fuel&, const Heat&) {}); CHECK(changed() == list{a, b});
    next();
    w.query<Fuel>([](auto) {}); CHECK(changed() == list{a, b, c});
    next();
    w.query_par<const Heat, Fuel>([](auto) {}); CHECK(changed() == list{a, b});
    next();
    CHECK(w.mark_changed<Fuel>(c) && !w.mark_changed<Heat>(c)); CHECK(changed() == list{c});
    CHECK(w.count<ponko::changed<Fuel>>() == 1 && w.count<Heat, ponko::changed<Fuel>>() == 0);

    // A filtered loop that writes marks only the rows it visits.
    next();
    w.set(a, Fuel{5});
    w.each<ponko::changed<Fuel>>([](Fuel& f) { f.value += 1; });
    CHECK(w.get<Fuel>(a).value == 6 && w.get<Fuel>(b).value == 20 && changed() == list{a});
    // Filtered columns: runs of rows, numbered by first().
    w.mark_changed<Fuel>(c);
    std::size_t rows = 0;
    w.query<Fuel, ponko::changed<Fuel>>([&](auto chunk) { CHECK(chunk.first() == rows); rows += chunk.size(); });
    CHECK(rows == 2);

    // Added: a new component on an old entity; a filter needn't be a column.
    next();
    w.remove<Fuel>(c); w.set(c, Fuel{7}); w.set(a, Fuel{8});
    CHECK(added() == list{c} && w.count<ponko::changed<Fuel>>() == 2);
    list heated; w.each<ponko::added<Fuel>>([&](ponko::entity e, const Heat&) { heated.push_back(e); });
    CHECK(heated.empty());
    w.set(c, Heat{3});
    w.each<ponko::added<Fuel>>([&](ponko::entity e, const Heat&) { heated.push_back(e); });
    CHECK(heated == list{c});
    // Several filters must all pass; any_of needs one.
    CHECK(w.count<ponko::changed<Fuel>, ponko::added<Fuel>>() == 1);
    CHECK((w.count<ponko::any_of<ponko::changed<Fuel>, ponko::added<Fuel>>>() == 2));
    // A filter starts tracking: Heat is new to it, so all three count.
    CHECK(w.count<ponko::changed<Heat>>() == 3);

    // Removals: a removed component or a despawned entity, oldest first.
    CHECK(w.removed<Fuel>() == list{c});   // from the remove above
    next();
    CHECK(w.removed<Fuel>().empty());
    w.remove<Fuel>(a); w.despawn(b);
    CHECK(w.removed<Fuel>() == list{a, b} && w.removed<Heat>() == list{b});
    const auto seen = w.advance_change_tick();
    CHECK(w.removed_since<Fuel>(seen).empty() && w.removed_since<Fuel>(0) == list{c, a, b});
    throws([&] { (void)w.removed<Pos>(); });   // not tracked
    throws([&] { ponko::world moved(std::move(w)); (void)w.change_tick(); });

    // On all cores: each passing row once.
    ponko::world big;
    big.track_changes<int>();
    std::vector<ponko::entity> es;
    for (int i = 0; i < 50000; ++i) es.push_back(i % 2 ? big.spawn(i) : big.spawn(i, 'x'));
    big.set_changes_since(big.advance_change_tick());
    for (std::size_t i = 0; i < es.size(); i += 3) big.mark_changed<int>(es[i]);
    std::atomic<long long> sum{0}, visits{0};
    big.each_par<ponko::changed<int>>([&](const int& n) { sum += n; ++visits; });
    long long want = 0, n = 0;
    for (int i = 0; i < 50000; i += 3) { want += i; ++n; }
    CHECK(sum == want && visits == n && big.count<ponko::changed<int>>() == std::size_t(n));

    // In a game every system has its own place: each change is seen once,
    // by a reader on either side of the writer, also through commands.
    for (bool reader_first : {true, false}) {
        std::vector<std::size_t> seen_changes, seen_removed, own;
        ponko::entity target = 0;
        auto reader = [&](ponko::ctx& ctx) {
            seen_changes.push_back(ctx.world().count<ponko::changed<Fuel>>());
            seen_removed.push_back(ctx.world().removed<Fuel>().size());
        };
        auto writer = [&](ponko::ctx& ctx) {
            own.push_back(ctx.world().count<ponko::changed<Fuel>>());   // its own writes aren't news to it
            if (ctx.sim_tick() == 1) ctx.world().set(target, Fuel{9});
            if (ctx.sim_tick() == 3) ctx.set_later(target, Fuel{10});
            if (ctx.sim_tick() == 5) ctx.despawn_later(target);
        };
        ponko::game g("t", 1, 1);
        if (reader_first) g.sim_system(reader, "reader").sim_system(writer, "writer");
        else g.sim_system(writer, "writer").sim_system(reader, "reader");
        g.run_headless(8, [&](ponko::ctx& ctx) {
            ctx.world().track_changes<Fuel>();
            target = ctx.world().spawn(Fuel{0}); ctx.world().spawn(Fuel{0});
        });
        const std::size_t late = reader_first ? 1 : 0;
        std::vector<std::size_t> changes(8, 0), removals(8, 0);
        changes[0] = 2; changes[1 + late] = 1; changes[3 + late] = 1; removals[5 + late] = 1;
        CHECK(seen_changes == changes && seen_removed == removals);
        CHECK(own == std::vector<std::size_t>{2, 0, 0, 0, 0, 0, 0, 0});
    }
}

struct Hp { int value; };
struct Bullet { float speed; };

// Presence filters: with, without, or_, all_of. One entity per combination of
// four markers, labelled with the combination, so each filter has a plain C++
// predicate to compare against.
struct Hero {};
struct Foe {};
struct Slain {};
struct Spirit {};
template<int N> struct Extra {};
struct Label { unsigned value; };
template<class... Fs> static unsigned labels_seen(ponko::world& w) {
    unsigned mask = 0;
    w.each<Fs...>([&](const Label& l) { mask |= 1u << l.value; });
    return mask;
}
template<class... Fs> static unsigned labels_seen_par(ponko::world& w) {
    std::atomic<unsigned> mask{0};
    w.each_par<Fs...>([&](const Label& l) { mask.fetch_or(1u << l.value); });
    return mask.load();
}
template<class F> static unsigned labels_where(F predicate) {
    unsigned mask = 0;
    for (unsigned bits = 0; bits < 16; ++bits) if (predicate(bits)) mask |= 1u << bits;
    return mask;
}
static void presence_filters() {
    using namespace ponko;
    constexpr unsigned HERO = 1, FOE = 2, SLAIN = 4, SPIRIT = 8;
    world w;
    for (unsigned bits = 0; bits < 16; ++bits) {
        const auto e = w.spawn(Label{bits});
        if (bits & HERO) w.set(e, Hero{});
        if (bits & FOE) w.set(e, Foe{});
        if (bits & SLAIN) w.set(e, Slain{});
        if (bits & SPIRIT) w.set(e, Spirit{});
    }
    auto same = [&](unsigned want, unsigned got, unsigned par) { CHECK(want == got && want == par); };
    same(labels_where([&](unsigned b) { return b & HERO; }), labels_seen<with<Hero>>(w), labels_seen_par<with<Hero>>(w));
    same(labels_where([&](unsigned b) { return !(b & SLAIN); }), labels_seen<without<Slain>>(w), labels_seen_par<without<Slain>>(w));
    // Several filters in one list must all pass.
    same(labels_where([&](unsigned b) { return (b & HERO) && !(b & SLAIN); }),
         labels_seen<with<Hero>, without<Slain>>(w), labels_seen_par<with<Hero>, without<Slain>>(w));
    same(labels_where([&](unsigned b) { return (b & HERO) && !(b & SLAIN); }),
         labels_seen<all_of<with<Hero>, without<Slain>>>(w), labels_seen_par<all_of<with<Hero>, without<Slain>>>(w));
    same(labels_where([&](unsigned b) { return b & (HERO | FOE); }),
         labels_seen<or_<with<Hero>, with<Foe>>>(w), labels_seen_par<or_<with<Hero>, with<Foe>>>(w));
    // The named filter: a fighter that isn't slain.
    using living = all_of<or_<with<Hero>, with<Foe>>, without<Slain>>;
    same(labels_where([&](unsigned b) { return (b & (HERO | FOE)) && !(b & SLAIN); }),
         labels_seen<living>(w), labels_seen_par<living>(w));
    // An AND group inside an OR: (Hero AND Foe) OR NOT Slain.
    using either = or_<all_of<with<Hero>, with<Foe>>, without<Slain>>;
    same(labels_where([&](unsigned b) { return ((b & HERO) && (b & FOE)) || !(b & SLAIN); }),
         labels_seen<either>(w), labels_seen_par<either>(w));
    // Nesting: (Hero AND (Slain OR (Spirit AND NOT Foe))) OR Foe.
    using nested = or_<all_of<with<Hero>, or_<with<Slain>, all_of<with<Spirit>, without<Foe>>>>, with<Foe>>;
    same(labels_where([&](unsigned b) { return ((b & HERO) && ((b & SLAIN) || ((b & SPIRIT) && !(b & FOE)))) || (b & FOE); }),
         labels_seen<nested>(w), labels_seen_par<nested>(w));
    // Empty groups: all_of<> is everything, or_<> nothing.
    CHECK(labels_seen<all_of<>>(w) == 0xFFFFu && labels_seen<or_<>>(w) == 0);
    CHECK(labels_seen<or_<all_of<>, with<Hero>>>(w) == 0xFFFFu && labels_seen<all_of<or_<>, with<Hero>>>(w) == 0);

    // Counting, the lower-level query, and columns beside the filters.
    CHECK(w.count<with<Hero>>() == 8 && w.count<Label, without<Slain>>() == 8 && w.count<living>() == 6);
    std::size_t rows = 0;
    w.query<const Label, with<Hero>, without<Foe>>([&](auto chunk) { rows += chunk.size(); });
    CHECK(rows == 4);
    // A filter can name a component that is also a column.
    CHECK((w.count<Hero, with<Hero>>() == 8 && w.count<Hero, without<Hero>>() == 0));

    // Change filters and presence filters combine: both must pass.
    w.track_changes<Label>();
    w.set_changes_since(w.advance_change_tick());
    ponko::entity hero_entity = 0, plain_entity = 0;
    w.each([&](ponko::entity e, const Label& l) { if (l.value == HERO) hero_entity = e; if (l.value == 0) plain_entity = e; });
    CHECK(w.mark_changed<Label>(hero_entity) && w.mark_changed<Label>(plain_entity));
    CHECK(w.count<ponko::changed<Label>>() == 2 && w.count<ponko::changed<Label>, with<Hero>>() == 1);
    CHECK(labels_seen<ponko::changed<Label>, without<Hero>>(w) == 1u);
    // any_of picks rows by what changed, and presence filters still narrow them.
    CHECK((w.count<ponko::any_of<ponko::changed<Label>, ponko::added<Label>>, or_<with<Hero>, with<Foe>>>() == 1));

    // The same filter list again, and in a second world with other ids.
    world other;
    other.spawn(Spirit{}); other.spawn(Hero{}, Label{HERO}); other.spawn(Label{0});
    CHECK(labels_seen<with<Hero>>(other) == 1u << HERO && labels_seen<with<Hero>>(w) == labels_where([&](unsigned b) { return b & HERO; }));
    CHECK(other.count<or_<with<Hero>, with<Spirit>>>() == 2 && other.count<without<Hero>>() == 2);

    // Limits and mistakes are exceptions, never a crash.
    throws_invalid([&] { (void)w.count<with<Extra<0>>, with<Extra<1>>, with<Extra<2>>, with<Extra<3>>, with<Extra<4>>,
                                       with<Extra<5>>, with<Extra<6>>, with<Extra<7>>, with<Extra<8>>>(); }, "at most 8 `with`");
    CHECK((w.count<with<Extra<0>>, with<Extra<1>>, with<Extra<2>>, with<Extra<3>>, with<Extra<4>>,
                   with<Extra<5>>, with<Extra<6>>, with<Extra<7>>>() == 0));
    throws_invalid([&] { (void)w.count<or_<with<Hero>>, or_<with<Foe>>, or_<with<Slain>>, or_<with<Spirit>>, or_<with<Label>>>(); },
                   "at most 4 `Or`");
    throws_invalid([&] { (void)w.count<or_<all_of<with<Hero>, with<Foe>, with<Label>>, all_of<with<Slain>, with<Spirit>>>>(); },
                   "at most 4 `Or`");
    // A repeat is one slot and a contradiction matches nothing, as for a Rust tuple.
    CHECK(w.count<with<Hero>, with<Hero>>() == 8 && w.count<without<Hero>, without<Hero>>() == 8);
    CHECK(w.count<with<Hero>, without<Hero>>() == 0 && w.count<without<Hero>, with<Hero>>() == 0);
    CHECK(labels_seen<with<Hero>, with<Hero>>(w) == labels_where([&](unsigned b) { return b & HERO; }));
    CHECK(labels_seen<all_of<with<Foe>, with<Foe>>, with<Foe>>(w) == labels_where([&](unsigned b) { return b & FOE; }));
    CHECK(labels_seen_par<with<Hero>, without<Hero>>(w) == 0);
    // A failed query leaves the world usable, and the working lists working.
    CHECK(w.count<with<Hero>>() == 8);
    // A type used for the first time inside a query can't be registered there.
    throws([&] { w.each([&](const Label&) { (void)w.count<with<Extra<20>>>(); }); });
}

// Queries over more than the 8 columns the first Rust tuples had: 12 in
// `each`, and a presence filter beside them.
template<int N> struct Col { int value; };
static void wide_queries() {
    ponko::world w;
    const auto spawn = [&](bool with_hero) {
        const auto e = w.spawn(Col<0>{0}, Col<1>{1}, Col<2>{2}, Col<3>{3}, Col<4>{4}, Col<5>{5},
                               Col<6>{6}, Col<7>{7}, Col<8>{8}, Col<9>{9}, Col<10>{10}, Col<11>{11});
        if (with_hero) w.set(e, Hero{});
        return e;
    };
    spawn(false); spawn(true); spawn(true);
    auto sum = [&](auto filters) {
        int total = 0, rows = 0;
        auto visit = [&](const Col<0>& a, const Col<1>& b, const Col<2>& c, const Col<3>& d, const Col<4>& e, const Col<5>& f,
                         const Col<6>& g, const Col<7>& h, const Col<8>& i, const Col<9>& j, const Col<10>& k, Col<11>& l) {
            total += a.value + b.value + c.value + d.value + e.value + f.value + g.value + h.value + i.value + j.value + k.value + l.value;
            ++rows;
        };
        filters(visit);
        return std::pair<int, int>(total, rows);
    };
    CHECK(sum([&](auto f) { w.each(f); }) == std::pair<int, int>(66 * 3, 3));
    CHECK(sum([&](auto f) { w.each<ponko::with<Hero>>(f); }) == std::pair<int, int>(66 * 2, 2));
    CHECK(sum([&](auto f) { w.each<ponko::without<Hero>>(f); }) == std::pair<int, int>(66, 1));
}

// The allocations per frame of system `name` in a profile report's JSON
// (the average over the frames, so any allocation shows), -1 if it isn't there.
static double system_allocations(const std::string& json, const std::string& name) {
    const auto at = json.find("\"name\": \"" + name + "\"");
    if (at == std::string::npos) return -1;
    const std::string key = "\"allocs_per_frame\":";
    const auto value = json.find(key, at);
    if (value == std::string::npos) return -1;
    return std::strtod(json.c_str() + value + key.size(), nullptr);
}

// What the wrapper does for a presence-filtered query in steady state: the
// filter is built once per list and world, so a repeated call allocates
// nothing (C++ `new` and the engine's own allocations are both counted, per
// system, and must be exactly zero).
// The archetype index: 128 archetypes (one per subset of seven markers), so
// the default policy applies. Hints and the world's policy change how a
// query finds its archetypes, never what it visits.
template<class... Fs> static std::vector<unsigned> labels_in_order(ponko::world& w) {
    std::vector<unsigned> seen;
    w.each<Fs...>([&](const Label& l) { seen.push_back(l.value); });
    return seen;
}
static void query_cache_hints() {
    using namespace ponko;
    world w;
    for (unsigned bits = 0; bits < 128; ++bits) {
        const auto e = w.spawn(Label{bits});
        if (bits & 1) w.set(e, Extra<0>{});
        if (bits & 2) w.set(e, Extra<1>{});
        if (bits & 4) w.set(e, Extra<2>{});
        if (bits & 8) w.set(e, Extra<3>{});
        if (bits & 16) w.set(e, Extra<4>{});
        if (bits & 32) w.set(e, Extra<5>{});
        if (bits & 64) w.set(e, Extra<6>{});
    }
    CHECK(w.query_cache_stats().signatures == 0);
    // uncached: every archetype is tested, the index is not touched.
    const auto scanned = labels_in_order<with<Extra<0>>, without<Extra<1>>, uncached>(w);
    CHECK(scanned.size() == 32 && w.query_cache_stats().signatures == 0);
    // No hint: the world indexes the query from its second run.
    for (int i = 0; i < 3; ++i) CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>>(w) == scanned));
    auto stats = w.query_cache_stats();
    CHECK(stats.signatures == 1 && stats.indexed == 1 && stats.indices == 32 && stats.hits == 2 && stats.bytes > 0);
    // cached: a query the policy leaves alone (it matches nearly every
    // archetype) is indexed on its first run. Same entities, same order.
    const auto all = labels_in_order<uncached>(w);
    CHECK(all.size() == 128 && (labels_in_order<>(w) == all) && (labels_in_order<>(w) == all));
    CHECK(w.query_cache_stats().indexed == 1);
    CHECK((labels_in_order<cached>(w) == all) && w.query_cache_stats().indexed == 2);
    // Hints combine with every other filter, with query, count and each_par.
    CHECK((w.count<Label, cached, or_<with<Extra<2>>, with<Extra<3>>>>() == 96));
    CHECK((w.count<Label, uncached, or_<with<Extra<2>>, with<Extra<3>>>>() == 96));
    std::size_t rows = 0;
    w.query<const Label, with<Extra<6>>, cached>([&](auto chunk) { rows += chunk.size(); });
    CHECK(rows == 64);
    std::atomic<unsigned> par{0};
    w.each_par<cached, with<Extra<5>>, with<Extra<6>>>([&](const Label&) { par.fetch_add(1); });
    CHECK(par.load() == 32);
    w.track_changes<Label>();
    CHECK((w.count<ponko::changed<Label>, cached>() == 128));
    // An archetype created later is listed before the next query.
    const auto late = w.spawn(Label{1}, Extra<0>{}, Hero{});
    CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>>(w).size() == 33));
    CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>, uncached>(w).size() == 33));
    w.despawn(late);
    CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>>(w) == scanned));

    // The world's policy: off for the whole world, hints included.
    CHECK(w.query_cache_policy().min_archetypes == ponko_query_cache_policy_default().min_archetypes);
    w.set_query_cache_policy(ponko_query_cache_policy_off());
    stats = w.query_cache_stats();
    CHECK(stats.signatures == 0 && stats.bytes == 0 && w.query_cache_policy().min_archetypes == SIZE_MAX);
    const auto hits = stats.hits;
    CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>, cached>(w) == scanned));
    CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>>(w) == scanned));
    CHECK(w.query_cache_stats().hits == hits && w.query_cache_stats().signatures == 0);
    // And tuned: index from the first run.
    query_cache_policy tuned = ponko_query_cache_policy_default();
    tuned.promote_after = 1;
    w.set_query_cache_policy(tuned);
    CHECK((labels_in_order<with<Extra<0>>, without<Extra<1>>>(w) == scanned));
    CHECK(w.query_cache_stats().hits == hits + 1 && w.query_cache_policy().promote_after == 1);
    tuned.reserved[0] = 1;
    throws_invalid([&] { w.set_query_cache_policy(tuned); }, "reserved");
    tuned.reserved[0] = 0;
    tuned.max_match_percent = 101;
    w.set_query_cache_policy(tuned);
    CHECK(w.query_cache_policy().max_match_percent == 100);
}

static void presence_filters_allocate_nothing() {
    using namespace ponko;
    auto setup = [](ponko::ctx& ctx) {
        auto w = ctx.world();
        for (unsigned bits = 0; bits < 64; ++bits) {
            const auto e = w.spawn(Label{bits});
            if (bits & 1) w.set(e, Hero{});
            if (bits & 2) w.set(e, Foe{});
            if (bits & 4) w.set(e, Slain{});
        }
    };
    std::size_t each_rows = 0, query_rows = 0, count_rows = 0, every_rows = 0;
    auto each_filtered = [&](ponko::ctx& ctx) {
        auto w = ctx.world();
        each_rows = 0;
        w.each<with<Hero>>([&](const Label&) { ++each_rows; });
        w.each<or_<all_of<with<Hero>, with<Foe>>, without<Slain>>, with<Label>>([&](const Label&) { ++each_rows; });
    };
    auto query_filtered = [&](ponko::ctx& ctx) {
        query_rows = 0;
        ctx.world().query<const Label, without<Foe>>([&](auto chunk) { query_rows += chunk.size(); });
    };
    auto count_filtered = [&](ponko::ctx& ctx) {
        count_rows = ctx.world().count<all_of<or_<with<Hero>, with<Foe>>, without<Slain>>>();
    };
    // The same three without a filter, to show the report sees allocations at all.
    auto leaky = [](ponko::ctx&) { int* p = new int[4]; *static_cast<volatile int*>(p) = 7; delete[] p; };
    auto parallel_filtered = [&](ponko::ctx& ctx) { ctx.world().each_par<with<Hero>>([](const Label&) {}); };
    auto parallel_plain = [&](ponko::ctx& ctx) { ctx.world().each_par([](const Label&) {}); };
    auto every = [&](ponko::ctx& ctx) { every_rows = ctx.world().count<Label>(); };
    auto run = ponko::game("filter allocations", 1, 1)
        .sim_system(each_filtered, "each_filtered")
        .sim_system(query_filtered, "query_filtered")
        .sim_system(count_filtered, "count_filtered")
        .sim_system(every, "every")
        .sim_system(leaky, "leaky")
        .sim_system(parallel_filtered, "parallel_filtered")
        .sim_system(parallel_plain, "parallel_plain")
        .profile_headless(5, 40, setup);
    CHECK(each_rows == 32 + 40 && query_rows == 32 && count_rows == 24 && every_rows == 64);
    const std::string json = run.report.json();
    CHECK(system_allocations(json, "leaky") >= 1.0);   // the report does see allocations
    CHECK(system_allocations(json, "each_filtered") == 0.0);
    CHECK(system_allocations(json, "query_filtered") == 0.0);
    CHECK(system_allocations(json, "count_filtered") == 0.0);
    CHECK(system_allocations(json, "every") == 0.0);
    // On all cores the thread pool may allocate now and then; the filter must
    // not add an allocation to every call (one call per frame here).
    const double plain = system_allocations(json, "parallel_plain");
    CHECK(plain >= 0 && system_allocations(json, "parallel_filtered") >= 0);
    CHECK(system_allocations(json, "parallel_filtered") < plain + 0.5);
}

static void games() {
    // Systems, the clock, quit and exceptions.
    std::uint64_t ticks = 0; float dt = 0;
    auto sim = ponko::game("t", 1, 1).fixed_hz(50)
        .sim_system([&](ponko::ctx& ctx) {
            ++ticks; dt = ctx.dt();
            CHECK(ctx.gamepads().empty());   // no pads in headless runs
            if (ctx.sim_tick() == 2) ctx.quit();
        }, "count")
        .run_headless(100, [&](ponko::ctx& ctx) { ctx.world().spawn(Pos{1, 1}); });
    CHECK(ticks == 3 && dt > .019f && dt < .021f && sim.size() == 1);
    // The returned world knows the game's components.
    sim.each([](const Pos& p) { CHECK(p.x == 1); });
    throws([] { ponko::game("t", 1, 1).run_headless(1, [](ponko::ctx&) { throw std::runtime_error("setup"); }); });
    bool second = false;
    throws([&] { ponko::game("t", 1, 1).sim_system([](ponko::ctx&) { throw std::runtime_error("tick"); })
                     .sim_system([&](ponko::ctx&) { second = true; }).run_headless(5); });
    CHECK(!second);
    throws([] { ponko::game("t", 1, 1).fixed_hz(0); });
    throws([] { ponko::game("t", 1, 1).remove_system("nope"); });
    ponko::game g("t", 1, 1);
    g.run_headless(1);
    throws([&] { g.run_headless(1); });

    // Commands from inside a loop (Bullet is first used there); spatial
    // queries; random streams; built-ins.
    std::vector<ponko::grid_hit> hits;
    std::size_t near = 0;
    std::uint64_t first_random = 0;
    auto w = ponko::game("t", 1, 1).spatial_grid(16).seed(3)
        .sim_system([&](ponko::ctx& ctx) {
            ctx.world().each([&](ponko::entity e, const Hp& hp) {
                if (hp.value <= 0) { ctx.despawn_later(e); ctx.spawn_later(Hp{100}, Bullet{2}); }
            });
            ctx.spatial_radius({45, 0}, 16, hits);
            near = hits.size();
            CHECK(ctx.spatial_nearest({1, 0}, 100).has_value());
            CHECK(!ctx.spatial_nearest({1e6f, 0}, 1).has_value());
            first_random = ctx.random().of(7).u64();
            ctx.quit();
        })
        .run_headless(10, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            std::uint8_t px[16] = {};
            auto tex = ctx.create_texture(2, 2, px, sizeof px);
            for (int x = 0; x < 10; ++x)
                world.spawn(Hp{x % 2 ? 5 : 0}, ponko::transform_at({x * 10.0f, 0}),
                            ponko::sprite_of(tex, ponko::colors::red, float(x)));
        });
    CHECK(w.size() == 10);
    CHECK(near == 4);
    long long total = 0;
    w.each([&](const Hp& hp) { total += hp.value; });
    CHECK(total == 5 * 5 + 5 * 100);
    CHECK(w.count<Hp, Bullet>() == 5);
    CHECK(first_random == ponko::random(0).of(7).u64() || first_random != 0);

    // set_later and remove_later, also for types first used inside a loop.
    auto changed = ponko::game("t", 1, 1)
        .sim_system([](ponko::ctx& ctx) {
            ctx.world().each([&](ponko::entity e, Hp& hp) {
                if (hp.value == 1) ctx.set_later(e, Vel{1, 1});
                if (hp.value == 2) ctx.remove_later<Tag>(e);
            });
        })
        .run_headless(1, [](ponko::ctx& ctx) { ctx.world().spawn(Hp{1}); ctx.world().spawn(Hp{2}, Tag{}); });
    CHECK(changed.count<Vel>() == 1 && changed.count<Tag>() == 0);
}

static void grid_views_match_engine_queries() {
    ponko::grid g(8);
    ponko::rng r(5);
    std::vector<ponko::grid_item> items;
    for (std::uint32_t i = 0; i < 20000; ++i)
        items.push_back({{i, 0}, {r.range(-500, 500), r.range(-500, 500)}, i});
    g.rebuild(items);
    auto view = g.view();
    CHECK(view.size() == items.size());
    std::vector<ponko::grid_item> engine;
    for (int q = 0; q < 300; ++q) {
        const ponko::vec2 c{r.range(-600, 600), r.range(-600, 600)};
        const float radius = q % 50 == 0 ? 2000.0f : r.range(0, 40);
        g.radius(c, radius, engine);
        std::vector<std::uint32_t> a, b;
        for (auto& e : engine) a.push_back(e.data);
        view.for_each_in_radius(c, radius, [&](const ponko::grid_item& e) { b.push_back(e.data); });
        std::sort(a.begin(), a.end());
        std::sort(b.begin(), b.end());
        CHECK(a == b);
    }
    throws([&] { ponko::grid_view<ponko::grid_hit> wrong(PonkoGridView{nullptr, 20, nullptr, nullptr, 1, 0, 0, 0, 0, 0}); });
}

struct Died { ponko::entity who; std::uint64_t tick; };
struct Never { int x; };

static void events() {
    // Sent from inside a loop (the first send is the type's first use), read
    // by a later system in the same tick, once each.
    std::vector<std::uint64_t> seen;
    auto w = ponko::game("t", 1, 1)
        .sim_system([](ponko::ctx& ctx) {
            ctx.world().each([&](ponko::entity e, const Hp& hp) {
                if (hp.value <= 0) ctx.send(Died{e, ctx.sim_tick()});
            });
        }, "deaths")
        .sim_system([&](ponko::ctx& ctx) {
            for (const Died& d : ctx.read<Died>()) { seen.push_back(d.tick); ctx.despawn_later(d.who); }
            CHECK(ctx.read<Died>().empty());
            CHECK(ctx.read<Never>().empty());
        }, "funerals")
        .run_headless(3, [](ponko::ctx& ctx) {
            for (int h : {0, 5, -1}) ctx.world().spawn(Hp{h});
        });
    CHECK(w.size() == 1);
    CHECK((seen == std::vector<std::uint64_t>{0, 0}));
}

static void rng_matches_rust() {
    // The same numbers as Rust's Rng (pinned in src/random.rs).
    ponko::rng r(42);
    CHECK(r.u64() == 0xc757960b442b0ac3ull);
    CHECK(r.u64() == 0x4bb22a7f77ff8c6cull);
    CHECK(r.u64() == 0x04950439d3c5eafeull);
    CHECK(ponko::random(7).stream(3).u64() == 0x6045c0e00af2e148ull);
    ponko::rng f(1);
    for (int i = 0; i < 1000; ++i) { float x = f.f32(); CHECK(x >= 0 && x < 1); CHECK(f.below(10) < 10); }
}

static void attachments() {
    // Chains follow their root; orphans are despawned with it.
    ponko::entity root = 0, arm = 0, hand = 0;
    ponko::transform2d seen{};
    auto w = ponko::game("t", 1, 1)
        .sim_system([&](ponko::ctx& ctx) {
            auto world = ctx.world();
            auto t = world.get<ponko::transform2d>(root);
            t.position.x += 1;
            world.set(root, t);
            if (ctx.sim_tick() == 2) ctx.despawn_later(root);
        })
        .follow_attachments()
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 1) seen = ctx.world().get<ponko::transform2d>(hand);
        })
        .run_headless(3, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            root = world.spawn(ponko::transform_at({5, 5}));
            arm = world.spawn(ponko::transform_at({0, 0}), ponko::attach_to(root, {1, 0}));
            hand = world.spawn(ponko::transform_at({0, 0}), ponko::attach_to(arm, {0, 1}, 0.5f));
        });
    CHECK(seen.position == (ponko::vec2{8, 6}) && seen.rotation == 0.5f);
    CHECK(ponko::bits(ponko::entity_of(arm)) == arm);
    CHECK(!w.alive(root) && !w.alive(arm) && !w.alive(hand));
}

static void attachments3d() {
    // A 3D chain follows its parent, turned with it.
    ponko::entity base = 0, arm = 0;
    ponko::transform3d seen{};
    ponko::game("t", 1, 1)
        .follow_attachments()
        .sim_system([&](ponko::ctx& ctx) { seen = ctx.world().get<ponko::transform3d>(arm); })
        .run_headless(1, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            base = world.spawn(ponko::transform3d_at({10, 0, 0}, ponko::quat_from_rotation_y(1.5707964f)));
            arm = world.spawn(ponko::transform3d_at({0, 0, 0}),
                              ponko::attach3d_to(base, ponko::transform3d_at({0, 2, -5})));
        });
    CHECK(std::fabs(seen.position.x - 5) < 1e-4f && std::fabs(seen.position.y - 2) < 1e-4f &&
          std::fabs(seen.position.z) < 1e-4f);
}

static void physics() {
    // A ball drops through a sensor onto the floor; queries and events work from C++.
    ponko::entity ball = 0, floor = 0, sensor = 0;
    std::vector<ponko::collision> started;
    std::optional<ponko::ray_hit> hit;
    std::optional<float> mass;
    bool kicked_nothing = true;
    auto w = ponko::game("t", 1, 1)
        .physics({0, 980})
        .sim_system([&](ponko::ctx& ctx) {
            for (const auto& c : ctx.collisions_started()) started.push_back(c);
            if (ctx.sim_tick() == 1) {
                hit = ctx.cast_ray({150, 0}, {0, 1}, 1000);
                mass = ctx.mass(ball);
                kicked_nothing = ctx.apply_impulse(12345, {1, 0});
                std::vector<PonkoEntity> at;
                ctx.entities_at({150, 300}, at);
                CHECK(at.size() == 1 && ponko::bits(at[0]) == floor);
            }
        })
        .run_headless(120, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            auto bouncy = ponko::circle_collider(10);
            bouncy.restitution = 0;
            ball = world.spawn(ponko::transform_at({0, 0}), ponko::dynamic_body(), bouncy, ponko::velocity{});
            auto trigger = ponko::rect_collider({80, 20});
            trigger.sensor = true;
            sensor = world.spawn(ponko::transform_at({0, 100}), ponko::fixed_body(), trigger);
            floor = world.spawn(ponko::transform_at({0, 300}), ponko::fixed_body(), ponko::rect_collider({400, 20}));
        });
    CHECK(std::fabs(w.get<ponko::transform2d>(ball).position.y - 280) < 2);
    CHECK(std::fabs(w.get<ponko::velocity>(ball).linear.y) < 5);
    CHECK(w.get<ponko::rigid_body>(floor).kind == PONKO_BODY_FIXED);
    CHECK(w.get<ponko::collider>(sensor).sensor);
    auto touched = [&](ponko::entity a, ponko::entity b) {
        return std::any_of(started.begin(), started.end(), [&](const ponko::collision& c) {
            const auto x = ponko::bits(c.a), y = ponko::bits(c.b);
            return (x == a && y == b) || (x == b && y == a);
        });
    };
    CHECK(touched(ball, sensor) && touched(ball, floor));

    // A wedge, flat side down, comes to rest on it (as tests/physics.rs).
    ponko::entity wedge = 0;
    auto rested = ponko::game("t", 1, 1).physics({0, 980}).run_headless(240, [&](ponko::ctx& ctx) {
        auto world = ctx.world();
        auto shape = ponko::polygon_collider({{-30, 15}, {30, 15}, {-30, -15}});
        shape.friction = 0.9f;
        wedge = world.spawn(ponko::transform_at({0, 0}), ponko::dynamic_body(), shape);
        world.spawn(ponko::transform_at({0, 200}), ponko::fixed_body(), ponko::rect_collider({400, 20}));
    });
    const auto t = rested.get<ponko::transform2d>(wedge);
    CHECK(std::fabs(t.position.y - 175) < 1.5f && std::fabs(t.rotation) < 0.01f);
    CHECK(rested.collider_points(wedge).size() == 3);
    CHECK(rested.get<ponko::collider>(wedge).shape == PONKO_SHAPE_POLYGON);
    CHECK(rested.get<ponko::collider>(wedge).friction == 0.9f);
    throws([] { ponko::polygon_collider({{0, 0}, {1, 0}}); });
    CHECK(hit && ponko::bits(hit->entity) == floor && std::fabs(hit->distance - 290) < 1e-3f);
    CHECK(mass && std::fabs(*mass - 314.159f) < 1);
    CHECK(!kicked_nothing);
    // Bad values are rejected, not simulated.
    auto flat = ponko::rect_collider({10, 0});
    throws([&] { w.set(floor, flat); });
}

static void phases() {
    // add(phase::before_physics) feeds this tick's step; a sim_system, even
    // one added before physics3d, runs after the step (as tests/phases.rs).
    ponko::entity ball = 0;
    float before = -1, after = -1;
    ponko::game("t", 1, 1)
        .fixed_hz(50)
        .sim_system([&](ponko::ctx& ctx) { after = ctx.world().get<ponko::transform3d>(ball).position.x; }, "watch")
        .add(ponko::phase::before_physics, [&](ponko::ctx& ctx) {
            before = ctx.world().get<ponko::transform3d>(ball).position.x;
            ctx.world().set(ball, ponko::velocity3d{{5, 0, 0}, {0, 0, 0}});
        }, "kick")
        .physics3d({0, 0, 0})
        .run_headless(1, [&](ponko::ctx& ctx) {
            ball = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::dynamic_body3d(),
                                     ponko::sphere_collider(0.5f), ponko::velocity3d{{0, 0, 0}, {0, 0, 0}});
        });
    CHECK(before == 0);
    CHECK(std::fabs(after - 5.0f / 50.0f) < 1e-3f);
    throws([] { ponko::game("t", 1, 1).add(ponko::phase::frame, [](ponko::ctx&) {}, "bad", 2); });
}

// A plugin with startup work, and one that only adds systems.
struct ScoreArea {
    std::string* log;
    void build(ponko::game& game) const {
        game.sim_system([log = log](ponko::ctx&) { *log += 'a'; }, "score");
    }
    void setup(ponko::ctx&) const { *log += 's'; }
};
struct KickArea {
    std::string* log;
    void build(ponko::game& game) const {
        game.add(ponko::phase::before_physics, [log = log](ponko::ctx&) { *log += 'b'; }, "kick");
    }
};

static void plugins_setups_and_system_ids() {
    // Plugins build at once and set up in the order they were added, with
    // setup() callbacks in between and the run setup last. The phase, not
    // the plugin order, decides when their systems run.
    std::string log;
    ponko::game("t", 1, 1)
        .plugin(ScoreArea{&log})
        .setup([&](ponko::ctx&) { log += 'c'; })
        .plugin(KickArea{&log})
        .run_headless(2, [&](ponko::ctx&) { log += 'r'; });
    CHECK(log == "scrbaba");
    // setup() alone runs with zero ticks, and applies its commands.
    auto w = ponko::game("t", 1, 1).setup([](ponko::ctx& ctx) { ctx.spawn_later(Pos{1, 2}); }).run_headless(0);
    CHECK(w.size() == 1);

    // Ids: two systems from the same function are told apart.
    log.clear();
    ponko::game g("t", 1, 1);
    auto mark = [&](char c) { return [&log, c](ponko::ctx&) { log += c; }; };
    ponko::system_id first = g.add_system(ponko::phase::after_physics, mark('1'), "same");
    ponko::system_id second = g.add_system(ponko::phase::after_physics, mark('2'), "same");
    CHECK(first != second && first.bits != 0);
    g.remove(first).remove(first);                  // twice is fine
    g.remove(ponko::systems::draw_sprites)
        .remove(ponko::systems::update_animators)
        .remove(ponko::systems::step_navmesh3d);    // not installed: nothing happens
    throws([&] { g.remove_system("ponko::ecs::draw_sprites"); });   // already gone
    g.run_headless(2);
    CHECK(log == "22");
    throws([&] { g.setup([](ponko::ctx&) {}); });   // already run
    throws([&] { g.plugin(KickArea{&log}); });
    throws([] { ponko::game("t", 1, 1).add_system(ponko::phase::frame, [](ponko::ctx&) {}, "bad", 2); });
    CHECK(ponko::systems::draw_sprites.bits == PONKO_SYSTEM_DRAW_SPRITES);
}

static void vehicles() {
    // A box car on raycast wheels settles on its suspension, then drives
    // off; the vehicle is a query column (as tests/vehicle3d.rs).
    ponko::entity car = 0;
    std::optional<ponko::vec3> center;
    auto w = ponko::game("t", 1, 1)
        .add(ponko::phase::before_physics, [](ponko::ctx& ctx) {
            if (ctx.sim_tick() < 100) return;
            ctx.world().each([](ponko::vehicle3d& v) {
                v.wheels[2].motor_torque = v.wheels[3].motor_torque = 500;
                v.wheels[0].steer_angle = v.wheels[1].steer_angle = 0.2f;
            });
        }, "drive")
        .physics3d({0, -9.81f, 0})
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 99) center = ctx.center_of_mass3d(car);
        }, "probe")
        .run_headless(200, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            world.spawn(ponko::transform3d_at({0, -0.5f, 0}), ponko::fixed_body3d(), ponko::box_collider({200, 1, 200}));
            const float x = 0.8f, y = -0.3f, z = 1.3f, r = 0.35f;
            car = world.spawn(ponko::transform3d_at({0, 1, 0}),
                              ponko::with_center_of_mass(ponko::with_mass(ponko::dynamic_body3d(), 1200), {0, -0.3f, 0}),
                              ponko::box_collider({1.8f, 0.6f, 4.2f}),
                              ponko::vehicle({ponko::wheel({-x, y, -z}, r), ponko::wheel({x, y, -z}, r),
                                               ponko::wheel({-x, y, z}, r), ponko::wheel({x, y, z}, r)}));
        });
    CHECK(center && std::fabs(center->y - 0.5f) < 0.02f);
    const auto v = w.get<ponko::vehicle3d>(car);
    const auto t = w.get<ponko::transform3d>(car);
    CHECK(v.wheel_count == 4 && v.wheels[0].grounded && ponko::rpm(v.wheels[3]) > 50);
    CHECK(t.position.z < -3 && t.position.x > 0.2f && t.position.y > 0.7f && t.position.y < 0.9f);
    CHECK(std::fabs(ponko::wheel_pose(v.wheels[0], t).position.y - 0.35f) < 0.05f);
    CHECK(ponko::evaluate(v.wheels[0].forward_friction, 0.4f) == 1);
    throws([] { ponko::vehicle({ponko::wheel({}, 1), ponko::wheel({}, 1), ponko::wheel({}, 1), ponko::wheel({}, 1),
                                 ponko::wheel({}, 1), ponko::wheel({}, 1), ponko::wheel({}, 1), ponko::wheel({}, 1),
                                 ponko::wheel({}, 1)}); });
}

static void lockstep() {
    // Two games over UDP on localhost see the same inputs, tick for tick.
    struct Order { std::uint32_t tick; float x; };
    struct Seen { std::uint64_t sum = 0; std::uint32_t others = 0, player = 9; bool desync = false; std::size_t loaded = 0; };
    // Both peers load it in setup and again at tick 50, in a sim system:
    // places every peer runs, so the loads keep them in step.
    ponko::game("scene", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ctx.world().spawn(ponko::transform_at({1, 2}));
        ctx.world().spawn(ponko::transform_at({3, 4}));
        ctx.save_scene("test-cpp-lockstep.json");
    });
    auto play = [](ponko::game& g, Seen& seen) {
        g.sim_system([&seen](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 50) {
                ctx.world().spawn(ponko::transform_at({5, 6}));
                ctx.load_scene_lockstep("test-cpp-lockstep.json");
                seen.loaded = ctx.world().size();
            }
            seen.player = ctx.player();
            ctx.set_input(Order{std::uint32_t(ctx.sim_tick()), float(ctx.player())});
            for (std::uint32_t p = 0; p < ctx.players(); ++p)
                if (auto o = ctx.input<Order>(p)) {
                    seen.sum += o->tick * (p + 1);
                    if (p != ctx.player()) ++seen.others;
                }
            seen.desync = ctx.desynced();
        });
        g.run_headless(200, [](ponko::ctx& ctx) { ctx.load_scene("test-cpp-lockstep.json"); });
    };
    std::uint16_t port = 0;
    Seen host_seen, guest_seen;
    ponko::game host("host", 1, 1);
    host.lockstep_host("127.0.0.1:0", 2, 2, 30, &port);
    CHECK(port != 0);
    std::thread host_thread([&] { play(host, host_seen); });
    ponko::game guest("guest", 1, 1);
    guest.lockstep_join(("127.0.0.1:" + std::to_string(port)).c_str());
    play(guest, guest_seen);
    host_thread.join();
    CHECK(host_seen.player == 0 && guest_seen.player == 1);
    CHECK(host_seen.sum == guest_seen.sum && host_seen.others > 150 && guest_seen.others > 150);
    CHECK(!host_seen.desync && !guest_seen.desync);
    CHECK(host_seen.loaded == 2 && guest_seen.loaded == 2);
    throws([] { ponko::game("t", 1, 1).lockstep_host("127.0.0.1:0", 1); });
    throws([] { ponko::game("t", 1, 1).lockstep_steam_host(480, 2); });   // built without `steam`
}

static void streaming() {
    ponko::cell_streamer cells(10, 25, 40);
    cells.update({5, 0, 5}, 2);
    CHECK(cells.to_load().size() == 2 && cells.to_load()[0].x == 0 && cells.to_load()[0].z == 0);
    cells.update({5, 0, 5}, 1000);
    CHECK(cells.loaded_count() > 2 && cells.is_loaded({1, 0}) && !cells.is_loaded({9, 9}));
    cells.update({5000, 0, 5}, 1000);
    CHECK(!cells.to_drop().empty() && !cells.is_loaded({0, 0}));
    CHECK(cells.corner({2, -1}).x == 20 && cells.corner({2, -1}).z == -10);
    cells.clear();
    CHECK(cells.loaded_count() == 0);
    throws([] { ponko::cell_streamer(0, 1, 1); });
    throws([] { ponko::terrain_tile_data(std::vector<float>(8, 0.0f), 4); });
    ponko::heightfield slope({0, 2, 0, 2}, 1);
    CHECK(std::abs(*slope.height_at(0.5f, 0.5f) - 1) < 1e-5f && slope.normal_at(0.5f, 0.5f)->x < 0);
    CHECK(!slope.height_at(2, 0.5f) && !slope.normal_at(-1, 0));
    throws([] { ponko::heightfield({0, 0, 0}, 1); });
    ponko::game("stream", 64, 64).run_headless(0, [](ponko::ctx& ctx) {
        auto ground = ctx.create_material(ponko::material_of(ponko::colors::white));
        // Built on another thread, uploaded on the game's.
        auto data = std::async(std::launch::async, [] { return ponko::terrain_tile_data(std::vector<float>(33 * 33, 1.0f), 64); });
        auto tile = ctx.upload_terrain_tile(data.get(), ground);
        CHECK(tile.lod_distance == 64 && ctx.is_mesh_live(tile.lods[0]));
        ctx.free_terrain_tile(tile);
        CHECK(!ctx.is_mesh_live(tile.lods[0]) && !ctx.is_mesh_live(tile.lods[3]));
        auto cube = ctx.create_mesh(ponko::mesh_data::cube(1), ground);
        CHECK(ctx.mesh_shadows(cube));
        ctx.set_mesh_shadows(cube, false);
        CHECK(!ctx.mesh_shadows(cube));
        ctx.free_mesh(cube);
        ctx.free_mesh(cube);
        CHECK(!ctx.is_mesh_live(cube));
    });
}

static void model_material_overrides() {
    ponko::game("overrides", 64, 64).run_headless(2, [](ponko::ctx& ctx) {
        auto gold = ctx.create_material(ponko::material_of(ponko::rgb(1, .8f, .2f)));
        auto wood = ctx.create_material(ponko::material_of(ponko::rgb(.5f, .3f, .1f)));
        auto cube = ctx.create_plain_mesh(ponko::mesh_data::cube(1));
        CHECK(ctx.is_mesh_live(cube));
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(cube));
        CHECK(!ctx.model_material(e));
        ctx.set_model_material(e, gold);
        CHECK(ctx.model_material(e) && ctx.model_material(e)->id == gold.id);
        throws([&] { ctx.set_model_material(e, ponko::material{99}); });
        ctx.clear_model_material(e);
        CHECK(!ctx.model_material(e));

        CHECK(ctx.model_part_materials(e).empty());
        ctx.set_model_part_materials(e, {{3, gold}, {0, wood}, {3, wood}});
        auto parts = ctx.model_part_materials(e);
        CHECK(parts.size() == 2 && parts[0].first == 0 && parts[0].second.id == wood.id);
        CHECK(parts[1].first == 3 && parts[1].second.id == wood.id);
        throws([&] { ctx.set_model_part_materials(e, {{0, ponko::material{99}}}); });
        CHECK(ctx.model_part_materials(e).size() == 2);
        ctx.set_model_part_materials(e, {});
        CHECK(ctx.model_part_materials(e).empty());
        // Left on for the frames after setup.
        ctx.set_model_material(e, gold);
        ctx.set_model_part_materials(e, {{0, wood}});
    });
}

static void asset_streaming() {
    ponko::game("streaming", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ctx.set_deterministic_uploads(true);
        ctx.set_upload_budget(std::nullopt);
        CHECK(!ctx.stream_texture("crab.png"));
        CHECK(ctx.asset_state(ponko::residency_kind::texture, "crab.png") == ponko::asset_state::loading);
        ctx.finish_uploads();
        auto crab = ctx.stream_texture("crab.png");
        CHECK(crab && crab->width > 0 && ctx.is_live(*crab));
        CHECK(ctx.asset_state(ponko::residency_kind::texture, "crab.png") == ponko::asset_state::resident);
        CHECK(!ctx.upload_stats().has_budget && ctx.upload_stats().pending == 0);
        CHECK(ctx.resident_texture("crab.png").id == crab->id);
        CHECK(ctx.is_resident(ponko::residency_kind::texture, "crab.png"));
        ctx.set_residency_budget(ponko::residency_kind::texture, 1);
        auto r = ctx.residency(ponko::residency_kind::texture);
        CHECK(r.has_budget && r.budget == 1 && r.resident == 1 && r.over_budget_bytes > 0);  // asked for this tick
        auto house = ctx.resident_mesh("models/house.glb", true);
        CHECK(ctx.is_live(house) && ctx.residency(ponko::residency_kind::mesh).pinned == 1);
        ctx.unpin_resident(ponko::residency_kind::mesh, "models/house.glb");
        CHECK(!ctx.stream_mesh("models/nope.glb"));
        ctx.finish_uploads();
        CHECK(ctx.asset_state(ponko::residency_kind::mesh, "models/nope.glb") == ponko::asset_state::failed);
        CHECK(ctx.asset_error(ponko::residency_kind::mesh, "models/nope.glb").find("nope.glb") != std::string::npos);
        CHECK(ctx.asset_error(ponko::residency_kind::mesh, "models/house.glb").empty());
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ctx.set_lod_group(e, 4, {ponko::lod_level{0.0f, 0, {ponko::lod_renderer_of(ponko::mesh_none)}, false, {}}});
        ctx.set_streamed_lod(e, {"models/house.glb"});
        throws([] { ponko::detail::check(ponko_ctx_set_streamed_lod(nullptr, 0, nullptr, 0)); });
    });
}

static void handle_lifecycle() {
    ponko::game("handles", 64, 64).run_headless(0, [](ponko::ctx& ctx) {
        auto before = ctx.live_counts();
        const std::uint8_t px[4] = {255, 255, 255, 255};
        auto t = ctx.create_texture(1, 1, px, sizeof px);
        auto m = ctx.create_material(ponko::material_of(ponko::colors::white));
        auto mesh = ctx.create_mesh(ponko::mesh_data::cube(1), m);
        auto target = ctx.create_render_target(8, 8);
        CHECK(ctx.is_live(t) && ctx.is_live(m) && ctx.is_live(mesh) && ctx.is_live(target));
        CHECK(ctx.live_counts().textures == before.textures + 2 && ctx.live_counts().render_targets == 1);
        ctx.free(t);
        ctx.free(m);
        ctx.free(mesh);
        ctx.free(target);
        ctx.free(t);  // again: nothing
        CHECK(!ctx.is_live(t) && !ctx.is_live(m) && !ctx.is_live(mesh) && !ctx.is_live(target));
        CHECK(!ctx.is_mesh_live(mesh));
        auto again = ctx.create_texture(1, 1, px, sizeof px);
        CHECK(again.id != t.id && ctx.is_live(again));
        ctx.free(again);
        CHECK(!ctx.is_live(ponko::none<ponko::texture>()) && !ctx.is_live(ponko::mesh_none));
        CHECK(ponko::none<ponko::mesh>().id == ponko::mesh_none.id);
        CHECK(ctx.is_live(ponko::font{}));  // the built-in font
        ctx.free(ponko::font{});
        CHECK(ctx.is_live(ponko::font{}));
        auto after = ctx.live_counts();
        CHECK(after.textures == before.textures && after.meshes == before.meshes && after.materials == before.materials);
        throws([] { ponko::detail::check(ponko_ctx_handle_free(nullptr, PONKO_HANDLE_MESH, 0)); });

        // Asset accounting: counts and bytes by class, each handle's age and owner.
        const auto textures = ctx.asset_count(ponko::asset_class::texture);
        auto pixel = ctx.create_texture(1, 1, px, sizeof px);
        CHECK(ctx.asset_count(ponko::asset_class::texture).live == textures.live + 1);
        CHECK(ctx.asset_count(ponko::asset_class::texture).bytes == textures.bytes + 4);
        bool listed = false;
        for (const auto& r : ctx.asset_handles(ponko::asset_class::texture)) {
            if (r.id == pixel.id) listed = r.owner == static_cast<std::uint32_t>(ponko::asset_owner::code) && r.bytes == 4;
        }
        CHECK(listed);
        CHECK(ctx.asset_owner_name(ponko::asset_class::texture, pixel.id).empty());
        ctx.free(pixel);
        auto house = ctx.load_mesh("models/house.glb");
        CHECK(ctx.asset_owner_name(ponko::asset_class::mesh, house.id) == "models/house.glb");
        CHECK(ctx.unload_mesh("models/house.glb") && !ctx.is_live(house));
        CHECK(!ctx.unload_mesh("models/house.glb"));
        throws([&] { ctx.asset_count(static_cast<ponko::asset_class>(99)); });
    });
}

static void streamed_worlds() {
    // Far ground, cut where a tile is shown.
    ponko::game("far", 64, 64).run_headless(0, [](ponko::ctx& ctx) {
        auto ground = ctx.default_material();
        ponko::far_terrain far(std::vector<float>(129 * 129, 1.0f), 129, {-1024, -1024}, 16, 128, ground);
        far.update(ctx);
        CHECK(far.mesh_count() == 4 && !far.is_cut({0, 0}));
        far.cut({0, 0});
        far.update(ctx);
        CHECK(far.is_cut({0, 0}) && far.mesh_count() == 4);
        far.uncut({0, 0});
        CHECK(!far.is_cut({0, 0}));
        far.clear(ctx);
        CHECK(far.mesh_count() == 0);
    });
    throws([] { ponko::far_terrain(std::vector<float>(16, 0.0f), 4, {5, 0}, 10, 10, ponko::material{}); });
    throws([] { ponko::collision_mesh_data::heightfield(std::vector<float>(8, 0.0f), 4); });
    throws([] { ponko::collision_mesh_data::convex_hull({{0, 0, 0}, {1, 0, 0}, {0, 1, 0}}); });
    // A ball on terrain built on another thread; a far wall that comes in
    // once the focus walks to it.
    ponko::entity ball = 0, wall = 0, focus = 0, ground = 0;
    bool wall_at_start = true, wall_later = false;
    auto w = ponko::game("t", 1, 1)
        .physics3d({0, -9.81f, 0})
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 0) {
                wall_at_start = ctx.is_collider_active(wall);
                ctx.world().set(focus, ponko::transform3d_at({290, 0, 0}));
            } else if (ctx.sim_tick() == 1) {
                wall_later = ctx.is_collider_active(wall);
            }
        })
        .run_headless(120, [&](ponko::ctx& ctx) {
            ctx.stream_colliders(8);
            auto data = std::async(std::launch::async, [] {
                return ponko::collision_mesh_data::heightfield(std::vector<float>(9, 2.0f), 20);
            });
            auto mesh = ctx.add_collision_mesh(data.get());
            auto world = ctx.world();
            ground = world.spawn(ponko::transform3d_at({-10, 0, -10}), ponko::solid_terrain(mesh));
            ball = world.spawn(ponko::transform3d_at({0, 4, 0}), ponko::dynamic_body3d(), ponko::sphere_collider(0.5f));
            wall = world.spawn(ponko::transform3d_at({300, 0, 0}), ponko::fixed_body3d(), ponko::box_collider({1, 1, 1}));
            focus = world.spawn(ponko::transform3d_at({0, 0, 0}), ponko::focus_colliders(50, 80));
        });
    CHECK(std::fabs(w.get<ponko::transform3d>(ball).position.y - 2.5f) < 0.05f);
    CHECK(!wall_at_start && wall_later);
    CHECK(w.get<ponko::terrain_collider>(ground).friction == 0.5f);
    CHECK(w.get<ponko::collider_focus>(focus).keep_radius == 80);
}

static void lod_groups() {
    float fov = 60 * 3.14159265f / 180;
    CHECK(std::abs(ponko::lod_screen_height_at(2, 10, fov) - 0.1732f) < 1e-3f);
    CHECK(std::abs(ponko::lod_distance_at(2, 0.1732f, fov) - 10) < 0.01f);
    ponko::game("lod", 64, 64).run_headless(0, [](ponko::ctx& ctx) {
        auto white = ctx.default_material();
        auto high = ctx.create_mesh(ponko::mesh_data::cube(1), white);
        auto low = ctx.create_mesh(ponko::mesh_data::cube(2), white);
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ponko::lod_level a{0.5f, 0, {ponko::lod_renderer_of(high)}};
        ponko::lod_level b{0.1f, 0, {ponko::lod_renderer_of(low)}, true, {}};
        ctx.set_lod_group(e, 2, {a, b});
        auto at = [&](float d) {
            auto cam = ctx.camera3d();
            cam.position = {0, 0, d};
            ctx.set_camera3d(cam);
            ctx.camera3d_look_at({0, 0, 0});
            return ctx.lod_pick_of(e);
        };
        CHECK(at(1).kind == 1 && at(1).level == 0);
        CHECK(at(5).kind == 1 && at(5).level == 1);
        CHECK(at(100).kind == 0);
        ctx.set_lod({20, true});
        CHECK(at(5).level == 0 && ctx.lod().bias == 20);
        throws([&] { ctx.set_lod({-1, true}); });
        throws([&] { ctx.lod_pick_of(ctx.world().spawn(ponko::transform3d_at({0, 0, 0}))); });
    });
}

static void skeletons() {
    ponko::game("skeletons", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto arm = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ponko::joint base{"base"}, tip{"tip", 0, ponko::transform3d_at({0, 1, 0})};
        ctx.set_skeleton(arm, {base, tip});
        CHECK(ctx.joint_count(arm) == 2);
        CHECK(ctx.find_joint(arm, "tip") == std::optional<std::size_t>(1));
        CHECK(!ctx.find_joint(arm, "tail"));
        // Bend the base a quarter turn: the tip swings from +Y to -X.
        auto pose = ctx.joint_pose(arm, 0);
        pose.rotation = ponko::quat_from_rotation_z(1.5707963f);
        ctx.set_joint_pose(arm, 0, pose);
        auto m = ctx.joint_matrix(arm, 1);
        CHECK(std::fabs(m[12] + 1) < 1e-5f && std::fabs(m[13]) < 1e-5f);
        ctx.reset_pose(arm);
        CHECK(std::fabs(ctx.joint_matrix(arm, 1)[13] - 1) < 1e-6f);
        throws([&] { ctx.joint_pose(arm, 2); });
        throws([&] { ctx.set_skeleton(arm, {ponko::joint{"loop", 0}}); });

        // A post: corners above the middle follow the tip.
        auto post = ponko::mesh_data::cube(0.2f);
        std::vector<std::array<std::uint16_t, 4>> joints;
        std::vector<std::array<float, 4>> weights;
        auto view = post.view();
        for (std::size_t i = 0; i < view.vertex_count; ++i) {
            joints.push_back({std::uint16_t(view.positions[i].y > 0 ? 1 : 0), 0, 0, 0});
            weights.push_back({1, 0, 0, 0});
        }
        post.set_skin(joints, weights);
        auto material = ctx.default_material();
        auto mesh = ctx.create_skinned_mesh(post, material, arm);
        ctx.world().spawn(ponko::model_of(mesh));
        auto hat = ctx.world().spawn(ponko::model_of(mesh));
        ctx.set_skin_of(hat, arm);
        auto cup = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ctx.attach_to_joint(cup, arm, 1, ponko::transform3d_at({0, 0.1f, 0}));
        throws([&] { ctx.create_skinned_mesh(ponko::mesh_data::cube(1), material, arm); });
        joints.assign(joints.size(), {7, 0, 0, 0});
        post.set_skin(joints, weights);
        throws([&] { ctx.create_skinned_mesh(post, material, arm); });
        throws([&] { post.set_skin({{0, 0, 0, 0}}, {{1, 0, 0, 0}}); });
        throws([&] { ctx.create_skinned_mesh(post, material, cup); });
        throws([&] { ctx.load_clip("no/such.glb", "walk"); });
    });
}

static void morph_targets() {
    ponko::game("morph targets", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        // Two targets on a plane: one dents the whole sheet, one tips a corner.
        auto sheet = ponko::mesh_data::plane(1.0);
        ponko::morph_target dent{"dent", std::vector<ponko::vec3>(4, {0, 0, -0.7f}), {}};
        sheet.add_morph_target(dent);
        ponko::morph_target tip{"tip", std::vector<ponko::vec3>(4), {}}; // normals stay flat
        tip.positions[0] = {0, 0.4f, 0};
        sheet.add_morph_target(tip);
        sheet.add_morph_frame(0, 1, 0.5f);
        sheet.add_morph_frame(0, 0, 1.0f);
        throws([&] { sheet.add_morph_frame(0, 0, 1.0f); });
        throws([&] { sheet.add_morph_target(ponko::morph_target{"short", std::vector<ponko::vec3>(3), {}}); });

        auto mesh = ctx.create_mesh(sheet, ctx.default_material());
        CHECK(ctx.morph_target(mesh, "dent") == std::optional<std::size_t>(0));
        CHECK(ctx.morph_target(mesh, "tip") == std::optional<std::size_t>(1));
        CHECK(!ctx.morph_target(mesh, "sag"));

        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}), ponko::model_of(mesh));
        CHECK(!ctx.morph_weight(e, 0)); // no weights: its mesh's defaults show
        ctx.set_morph_weight(e, 0, 0.5f);
        ctx.set_morph_weight(e, 1, 1.0f);
        CHECK(ctx.morph_weight(e, 0) == std::optional(0.5f));
        CHECK(ctx.morph_weight(e, 1) == std::optional(1.0f));
        CHECK(ctx.morph_weight(e, 2) == std::optional(0.0f));
        ctx.set_skeleton(e, {});
        ponko::clip face("face");
        face.set_morph_track("dent", {0, 2}, {0, 1});
        CHECK(face.duration() == 2);
        ctx.sample_clip(face, e, 1);
        CHECK(ctx.morph_weight(e, 0) == std::optional(0.5f));
        CHECK(ctx.morph_weight(e, 1) == std::optional(1.0f));
        throws([&] { face.set_morph_track("dent", {0, 1}, {0}); });
        ctx.write_animated_morphs();
        ctx.clear_morph_weights(e);
        CHECK(!ctx.morph_weight(e, 0));
        throws([&] { ctx.set_morph_weight(12345, 0, 0.5f); }); // no such entity
    });
}

static void sound3d() {
    ponko::game("sound3d", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        float samples[64] = {};
        auto hum = ctx.create_sound(44100, samples, 64);
        auto music = ctx.audio_bus("music");
        CHECK(music.index == ctx.audio_bus("music").index);
        CHECK(music.index != ponko::audio_bus{}.index);
        ctx.set_audio_bus_volume(music, 0.5f);
        ctx.mute_audio_bus(music, true);
        auto settings = ponko::audio_settings3d_defaults();
        CHECK(settings.max_distance == 100.0f && settings.doppler == 1.0f);
        settings.rolloff = PONKO_ROLLOFF_LINEAR;
        settings.bus = music;
        auto e = ctx.world().spawn(ponko::transform3d_at({4, 0, 0}));
        ctx.set_audio_source3d(e, hum, settings);
        ctx.remove_audio_source3d(e);
        auto voice = ctx.play_sound_at(hum, {-4, 0, 0}, settings);
        ctx.set_sound_effects(voice, {800.0f, 0.3f});
        ctx.set_audio_listener(ponko::audio_listener3d{{7, 2, 3}, {0, 0, 0, 1}});
        CHECK(ctx.audio_listener().position.x == 7.0f);
        ctx.set_audio_listener(std::nullopt);
        CHECK(ctx.audio_listener().position.x != 7.0f);
        throws([&] { ctx.load_stream("missing.ogg"); });
    });
}

// Writes the two-joint skin with the clip "rise" to the temp folder.
static std::filesystem::path write_rise_gltf(const char* name) {
    const auto file = std::filesystem::temp_directory_path() / name;
    std::ofstream(file) << R"({"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "root", "children": [1]}, {"name": "arm", "mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 2}}]}],
        "skins": [{"joints": [0, 1]}],
        "animations": [{"name": "rise", "samplers": [{"input": 0, "output": 1}],
            "channels": [{"sampler": 0, "target": {"node": 1, "path": "translation"}}]}],
        "buffers": [{"byteLength": 68, "uri": "data:application/octet-stream;base64,AAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAACAPwAAAAA="}],
        "bufferViews": [{"buffer": 0, "byteLength": 8}, {"buffer": 0, "byteOffset": 8, "byteLength": 24},
            {"buffer": 0, "byteOffset": 32, "byteLength": 36}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 2, "type": "SCALAR", "min": [0], "max": [1]},
            {"bufferView": 1, "componentType": 5126, "count": 2, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0, 0, 0], "max": [1, 1, 0]}]})";
    return file;
}

// A two-joint skin with one clip, "rise": the arm goes up 2 over 1 second.
static void animators() {
    const auto file = write_rise_gltf("ponko-cpp-rise.gltf");
    const std::string path = file.string();
    ponko::game("animators", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto rise = ctx.load_clip(path.c_str(), "rise");
        CHECK(rise.duration() == 1.0f);
        ponko::animation_graph_builder b;
        auto speed = b.add_float();
        auto still = b.add_state(ponko::motion::of(rise), 0);
        auto up = b.add_state(ponko::motion::of(rise), 1, std::nullopt, false);
        b.add_event(up, 0.5f, "half");
        auto go = b.add_transition(still, up, 0);
        b.add_condition(go, PONKO_CONDITION_GREATER, speed, 0.5f);
        auto tree = ponko::motion::blend1d(speed);
        tree.add(0, ponko::motion::of(rise)).add(1, ponko::motion::of(rise), 2);
        b.add_state(std::move(tree));
        auto graph = b.build(still);
        throws([&] { ponko::animation_graph_builder bad; bad.build(3); });

        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        throws([&] { ctx.set_animator(e, graph); }); // no skeleton yet
        ctx.load_skeleton(e, path.c_str());
        ctx.set_animator(e, graph);
        ctx.animator_set_manual(e, true);
        auto arm = *ctx.find_joint(e, "arm");
        ctx.animator_update(e, 0.1f);
        CHECK(ctx.animator_state(e) == still);
        ctx.animator_set_float(e, speed, 1);
        ctx.animator_update(e, 0.0f); // takes the transition
        CHECK(ctx.animator_state(e) == up);
        ctx.animator_update(e, 0.75f);
        CHECK(std::fabs(ctx.joint_pose(e, arm).position.y - 1.5f) < 1e-4f);
        CHECK(ctx.animator_normalized_time(e) == 0.75f);
        CHECK(ctx.animator_next_event(e) == std::optional<std::string>("half"));
        CHECK(!ctx.animator_next_event(e));
        throws_invalid([&] { ctx.animator_set_bool(e, speed, true); }, "not a bool"); // speed is a float
        throws_invalid([&] { ctx.animator_crossfade(e, 7, 0.1f); }, "no such animation state");
        throws_invalid([&] { ctx.animator_crossfade(e, still, -1.0f); }, "duration");
        ctx.animator_play(e, still);
        ctx.animator_update(e, 0.0f);
        CHECK(ctx.animator_state(e) == still);

        ctx.animator_set_float(e, speed, 0); // the base stays still
        ponko::animation_graph_builder lb;
        lb.add_state(ponko::motion::of(rise), 1, std::nullopt, false);
        auto overlay = lb.build(0);
        auto layer = ctx.animator_add_layer(e, overlay, ponko::layer_blend::override_pose, "arm");
        CHECK(layer == 0);
        ctx.animator_set_layer_weight(e, layer, 0.5f);
        ctx.animator_update(e, 0.5f); // layer: y = 1, base: y = 0
        CHECK(std::fabs(ctx.joint_pose(e, arm).position.y - 0.5f) < 1e-4f);
        throws([&] { ctx.animator_set_layer_weight(e, 1, 1); });
        throws([&] { ctx.animator_add_layer(e, overlay, ponko::layer_blend::additive, "tail"); });
        ctx.animator_set_root_motion(e, ponko::root_motion::extract, arm);
        ctx.animator_update(e, 0.1f);
        auto [moved, turned] = ctx.animator_root_motion(e);
        CHECK(moved.x == 0 && moved.z == 0 && turned.w == 1); // rising isn't horizontal
    });
    std::filesystem::remove(file);
}

// A ball on a 1 m ball joint below a fixed anchor swings but stays 1 m away.
static void joints3d() {
    ponko::entity ball = 0, joint = 0;
    std::size_t joints_seen = 0;
    float reach = 0, lowest = 5;
    ponko::game("joints", 1, 1)
        .physics3d({0, -9.81f, 0})
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 10) joints_seen = ctx.joint_count();
            if (ctx.sim_tick() < 60) lowest = std::min(lowest, ctx.world().get<ponko::transform3d>(ball).position.y);
            if (ctx.sim_tick() == 60) {
                auto p = ctx.world().get<ponko::transform3d>(ball).position;
                reach = std::sqrt(p.x * p.x + (p.y - 5) * (p.y - 5) + p.z * p.z);
                ctx.world().despawn(joint);
            }
        }, "probe")
        .run_headless(70, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            auto anchor = world.spawn(ponko::transform3d_at({0, 5, 0}), ponko::fixed_body3d(), ponko::sphere_collider(0.1f));
            ball = world.spawn(ponko::transform3d_at({1, 5, 0}), ponko::with_dominance(ponko::dynamic_body3d(), 3),
                               ponko::sphere_collider(0.2f));
            auto j = ponko::spherical_joint3d(anchor, ball);
            j.frame_b = ponko::transform3d_at({-1, 0, 0}); // the anchor, seen from the ball
            joint = world.spawn(j);
            CHECK(world.get<ponko::joint3d>(joint).axes[ponko::joint_axis::linear_x].kind == PONKO_JOINT_LOCKED);
            CHECK(world.get<ponko::rigid_body3d>(ball).dominance == 3);
            auto bad = ponko::with_limits(ponko::revolute_joint3d(anchor, ball), ponko::joint_axis::angular_x, 1, -1);
            throws([&] { world.spawn(bad); });
            auto motor = ponko::with_drive(ponko::revolute_joint3d(anchor, ball), ponko::joint_axis::angular_x, ponko::velocity_drive(2, 10));
            CHECK(motor.drives[ponko::joint_axis::angular_x].enabled && motor.drives[3].target_velocity == 2);
        });
    CHECK(joints_seen == 1);
    CHECK(std::fabs(reach - 1) < 0.05f);
    CHECK(lowest < 4.1f); // it swung down to about 1 m below the anchor
}

// A leg of two 1 m bones bends so its foot lands on a reachable target.
static void inverse_kinematics() {
    auto g = ponko::game("ik", 1, 1);
    g.late_system([](ponko::ctx&) {}, "after animation");
    g.run_headless(0, [&](ponko::ctx& ctx) {
        auto leg = ctx.world().spawn(ponko::transform3d_at({5, 0, 0}));
        ctx.set_skeleton(leg, {ponko::joint{"hip"}, ponko::joint{"knee", 0, ponko::transform3d_at({0, -1, 0})},
                               ponko::joint{"foot", 1, ponko::transform3d_at({0, -1, 0})}});
        ctx.two_bone_ik(leg, 0, 1, 2, {5.5f, -1.5f, 0}, {0, 0, 1});
        auto foot = ctx.joint_matrix(leg, 2); // model space: the leg stands at x = 5
        CHECK(std::fabs(foot[12] - 0.5f) < 1e-3f && std::fabs(foot[13] + 1.5f) < 1e-3f);
        // The hip's -Y axis (down the leg) aims at a point in front.
        ctx.reset_pose(leg);
        ctx.look_at_joint(leg, 0, {0, -1, 0}, {5, 0, 3});
        auto knee = ctx.joint_matrix(leg, 1);
        CHECK(std::fabs(knee[14] - 1) < 1e-3f && std::fabs(knee[13]) < 1e-3f);
        ctx.draw_skeleton(leg, ponko::colors::white);
        throws([&] { ctx.two_bone_ik(leg, 0, 1, 9, {0, 0, 0}, {0, 0, 1}); });
    });
}

// A stick figure ragdolled 1 m up falls and is posed by its bodies; stopping removes them.
static void ragdolls() {
    ponko::entity figure = 0;
    float hips_y = 0, rest_y = 0;
    ponko::game("ragdolls", 1, 1)
        .physics3d({0, -9.81f, 0})
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 60) {
                hips_y = ctx.joint_pose(figure, 0).position.y;
                ctx.stop_ragdoll(figure, 0.5f);
                CHECK(ctx.ragdolled(figure) && ctx.ragdoll_blending(figure));
                CHECK(!ctx.ragdoll_body(figure, 0));
            }
        }, "probe")
        .run_headless(62, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            world.spawn(ponko::transform3d_at({0, -0.5f, 0}), ponko::fixed_body3d(), ponko::box_collider({20, 1, 20}));
            figure = world.spawn(ponko::transform3d_at({0, 1, 0}));
            ctx.set_skeleton(figure, {ponko::joint{"hips"}, ponko::joint{"spine", 0, ponko::transform3d_at({0, 0.6f, 0})},
                                      ponko::joint{"head", 1, ponko::transform3d_at({0, 0.4f, 0})}});
            rest_y = ctx.joint_pose(figure, 0).position.y;
            std::size_t before = world.size();
            ctx.start_ragdoll(figure, {ponko::ragdoll_bone("hips", 0.12f), ponko::ragdoll_bone("spine", 0.1f).swing(0.5f).twist(0.2f)});
            CHECK(world.size() == before + 3); // two bodies and a joint
            auto spine = ctx.find_joint(figure, "spine");
            auto body = ctx.ragdoll_body(figure, *spine);
            CHECK(body && ctx.apply_impulse3d(*body, {1, 0, 0}));
            CHECK(!ctx.ragdoll_body(figure, 2)); // the head has no body of its own
            CHECK(ctx.ragdolled(figure) && !ctx.ragdoll_blending(figure));
            throws([&] { ctx.start_ragdoll(figure, {ponko::ragdoll_bone("tail", 0.1f)}); });
        });
    CHECK(hips_y < rest_y - 0.3f); // it fell
}

// One-way platforms pass things up through, belts carry them, and a hook can ignore a pair.
static void contacts3d() {
    auto floor = [](ponko::world_view world, ponko::vec3 at, ponko::contact_modifier3d m) {
        return world.spawn(ponko::transform3d_at(at), ponko::fixed_body3d(), ponko::box_collider({20, 0.4f, 20}), m);
    };
    auto ball = [](ponko::world_view world, ponko::vec3 at, ponko::vec3 linear) {
        return world.spawn(ponko::transform3d_at(at), ponko::dynamic_body3d(), ponko::sphere_collider(0.25f),
                           ponko::velocity3d{linear, {0, 0, 0}});
    };
    ponko::entity from_below = 0, blocked = 0, carried = 0, ghost = 0, standing = 0;
    std::size_t hook_calls = 0;
    bool pair_ok = true;
    auto world = ponko::game("contacts", 1, 1).physics3d({0, -9.81f, 0}).run_headless(150, [&](ponko::ctx& ctx) {
        auto w = ctx.world();
        // A one-way platform: a ball thrown up from below comes up through it.
        auto up = ponko::one_way_modifier3d({0, 1, 0});
        CHECK(up.has_one_way && up.one_way_angle == 1.2f);
        floor(w, {0, 0, 0}, up);
        from_below = ball(w, {0, -1, 0}, {0, 8, 0});
        // An ordinary one blocks the same throw.
        floor(w, {30, 0, 0}, ponko::default_contact_modifier3d());
        blocked = ball(w, {30, -1, 0}, {0, 8, 0});
        // A belt carrying things along +X at 2 m/s.
        auto belt = ponko::conveyor_modifier3d({2, 0, 0});
        CHECK(belt.surface_velocity.x == 2);
        floor(w, {60, 0, 0}, belt);
        carried = ball(w, {60, 0.5f, 0}, {0, 0, 0});
        // A hook: only `ghost` is let through the hooked floor.
        auto hooked = floor(w, {90, 0, 0}, ponko::hook_modifier3d());
        ghost = ball(w, {90, 1, 0}, {0, 0, 0});
        standing = ball(w, {93, 1, 0}, {0, 0, 0});
        ctx.set_contact_hook3d([&, hooked](ponko::contact_pair3d& pair) {
            ++hook_calls;
            pair_ok = pair_ok && (pair.a() == hooked || pair.b() == hooked);
            auto count = pair.point_count();
            pair_ok = pair_ok && !pair.point(count + 5);
            pair.set_friction(0.5f);
            pair.set_restitution(0.25f);
            pair_ok = pair_ok && pair.friction() == 0.5f && pair.restitution() == 0.25f;
            if (count > 0) {
                pair_ok = pair_ok && std::abs(pair.normal().y) > 0.99f;
                auto p = pair.point(0);
                pair_ok = pair_ok && p.has_value();
                if (p && p->position.x < 91.5f) {
                    pair.ignore();
                    pair_ok = pair_ok && pair.point_count() == 0;
                }
            }
            pair.remove_point(count + 5);
            pair.set_surface_velocity({0, 0, 0});
            throws([&] { pair.set_friction(std::nanf("")); });
        });
    });
    float rest = 0.2f + 0.25f;
    CHECK(std::abs(world.get<ponko::transform3d>(from_below).position.y - rest) < 0.05f);
    CHECK(world.get<ponko::transform3d>(blocked).position.y < -0.3f);
    CHECK(world.get<ponko::transform3d>(carried).position.x > 61);
    CHECK(hook_calls > 0 && pair_ok);
    CHECK(world.get<ponko::transform3d>(ghost).position.y < -1);
    CHECK(world.get<ponko::transform3d>(standing).position.y > 0.3f);

    // A cleared hook leaves the hooked floor ordinary.
    std::size_t calls_after_clear = 0;
    ponko::entity ball_on_floor = 0;
    auto cleared = ponko::game("contacts cleared", 1, 1).physics3d({0, -9.81f, 0}).run_headless(100, [&](ponko::ctx& ctx) {
        auto w = ctx.world();
        floor(w, {0, 0, 0}, ponko::hook_modifier3d());
        ball_on_floor = ball(w, {0, 1, 0}, {0, 0, 0});
        ctx.set_contact_hook3d([&](ponko::contact_pair3d& pair) { ++calls_after_clear; pair.ignore(); });
        ctx.clear_contact_hook3d();
    });
    CHECK(calls_after_clear == 0 && cleared.get<ponko::transform3d>(ball_on_floor).position.y > 0.3f);

    // An exception in the hook never unwinds into the engine: the run ends and rethrows it.
    bool caught = false;
    try {
        ponko::game("contacts throwing", 1, 1).physics3d({0, -9.81f, 0}).run_headless(100, [&](ponko::ctx& ctx) {
            auto w = ctx.world();
            floor(w, {0, 0, 0}, ponko::hook_modifier3d());
            ball(w, {0, 1, 0}, {0, 0, 0});
            ctx.set_contact_hook3d([](ponko::contact_pair3d&) { throw std::runtime_error("boom"); });
        });
    } catch (const std::runtime_error& e) { caught = std::string(e.what()) == "boom"; }
    CHECK(caught);

    // The modifier is an ordinary component: read it back, and it is no query column.
    ponko::world plain;
    auto m = ponko::one_way_modifier3d({0, 1, 0});
    m.hook = true;
    auto e = plain.spawn(m);
    CHECK(plain.get<ponko::contact_modifier3d>(e).hook);
    m.one_way_angle = -1;
    throws([&] { plain.set(e, m); });
}

// Two endpoints in memory: connect, then a reliable message and a broadcast.
static void messages() {
    throws([] { ponko::net_messages::steam_host(480, 0); });
    throws([] { ponko::net_messages::steam_join(480, 0); });
    throws([] { ponko::net_messages::steam_host(480, 2); }); // no Steam support/client in native test
    throws([] { ponko::steam_lobby::create(480, ponko::steam_lobby_visibility::private_lobby, 0); });
    throws([] { ponko::steam_lobby::create(480, ponko::steam_lobby_visibility::friends_only, 2); });
    throws([] { ponko::steam_lobby::join(480, 0); });
    auto peers = ponko::net_messages::loopback(2);
    auto& host = peers[0];
    auto& client = peers[1];
    std::vector<ponko::message_event> got;
    for (int i = 0; i < 3000 && got.size() < 4; i++) { // the handshake takes real time
        std::this_thread::sleep_for(std::chrono::milliseconds(1));
        host.poll();
        client.poll();
        while (auto e = host.next_event()) got.push_back(*e);
        while (auto e = client.next_event()) {
            if (e->kind == PONKO_MESSAGE_CONNECTED) {
                client.send(0, PONKO_LANE_RELIABLE, std::string("buy apple"));
                host.send_to_all(PONKO_LANE_UNRELIABLE, std::string("tick"));
            }
            got.push_back(*e);
        }
    }
    auto has = [&](PonkoMessageEventKind kind, const std::string& text) {
        for (auto& e : got)
            if (e.kind == kind && std::string(e.bytes.begin(), e.bytes.end()) == text) return true;
        return false;
    };
    CHECK(has(PONKO_MESSAGE_CONNECTED, ""));
    CHECK(has(PONKO_MESSAGE, "buy apple"));
    CHECK(has(PONKO_MESSAGE, "tick"));
    CHECK(host.stats().packets_sent > 0);
    throws([&] { client.send(0, PONKO_LANE_RELIABLE, nullptr, 2 * 1024 * 1024); });
    throws([] { ponko::net_messages::loopback(1); });
}

static void navigation_carving() {
    ponko::entity box = 0;
    int tick = 0;
    bool cut = false, restored = false;
    ponko::game("carve", 1, 1).navmesh3d()
        .sim_system([&](ponko::ctx& ctx) {
            ++tick;
            auto state = ctx.navmesh_update_carving(1);
            CHECK(state.first <= 1);
            if (tick == 20) {
                CHECK(!state.second);
                auto path = ctx.navmesh_find_path({-7, 0, 0}, {7, 0, 0});
                CHECK(path.size() >= 3);
                CHECK(!ctx.navmesh_sample_position({0, 0, 0}, 0.2f));
                cut = true;
                ctx.remove_nav(box);
            }
            if (tick == 40) {
                CHECK(!state.second);
                auto path = ctx.navmesh_find_path({-7, 0, 0}, {7, 0, 0});
                CHECK(path.size() == 2);
                restored = true;
            }
        }).run_headless(45, [&](ponko::ctx& ctx) {
            ponko::nav_sources sources;
            sources.add_mesh(ponko::mesh_data::plane(20), ponko::transform3d_at({0, 0, 0}));
            auto settings = ponko::nav_bake_defaults();
            settings.carving = true;
            ctx.bake_navmesh(sources, settings);
            box = ctx.world().spawn();
            ctx.set_nav_carve_obstacle(box, {0, 1, 0}, {0.5f, 1, 2});
        });
    CHECK(cut && restored);
}

// A 20 m floor with a wall across the middle, open at one end: the agent walks around.
static void navigation() {
    ponko::entity walker = 0;
    std::size_t corners = 0;
    auto w = ponko::game("nav", 1, 1).navmesh3d().run_headless(600, [&](ponko::ctx& ctx) {
        ponko::nav_sources sources;
        sources.add_mesh(ponko::mesh_data::plane(20), ponko::transform3d_at({0, 0, 0}));
        auto wall = ponko::mesh_data::cube(1);
        auto t = ponko::transform3d_at({0, 1, -2});
        t.scale = {1, 2, 16}; // x = -0.5..0.5, z = -10..6: open beyond z = 6
        sources.add_mesh(wall, t);
        ctx.bake_navmesh(sources);
        auto snapped = ctx.navmesh_sample_position({-5, 3, 0}, 5);
        CHECK(snapped && std::fabs(snapped->y) < 0.3f);
        auto path = ctx.navmesh_find_path({-5, 0, 0}, {5, 0, 0});
        auto status = ctx.navmesh_update_carving(0);
        CHECK(status.first == 0 && !status.second);
        ctx.set_nav_carve_obstacle(ctx.world().spawn(), {8, 1, 8}, {0.5f, 1, 0.5f});
        corners = path.size();
        CHECK(path.size() >= 3); // around the wall's end
        walker = ctx.world().spawn(ponko::transform3d_at({-5, 0, 0}));
        ctx.set_nav_agent(walker, 4);
        ctx.set_nav_avoidance(walker);
        throws([&] { ctx.set_nav_avoidance(walker, true, -1); });
        throws([&] { ctx.set_nav_avoidance(walker, true, 2, 0); });
        throws([&] { ctx.set_nav_avoidance(ctx.world().spawn()); });
        ctx.set_nav_destination(walker, ponko::vec3{5, 0, 0});
        throws([&] { ctx.set_nav_agent(walker, -1); });
        ponko::nav_sources empty;
        throws([&] { ctx.bake_navmesh(empty); });
    });
    auto end = w.get<ponko::transform3d>(walker).position;
    CHECK(corners >= 3);
    CHECK(std::fabs(end.x - 5) < 0.3f && std::fabs(end.z) < 0.3f);
}

static void particles3d() {
    ponko::game("particles", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 1, 0}));
        auto fire = ponko::particle_settings()
                        .rate(40)
                        .cone(0.3f)
                        .lifetime(0.5f, 1)
                        .size_over_life({{0, 1}, {1, 0}})
                        .color_over_life({{0, {1, 0.6f, 0.1f, 1}}, {1, {1, 0, 0, 0}}})
                        .blend(ponko::particle_blend::additive)
                        .max_particles(256);
        ctx.set_particle_emitter3d(e, fire);
        ctx.particle_burst(e, 20);
        // Particles simulate in the visual frame phase, which headless runs skip.
        CHECK(ctx.particle_live_count(e) == 0);
        throws([&] { ctx.set_particle_emitter3d(e, ponko::particle_settings().lifetime(0, 1)); });
        throws([&] { ctx.set_particle_emitter3d(e, ponko::particle_settings().lifetime(2, 1)); });
        throws([&] { ctx.set_particle_emitter3d(e, ponko::particle_settings().max_particles(0)); });
        ctx.remove_particle_emitter3d(e);
        throws([&] { ctx.particle_burst(e, 1); });
    });
}

static void particles2d() {
    ponko::game("particles 2d", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto e = ctx.world().spawn(ponko::transform_at({100, 100}));
        auto sparks = ponko::particle_settings2d()
                          .rate(0)
                          .burst(12)
                          .circle(4)
                          .lifetime(1, 2)
                          .size_over_life({{0, 1}, {1, 0}})
                          .color_over_life({{0, {1, 0.6f, 0.1f, 1}}, {1, {1, 0, 0, 0}}})
                          .blend(ponko::sprite_blend::additive)
                          .z(3)
                          .max_particles(64);
        ctx.set_particle_emitter2d(e, sparks);
        // Headless runs skip the frame phase; step the emitter by hand.
        CHECK(ctx.particle_live_count2d(e) == 0);
        ctx.particle_update2d(e, 0);
        CHECK(ctx.particle_live_count2d(e) == 12);
        ctx.particle_burst2d(e, 100);
        ctx.particle_update2d(e, 0.1f);
        CHECK(ctx.particle_live_count2d(e) == 64);
        throws([&] { ctx.set_particle_emitter2d(e, ponko::particle_settings2d().lifetime(0, 1)); });
        throws([&] { ctx.set_particle_emitter2d(e, ponko::particle_settings2d().size(2, 1)); });
        throws([&] { ctx.set_particle_emitter2d(e, ponko::particle_settings2d().max_particles(0)); });
        ctx.remove_particle_emitter2d(e);
        throws([&] { ctx.particle_burst2d(e, 1); });
    });
}

// A 4 x 4 m floor facing up, as a glTF file.
static void ribbons_and_decals() {
    const auto file = std::filesystem::temp_directory_path() / "ponko-cpp-floor.gltf";
    std::ofstream(file) << R"({"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
        "buffers": [{"byteLength": 60, "uri": "data:application/octet-stream;base64,AAAAwAAAAAAAAABAAAAAQAAAAAAAAABAAAAAQAAAAAAAAADAAAAAwAAAAAAAAADAAAABAAIAAAACAAMA"}],
        "bufferViews": [{"buffer": 0, "byteLength": 48}, {"buffer": 0, "byteOffset": 48, "byteLength": 12}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [-2, 0, -2], "max": [2, 0, 2]},
            {"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"}]})";
    const std::string path = file.string();
    // The box's +Z turned up, 0.5 m wide and 0.2 m deep, at (1, 0, 0).
    const std::array<float, 16> down = {0.5f, 0, 0, 0,   0, 0, -0.2f, 0,   0, 0.5f, 0, 0,   1, 0, 0, 1};
    ponko::game("ribbons", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 1, 0}));
        auto tracer = ponko::trail3d(0.25f, 0.05f)
                          .width_over_age({{0, 0.1f}, {1, 0}})
                          .color_over_age({{0, {1, 0.8f, 0.3f, 1}}, {1, {1, 0.3f, 0, 0}}})
                          .blend(ponko::particle_blend::additive)
                          .min_distance(0.1f)
                          .max_points(32);
        ctx.set_trail3d(e, tracer);
        // Trails record in the visual frame phase, which headless runs skip.
        CHECK(ctx.trail3d_point_count(e) == 0);
        ctx.set_trail3d_emitting(e, false);
        ctx.clear_trail3d(e);
        throws([&] { ctx.set_trail3d(e, ponko::trail3d(0, 0.1f)); });
        throws([&] { ctx.set_trail3d(e, ponko::trail3d(1, -0.1f)); });
        throws([&] { ctx.set_trail3d(e, ponko::trail3d(1, 0.1f).max_points(1)); });
        throws([&] { ctx.set_trail3d(e, ponko::trail3d(1, 0.1f).width_over_age({{0, -1}})); });
        ctx.remove_trail3d(e);
        throws([&] { ctx.trail3d_point_count(e); });
        throws([&] { ctx.clear_trail3d(e); });

        auto beam = ponko::line3d({{0, 0, 0}, {0, 0, -20}}, 0.02f, ponko::colors::red)
                        .blend(ponko::particle_blend::additive)
                        .texture_length(2);
        ctx.set_line3d(e, beam);
        ctx.set_line3d(e, ponko::line3d({{0, 0, 0}, {1, 0, 0}, {1, 1, 0}}, 0.1f, ponko::colors::white)); // replaces it
        throws([&] { ctx.set_line3d(e, ponko::line3d({{0, 0, 0}, {1, 0, 0}}, -1, ponko::colors::white)); });
        throws([&] { ctx.set_line3d(e, ponko::line3d({{0, 0, 0}, {1, NAN, 0}}, 0.1f, ponko::colors::white)); });
        ctx.remove_line3d(e);

        auto floor = ctx.load_mesh_data(path.c_str());
        CHECK(floor.view().vertex_count == 4 && floor.view().index_count == 6);
        auto mark = floor.decal(down, 0.002f);
        auto view = mark.view();
        CHECK(view.index_count >= 3 && view.index_count % 3 == 0);
        for (std::size_t i = 0; i < view.vertex_count; i++) {
            CHECK(std::fabs(view.positions[i].x - 1) <= 0.26f && std::fabs(view.positions[i].z) <= 0.26f);
            CHECK(std::fabs(view.positions[i].y - 0.002f) < 1e-4f);
        }
        // A box that misses gives an empty decal; a broken projector throws.
        auto away = down;
        away[12] = 50;
        CHECK(floor.decal(away, 0.002f).view().index_count == 0);
        auto broken = down;
        broken[0] = NAN;
        throws([&] { floor.decal(broken, 0.002f); });
        throws([&] { ctx.load_mesh_data("no/such.glb"); });
    });
    std::filesystem::remove(file);
}

static void ui_panels() {
    ponko::game("ui panels", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        namespace u = ponko::ui;
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 1.5f, -2}));
        auto texture = ctx.create_ui_panel(e, 600, 400, {1.2f, 0.8f});
        CHECK(texture.width == 600 && texture.height == 400);
        CHECK(ctx.world().has<ponko::model>(e));

        // The panel's UI takes the same calls as the screen's, in the panel's pixels.
        auto panel = ctx.panel_ui(e);
        auto root = panel.add_panel(u::none, u::layout().column().size(u::percent(1), u::percent(1)).padding(20));
        auto buy = panel.add_button(root, "Buy", u::layout().size(200, 60).shrink(0));
        panel.push_screen(root);
        panel.layout_for({600, 400});
        CHECK(panel.top_screen() == root && panel.rect(buy).max.x == 220 && panel.rect(root).max.y == 400);
        CHECK(!ctx.ui_retained().top_screen());  // the screen's own UI is a different tree

        auto input = u::default_input();
        input.pointer = {50, 50};
        input.pressed = input.held = true;
        panel.frame({600, 400}, input);
        input.pressed = input.held = false;
        input.released = true;
        panel.frame({600, 400}, input);
        auto events = panel.events();
        CHECK(events.size() == 1 && events[0].kind == u::event_kind::clicked && events[0].node == buy);
        CHECK(ctx.ui_retained().events().empty());

        // Headless runs don't give panels the mouse; the calls still work.
        ctx.set_ui_pointer_ray({0, 1.5f, 0}, {0, 0, -1});
        CHECK(!ctx.ui_panel_pointer(e));
        ctx.set_ui_panel_interactive(e, false);
        ctx.clear_ui_pointer_ray();
        throws([&] { ctx.set_ui_pointer_ray({0, 0, 0}, {0, 0, 0}); });
        throws([&] { ctx.create_ui_panel(e, 0, 10, {1, 1}); });
        throws([&] { ctx.create_ui_panel(e, 10, 10, {-1, 1}); });
        ctx.remove_ui_panel(e);
        throws([&] { panel.add_panel(u::none); });  // no panel on it now
        throws([&] { ctx.ui_panel_pointer(e); });
    });
}

// Layout presets, the theme, titles, names and take_click.
static void retained_ui_presets() {
    ponko::game("retained ui presets", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        namespace u = ponko::ui;
        auto ui = ctx.ui_retained();
        const ponko::vec2 window{800, 600};
        auto root = ui.add_panel(u::none, u::fill().padding(10).gap(5));
        auto title = ui.add_title(root, "Shop", u::row(40));
        auto bar = ui.add_panel(root, u::row(50).gap(10));
        auto side = ui.add_panel(bar, u::column(120));
        auto buy = ui.add_button(bar, "Buy", u::fixed(100, 30));
        auto map = ui.add_panel(root, u::anchored(0.75f, 0, 0.25f, 0.25f));
        ui.push_screen(root);
        auto dialog = ui.add_panel(u::none, u::centered(200, 100).top(40));
        ui.push_screen(dialog, true);
        ui.layout_for(window);
        CHECK(ui.rect(title).min.y == 10 && ui.rect(title).max.x == 790);
        CHECK(ui.rect(bar).min.y == 55 && ui.rect(side).max.x == 130 && ui.rect(side).max.y == 105);
        CHECK(ui.rect(buy).min.x == 140 && ui.rect(buy).max.x == 240);
        CHECK(ui.rect(map).min.x == 600 && ui.rect(map).max.y == 150);
        CHECK(ui.rect(dialog).min.x == 300 && ui.rect(dialog).min.y == 40);
        CHECK(ui.pop_screen() == dialog);

        // One theme for the tree and the immediate widgets.
        auto theme = ui.theme();
        CHECK(theme.title_size > theme.text_size && ui.resolved_paint(title).font_size == theme.title_size);
        theme.accent = {0, 1, 0, 1};
        theme.title = {1, 1, 0, 1};
        theme.title_size = 30;
        ui.set_theme(theme);
        CHECK(ui.theme().title_size == 30 && ui.resolved_paint(title).font_size == 30);
        CHECK(ui.resolved_paint(buy).border.g == 1 && ctx.ui_style().accent.g == 1);
        theme.border_width = -1;
        throws([&] { ui.set_theme(theme); });

        // Names instead of a list of ids.
        ui.set_name(buy, "shop.buy");
        CHECK(ui.find("shop.buy") == buy && ui.name(buy) == "shop.buy" && !ui.find("shop.sell"));
        ui.set_name(side, "shop.buy");  // moves the name
        CHECK(ui.find("shop.buy") == side && ui.name(buy).empty());

        // take_click: the last click's node, once.
        auto input = u::default_input();
        input.pointer = {190, 70};
        input.pressed = input.held = true;
        ui.frame(window, input);
        input.pressed = input.held = false;
        input.released = true;
        ui.frame(window, input);
        CHECK(ui.take_click() == buy && !ui.take_click());
        ui.remove(root);
        CHECK(!ui.find("shop.buy"));
    });
}

// What a screen reader reads of a UI, what changed, and what it can ask.
static void retained_ui_access() {
    ponko::game("retained ui access", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        namespace u = ponko::ui;
        auto ui = ctx.ui_retained();
        const ponko::vec2 window{800, 600};
        auto root = ui.add_panel(u::none, u::fill());
        auto play = ui.add_button(root, "Play", u::row(30));
        auto volume = ui.add_slider(root, 0.5f, u::row(30));
        auto name = ui.add_text_input(root, "Ann", u::row(30));
        auto music = ui.add_toggle(root, "Music", true, u::row(30));
        ui.push_screen(root);

        // Names, descriptions and live regions.
        CHECK(ui.accessible_name(play) == "Play" && ui.accessible_name(volume).empty());
        ui.set_accessible_name(volume, "Volume");
        ui.set_accessible_description(play, "Starts a run");
        ui.set_live_region(play, u::live::polite);
        CHECK(ui.accessible_name(volume) == "Volume" && ui.accessible_description(play) == "Starts a run");
        CHECK(ui.live_region(play) == u::live::polite && ui.live_region(volume) == u::live::off);
        ui.frame(window);
        CHECK(ui.access_text().rfind("group\n  button \"Play\" description=\"Starts a run\" live=polite focusable\n", 0) == 0);

        // The snapshot: everything at first, then nothing, then what changed.
        u::access access;
        auto first = access.update(ui);
        CHECK(first.any && first.screens_changed && first.changed == std::vector<u::id>({root, play, volume, name, music}));
        CHECK(access.size() == 5 && access.at(0).id == root && access.children(root).size() == 4);
        auto slider = access.find(volume);
        CHECK(slider && slider->role == PONKO_UI_ROLE_SLIDER && slider->value == 0.5f && slider->focusable);
        CHECK(access.name(volume) == "Volume" && access.description(play) == "Starts a run");
        CHECK(access.value_text(name) == "Ann" && access.find(name)->has_selection && !access.focus());
        CHECK(access.find(music)->value_kind == PONKO_UI_ACCESS_VALUE_CHECKED && access.find(music)->value == 1.0f);
        CHECK(!access.update(ui).any);

        // Requests do what keys do.
        ui.access_action(u::access_action_kind::activate, play);
        ui.access_action(u::access_action_kind::decrement, volume);
        ui.access_action(u::access_action_kind::set_value, music, 0.0f);
        ui.access_action(u::access_action_kind::set_text, name, 0.0f, "Bea");
        ui.frame(window);
        auto events = ui.events();
        CHECK(events.size() == 4 && events[0].kind == u::event_kind::clicked && events[0].node == play);
        CHECK(ui.focus() == play && ui.text(name) == "Bea" && ui.value(music) == 0.0f);
        auto after = access.update(ui);
        CHECK(after.any && after.focus_changed && after.changed.size() == 4 && after.removed.empty());
        CHECK(access.focus() == play && access.find(play)->focused && access.value_text(name) == "Bea");

        // A modal dialog covers the menu; a reset reports everything again.
        auto dialog = ui.add_panel(u::none, u::centered(200, 100));
        ui.push_screen(dialog, true);
        ui.frame(window);
        auto covered = access.update(ui);
        CHECK(covered.screens_changed && covered.removed.size() == 5 && covered.changed == std::vector<u::id>({dialog}));
        CHECK(!access.find(play) && access.find(dialog)->modal && access.find(dialog)->role == PONKO_UI_ROLE_DIALOG);
        throws([&] { access.name(play); });  // not exposed
        access.reset();
        CHECK(access.update(ui).changed.size() == 1);
        throws([&] { ui.set_accessible_name(999999, "x"); });
        u::access moved = std::move(access);
        CHECK(moved.size() == 1);
    });
}

static void retained_ui() {
    ponko::game("retained ui", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto ui = ctx.ui_retained();
        const ponko::vec2 window{800, 600};
        namespace u = ponko::ui;
        auto root = ui.add_panel(u::none, u::layout().column().size(400, 300).padding(10).gap(5));
        auto label = ui.add_label(root, "Title", u::layout().size(100, 20).shrink(0));
        auto go = ui.add_button(root, "Go", u::layout().size(200, 40).shrink(0));
        auto gift = ui.add_toggle(root, "Gift", false, u::layout().size(100, 20).shrink(0));
        auto volume = ui.add_slider(root, 0.25f, u::layout().size(200, 20).shrink(0));
        auto note = ui.add_text_input(root, "hi", u::layout().size(200, 30).shrink(0));
        CHECK(!ui.top_screen() && !ui.wants_keyboard());
        ui.push_screen(root);
        CHECK(ui.top_screen() == root && ui.wants_keyboard());
        ui.layout_for(window);
        CHECK(ui.rect(go).min.y == 35 && ui.rect(go).max.x == 210);
        CHECK(ui.text(label) == "Title" && ui.value(volume) == 0.25f);
        CHECK(ui.hit_test({50, 50}) == go && !ui.hit_test({700, 500}));

        // A click is a press, then a release over the same button.
        auto input = u::default_input();
        input.pointer = {50, 50};
        input.pressed = input.held = true;
        ui.frame(window, input);
        input.pressed = input.held = false;
        input.released = true;
        ui.frame(window, input);
        auto events = ui.events();
        CHECK(events.size() == 1 && events[0].kind == u::event_kind::clicked && events[0].node == go);
        CHECK(!ui.next_event() && ui.focus() == go);

        // Text, values and enabled state.
        ui.set_text(go, "Buy");
        CHECK(ui.text(go) == "Buy");
        ui.set_value(gift, 1);
        ui.set_value(volume, 9);
        CHECK(ui.value(gift) == 1 && ui.value(volume) == 1);
        ui.set_enabled(go, false);
        input.pointer = {50, 50};
        input.pressed = input.held = true; input.released = false;
        ui.frame(window, input);
        input.pressed = input.held = false; input.released = true;
        ui.frame(window, input);
        CHECK(ui.events().empty());

        // Typing into the input after focusing it.
        input = u::default_input();
        input.pointer = {205, 145};
        input.pressed = input.held = true;
        ui.frame(window, input);
        input = u::default_input();
        input.pointer = {205, 145};
        input.released = true;
        ui.frame(window, input);
        input = u::default_input();
        input.text = "!?";
        ui.frame(window, input);
        CHECK(ui.text(note) == "hi!?");
        events = ui.events();
        CHECK(events.size() == 1 && events[0].kind == u::event_kind::changed && events[0].node == note);

        // Modal screens stack, and Cancel pops the top one.
        auto dialog = ui.add_panel(u::none, u::layout().size(100, 100));
        ui.push_screen(dialog, true);
        CHECK(ui.top_screen() == dialog && ui.wants_pointer());
        input = u::default_input();
        input.cancel = true;
        ui.frame(window, input);
        events = ui.events();
        CHECK(events.size() == 1 && events[0].kind == u::event_kind::screen_popped && events[0].node == dialog);
        CHECK(ui.top_screen() == root);
        ui.push_screen(dialog);
        CHECK(ui.pop_screen() == dialog);
        throws([&] { ui.push_screen(go); });    // it has a parent
        throws([&] { ui.push_screen(root); });  // already a screen

        // Grid tracks, spans and absolute positions.
        auto grid = ui.add_panel(u::none, u::layout().grid({u::fr(1), u::fr(1), u::fr(1)}).auto_rows({u::track_points(40)}).size(300, 200));
        auto cell = ui.add_panel(grid);
        auto wide = ui.add_panel(grid, u::layout().grid_column(u::grid_auto(), u::span(2)));
        auto corner = ui.add_panel(u::none, u::layout().position(u::position::relative).size(200, 100));
        auto pinned = ui.add_panel(corner, u::layout().absolute().size(u::percent(0.5f), u::points(30))
                                               .inset(u::points(20), u::automatic(), u::automatic(), u::points(10)));
        for (auto screen : {grid, corner}) ui.push_screen(screen);
        ui.layout_for(window);
        CHECK(ui.rect(cell).max.x == 100);
        CHECK(ui.rect(wide).min.x == 100 && ui.rect(wide).max.x == 300);  // the next two columns
        CHECK(ui.rect(pinned).min.x == 10 && ui.rect(pinned).min.y == 20 && ui.rect(pinned).max.x == 110);

        // Named areas, owned builders, repeats and named line setters.
        auto named_layout = u::layout().grid({u::repeat(2, u::fr(1))})
            .grid_rows({u::track_points(40), u::track_points(160)})
            .areas({"header header", "side main"}).size(300, 200);
        auto copied = named_layout;
        auto named = ui.add_panel(u::none, copied);
        auto main = ui.add_panel(named, u::layout().area(std::string("main")));
        ui.push_screen(named);
        ui.layout_for(window);
        CHECK(ui.rect(main).min.x == 150 && ui.rect(main).min.y == 40);
        CHECK(ui.rect(main).max.x == 300 && ui.rect(main).max.y == 200);
        throws([&] { ui.set_grid_template_areas(named, {"x x", "x y"}); });
        ui.set_grid_line_names(named, 0, {"left", "right"});
        ui.set_grid_named_line(main, 0, false, "left", 1);
        ui.set_grid_named_line(main, 0, true, "right", 1);
        ui.layout_for(window);
        CHECK(ui.rect(main).min.x == 0 && ui.rect(main).max.x == 300);
        auto repeated_names = ui.add_panel(u::none, u::layout()
            .grid({u::repeat(2, u::fr(1)).names({"begin", "finish"})}).size(300, 100));
        auto repeated_item = ui.add_panel(repeated_names);
        ui.set_grid_named_line(repeated_item, 0, false, "begin", 2);
        ui.set_grid_named_line(repeated_item, 0, true, "finish", -1);
        ui.push_screen(repeated_names);
        ui.layout_for(window);
        CHECK(ui.rect(repeated_item).min.x == 150 && ui.rect(repeated_item).max.x == 300);
        throws([&] { ui.add_panel(u::none, u::layout().grid({u::repeat(2, u::fr(1)).names({"bad"})})); });
        // A named span: from line 1 to the second line called "edge".
        auto spans = ui.add_panel(u::none, u::layout().grid({u::track_points(100), u::track_points(100),
            u::track_points(100), u::track_points(100)}).size(400, 100));
        ui.set_grid_line_names(spans, 0, {"", "", "edge", "", "edge"});
        auto spanned = ui.add_panel(spans, u::layout().grid_column(u::line(1)));
        ui.set_grid_named_span(spanned, 0, true, "edge", 2);
        throws([&] { ui.set_grid_named_span(spanned, 0, true, "edge", 0); });
        ui.push_screen(spans);
        ui.layout_for(window);
        CHECK(ui.rect(spanned).min.x == 0 && ui.rect(spanned).max.x == 400);
        ui.set_grid_named_span(spanned, 0, true, "edge");
        ui.layout_for(window);
        CHECK(ui.rect(spanned).max.x == 200);
        ui.set_grid_area(main, "main");
        ui.layout_for(window);
        CHECK(ui.rect(main).min.x == 150);
        for (auto component : {u::auto_fill(u::track_points(100)), u::auto_fit({u::track_points(100)})}) {
            auto repeated = ui.add_panel(u::none, u::layout().grid({component}).size(300, 100));
            auto item = ui.add_panel(repeated);
            ui.push_screen(repeated);
            ui.layout_for(window);
            CHECK(ui.rect(item).max.x == 100);
        }
        throws([&] { ui.add_panel(u::none, u::layout().grid({u::repeat(0, u::fr(1))})); });
        throws([&] { ui.add_panel(u::none, u::layout().grid({u::auto_fill(u::fr(1))})); });

        // Paint, scrolling and scale.
        ui.set_paint(go, u::paint().background({1, 0, 0, 1}).border({1, 1, 1, 1}, 2).font_size(24).wrap().align(u::text_align::center));
        auto scroll = ui.add_scroll(u::none, u::layout().size(100, 100));
        ui.add_panel(scroll, u::layout().size(100, 400).shrink(0));
        ui.push_screen(scroll);
        ui.layout_for(window);
        ui.set_scroll(scroll, {0, 9999});
        ui.layout_for(window);
        CHECK(ui.scroll(scroll).y == 300);
        ui.set_scale(2);
        ui.layout_for(window);
        CHECK(ui.rect(scroll).max.x == 200);
        throws([&] { ui.set_paint(go, u::paint().font({99}, 16)); });

        // Removing a node takes its children, and its ids stay invalid.
        ui.remove(root);
        for (auto gone : {root, go, note}) {
            throws([&] { (void)ui.rect(gone); });
            throws([&] { ui.set_text(gone, "x"); });
        }
        throws([&] { ui.add_panel(root); });
        throws([&] { ui.add_panel(u::none, u::layout().display(static_cast<u::display>(99))); });
    });
}

static void world_text() {
    ponko::game("text", 1, 1).run_headless(0, [](ponko::ctx& c) {
        auto e = c.world().spawn();
        CHECK(!c.text3d(e));
        ponko::text3d t("Hello\nworld");
        t.size = .5f; t.align = ponko::text_align3d::left;
        t.billboard = true; t.outline_width = .06f;
        c.set_text3d(e, t);
        auto got = c.text3d(e);
        CHECK(got && got->text == t.text && got->size == .5f && got->billboard);
        CHECK(got->align == t.align && got->outline_width == .06f);
        t.text = "Changed"; c.set_text3d(e, t);
        CHECK(c.text3d(e)->text == "Changed");
        t.font.id = UINT32_MAX;
        throws([&] { c.set_text3d(e, t); });
        CHECK(c.text3d(e)->text == "Changed");
        t.font.id = 0; t.align = static_cast<ponko::text_align3d>(99);
        throws([&] { c.set_text3d(e, t); });
        c.remove_text3d(e); CHECK(!c.text3d(e));
        c.world().despawn(e);
        throws([&] { (void)c.text3d(e); });
    });
}
// Replication helpers: sequence numbers, authority, snapshots and interest.
static void replication() {
    CHECK(ponko::seq_newer(0, 65535) && !ponko::seq_newer(5, 5) && ponko::seq_diff(1, 65535) == 2);
    CHECK(ponko::stamp_newer({1, 0}, {0, 60000}) && !ponko::stamp_newer({0, 7}, {0, 7}));

    ponko::authority car(0);
    CHECK(car.check(0, 0) == ponko::authority_check::accept);
    CHECK(car.handoff(1) == 1 && car.owner() == 1 && car.epoch() == 1);
    CHECK(car.check(0, 0) == ponko::authority_check::stale);
    CHECK(car.check(0, 1) == ponko::authority_check::wrong_owner);
    CHECK(car.check(1, 2) == ponko::authority_check::ahead);
    CHECK(!car.adopt(2, 1) && car.owner() == 1); // simultaneous handoff: the lower id wins
    CHECK(car.adopt(2, 2) && car.owner() == 2);

    ponko::latest_wins filter;
    CHECK(filter.accept(5, std::uint16_t(3)) && !filter.accept(5, std::uint16_t(2)) && !filter.accept(5, std::uint16_t(3)));
    CHECK(filter.accept(5, ponko::update_stamp{1, 0})); // a new epoch beats any sequence number
    CHECK(filter.size() == 1);
    filter.forget(5);
    CHECK(filter.accept(5, std::uint16_t(1)) && filter.size() == 1);
    auto moved = std::move(filter);
    moved.clear();
    CHECK(moved.size() == 0);

    ponko::snapshot_stream stream;
    CHECK(!stream.sample(0) && !stream.clock_offset());
    for (int n = 0; n < 20; n++) {
        auto p = ponko::moving(ponko::pose_at({float(n), 0, 0}), {20, 0, 0});
        CHECK(stream.push(5000 + n * 0.05, n * 0.05 + 0.03, p) == ponko::snapshot_push::added);
    }
    CHECK(stream.push(5000, 1.0, ponko::pose_at({0, 0, 0})) == ponko::snapshot_push::duplicate);
    auto shown = stream.sample(1.0);
    CHECK(shown && shown->state == ponko::snapshot_state::interpolated && std::fabs(shown->value.position.x - 17.4f) < 0.05f);
    CHECK(stream.sample(1.2)->state == ponko::snapshot_state::extrapolated);
    CHECK(stream.sample(5.0)->state == ponko::snapshot_state::held);
    CHECK(stream.size() == 20 && stream.clock_offset() && std::fabs(*stream.clock_offset() + 4999.97) < 0.01);
    stream.set_delay(0);
    throws([&] { stream.set_delay(-1); });
    stream.clear();
    CHECK(stream.size() == 0 && !stream.sample(1.0));
    auto config = ponko::snapshot_defaults();
    config.capacity = 1;
    throws([&] { ponko::snapshot_stream bad(config); });

    ponko::interest interest;
    std::vector<ponko::interest_viewer> viewers{{1, {0, 0, 0}}};
    std::vector<ponko::interest_entity> world{{10, {10, 0, 0}}, {20, {100, 0, 0}}, {30, {900, 0, 0}}};
    interest.update(0, viewers, world);
    auto entries = interest.entries(1);
    CHECK(entries.size() == 2 && entries[0].entity == 10 && entries[0].tier == 0 && entries[1].tier == 1 && entries[1].send);
    CHECK(interest.changes().size() == 2 && interest.changes()[0].kind == PONKO_INTEREST_ENTERED);
    int near_sent = 0, mid_sent = 0;
    for (std::uint64_t tick = 1; tick <= 8; tick++) {
        interest.update(tick, viewers, world);
        CHECK(interest.changes().empty());
        for (auto& e : interest.entries(1)) (e.tier == 0 ? near_sent : mid_sent) += e.send;
    }
    CHECK(near_sent == 8 && mid_sent == 2); // every tick, and every 4th
    interest.update(9, {}, world); // nobody is looking any more
    CHECK(interest.entries(1).empty() && interest.changes().size() == 2);
    interest.remove_peer(1);
    auto bad = ponko::interest_defaults();
    bad.hysteresis = 99;
    throws([&] { ponko::interest invalid(bad); });
}

static void profiling() {
    auto work = [](ponko::ctx&) {
        PONKO_PROFILE_SCOPE("cpp_work");
        volatile float x = 0;
        for (int i = 0; i < 3000; ++i) x = x + std::sqrt(float(i));
    };
    auto leaky = [](ponko::ctx&) { int* p = new int[4]; *static_cast<volatile int*>(p) = 7; delete[] p; };
    auto run = ponko::game("p", 1, 1)
        .sim_system(work, "work")
        .sim_system(leaky, "leaky")
        .profile_headless(5, 40, [](ponko::ctx& ctx) { ctx.world().spawn(Pos{1, 2}); });
    CHECK(run.world.size() == 1);
    const std::string text = run.report.text();
    CHECK(text.find("40 frames") != std::string::npos && text.find("cpp_work") != std::string::npos);
    // C++ `new` is counted: the leaky system is named, the other isn't.
    CHECK(text.find("`leaky` allocates") != std::string::npos);
    CHECK(text.find("`work` allocates") == std::string::npos);
    CHECK(run.report.within(R"({"frame_p99_ms": 1000, "systems": {"work": 1000}})"));
    const std::string broken = run.report.check(R"({"allocs_per_frame": 0, "scopes": {"cpp_work": 0}})");
    CHECK(broken.find("allocations per frame") != std::string::npos && broken.find("cpp_work") != std::string::npos);
    CHECK(run.report.regressions(run.report).empty());
    throws([&] { (void)run.report.check("not json"); });
    const auto file = std::filesystem::temp_directory_path() / "ponko-cpp-report.json";
    run.report.save(file.string().c_str());
    CHECK(ponko::perf_report::load(file.string().c_str()).json() == run.report.json());
    std::filesystem::remove(file);
    // Scopes and profiling toggles work from a live ctx too.
    ponko::game("q", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ctx.set_profiling(true);
        CHECK(ctx.perf_report().json().size() > 2);
        ctx.set_profiling(false);
        // Headless: no renderer, so nothing uploaded to the GPU.
        const ponko::gpu_uploads uploads = ctx.gpu_uploads();
        CHECK(uploads.bytes == 0 && uploads.buffers_created == 0 && uploads.reallocations == 0);
    });
}

// Animator parameters a layer reads (bool, trigger, float), a 2D blend tree,
// and the C++ face of the 3D joint kinds, queries and clock.
static void animator_layers_and_blend_trees() {
    const auto file = write_rise_gltf("ponko-cpp-layers.gltf");
    const std::string path = file.string();
    ponko::game("layers", 1, 1).run_headless(0, [&](ponko::ctx& ctx) {
        auto rise = ctx.load_clip(path.c_str(), "rise");
        auto e = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ctx.load_skeleton(e, path.c_str());
        auto arm = *ctx.find_joint(e, "arm");

        // The base animation holds still; a layer rises when told to.
        ponko::animation_graph_builder base;
        base.add_state(ponko::motion::of(rise), 0);
        ctx.set_animator(e, base.build(0));
        ctx.animator_set_manual(e, true);

        ponko::animation_graph_builder lb;
        auto flag = lb.add_bool();
        auto fire = lb.add_trigger();
        auto amount = lb.add_float();
        auto idle = lb.add_state(ponko::motion::of(rise), 0);
        auto run = lb.add_state(ponko::motion::of(rise), 1, std::nullopt, false);
        auto by_flag = lb.add_transition(idle, run, 0);
        lb.add_condition(by_flag, PONKO_CONDITION_BOOL, flag, 1);
        auto by_trigger = lb.add_transition(idle, run, 0);
        lb.add_condition(by_trigger, PONKO_CONDITION_TRIGGER, fire);
        auto by_float = lb.add_transition(idle, run, 0);
        lb.add_condition(by_float, PONKO_CONDITION_GREATER, amount, 0.5f);
        auto overlay = lb.build(idle);
        auto layer = ctx.animator_add_layer(e, overlay, ponko::layer_blend::override_pose);

        auto arm_y = [&] { return ctx.joint_pose(e, arm).position.y; };
        auto back_to_idle = [&] {
            ctx.animator_layer_crossfade(e, layer, idle, 0);
            ctx.animator_update(e, 0);
            CHECK(std::fabs(arm_y()) < 1e-4f);
        };
        ctx.animator_update(e, 0.5f);
        CHECK(std::fabs(arm_y()) < 1e-4f); // nothing asked the layer to move

        ctx.animator_layer_set_bool(e, layer, flag, true);
        ctx.animator_update(e, 0);
        ctx.animator_update(e, 0.5f);
        CHECK(std::fabs(arm_y() - 1.0f) < 1e-4f); // by the bool
        ctx.animator_layer_set_bool(e, layer, flag, false);
        back_to_idle();

        ctx.animator_layer_set_trigger(e, layer, fire);
        ctx.animator_update(e, 0);
        ctx.animator_update(e, 0.5f);
        CHECK(std::fabs(arm_y() - 1.0f) < 1e-4f); // by the trigger
        back_to_idle();

        ctx.animator_layer_set_float(e, layer, amount, 1.0f);
        ctx.animator_update(e, 0);
        ctx.animator_update(e, 0.5f);
        CHECK(std::fabs(arm_y() - 1.0f) < 1e-4f); // by the float
        throws([&] { ctx.animator_layer_set_float(e, layer + 1, amount, 1.0f); }); // no such layer
        throws_invalid([&] { ctx.animator_layer_set_bool(e, layer, amount, true); }, "not a bool"); // amount is a float

        // A base graph with a trigger: it fires once and is used up.
        ponko::animation_graph_builder tb;
        auto go = tb.add_trigger();
        auto first = tb.add_state(ponko::motion::of(rise), 0);
        auto second = tb.add_state(ponko::motion::of(rise), 0);
        auto t = tb.add_transition(first, second, 0);
        tb.add_condition(t, PONKO_CONDITION_TRIGGER, go);
        auto second_entity = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ctx.load_skeleton(second_entity, path.c_str());
        ctx.set_animator(second_entity, tb.build(first));
        ctx.animator_set_manual(second_entity, true);
        ctx.animator_update(second_entity, 0.1f);
        CHECK(ctx.animator_state(second_entity) == first);
        ctx.animator_set_trigger(second_entity, go);
        ctx.animator_update(second_entity, 0);
        CHECK(ctx.animator_state(second_entity) == second);

        // A 2D blend tree of the same clip at three points plays the clip
        // wherever the two parameters put the blend.
        ponko::animation_graph_builder bb;
        auto x = bb.add_float();
        auto y = bb.add_float();
        auto tree = ponko::motion::blend2d(x, y);
        tree.add(0, 0, ponko::motion::of(rise)).add(1, 0, ponko::motion::of(rise)).add(0, 1, ponko::motion::of(rise), 1);
        bb.add_state(std::move(tree), 1);
        auto third = ctx.world().spawn(ponko::transform3d_at({0, 0, 0}));
        ctx.load_skeleton(third, path.c_str());
        ctx.set_animator(third, bb.build(0));
        ctx.animator_set_manual(third, true);
        ctx.animator_set_float(third, x, 0.3f);
        ctx.animator_set_float(third, y, 0.6f);
        ctx.animator_update(third, 0.5f);
        CHECK(std::fabs(ctx.joint_pose(third, arm).position.y - 1.0f) < 1e-4f);
    });
    std::filesystem::remove(file);
}

// Joint kinds, 3D queries, 3D body kinds, gravity, the clock, idle input and
// the headless camera helpers, through the C++ wrapper.
static void headless_wrapper_calls() {
    // Joints: which axes each kind leaves free.
    {
        auto count = [](const ponko::joint3d& j, PonkoJointMotionKind kind) {
            int n = 0;
            for (auto& axis : j.axes) n += axis.kind == kind;
            return n;
        };
        CHECK(count(ponko::fixed_joint3d(1, 2), PONKO_JOINT_LOCKED) == 6);
        CHECK(count(ponko::generic_joint3d(1, 2), PONKO_JOINT_FREE) == 6);
        auto slider = ponko::prismatic_joint3d(1, 2);
        CHECK(slider.axes[ponko::joint_axis::linear_x].kind == PONKO_JOINT_FREE && count(slider, PONKO_JOINT_FREE) == 1);
        CHECK(ponko::kinematic_body3d().kind == PONKO_BODY_KINEMATIC);
        CHECK(ponko::kinematic_body().kind == PONKO_BODY_KINEMATIC);
    }

    ponko::entity wall = 0, ball = 0, plain = 0;
    bool asked = false;
    ponko::game("queries", 1, 1)
        .physics3d({0, 0, 0})
        .physics({0, 0})
        .spatial_grid(16)
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() != 1) return;
            asked = true;
            // A ray along +Z meets the wall (4 to 6 m away) and then the ball.
            std::vector<ponko::hit3d> hits;
            ctx.cast_ray3d_all({0, 0, 0}, {0, 0, 1}, 100, hits);
            CHECK(hits.size() == 2);
            CHECK(ponko_entity_bits(hits[0].entity) == wall);
            CHECK(std::fabs(hits[0].distance - 4.0f) < 1e-3f);
            CHECK(ponko_entity_bits(hits[1].entity) == ball);
            CHECK(hits[1].distance > hits[0].distance);
            // A ball swept along +Z touches the wall half its radius early.
            auto swept = ctx.cast_sphere3d({0, 0, 0}, 0.5f, {0, 0, 1}, 100);
            CHECK(swept && std::fabs(swept->distance - 3.5f) < 1e-3f);
            CHECK(!ctx.cast_sphere3d({0, 0, 0}, 0.5f, {0, 1, 0}, 100));
            auto capsule = ctx.cast_capsule3d({0, 0, 0}, 2, 0.5f, {0, 0, 1}, 100);
            CHECK(capsule && std::fabs(capsule->distance - 3.5f) < 1e-3f);
            // Overlaps: a box around the wall finds it and not the ball.
            std::vector<PonkoEntity> found;
            ctx.overlap_box3d({0, 0, 5}, {4, 4, 4}, {0, 0, 0, 1}, found);
            CHECK(found.size() == 1 && ponko_entity_bits(found[0]) == wall);
            // Masses: the ball has one, a plain entity is no body.
            CHECK(ctx.mass3d(ball) && *ctx.mass3d(ball) > 0);
            CHECK(!ctx.mass3d(plain));
            // The built-in grid sees the 2D transforms, and nothing outside the rectangle.
            std::vector<ponko::grid_hit> cells;
            ctx.spatial_rect({-1, -1}, {1, 1}, cells);
            CHECK(cells.empty());

            // The clock, idle input and cameras of a headless run.
            ctx.set_sim_speed(0.5f);
            CHECK(ctx.sim_speed() == 0.5f);
            throws([&] { ctx.set_sim_speed(-1); });
            CHECK(!ctx.key_released(PONKO_KEY_A) && !ctx.mouse_released(PONKO_MOUSE_LEFT));
            CHECK(ctx.mouse_pos().x == 0 && ctx.mouse_pos().y == 0);
            CHECK(ctx.mouse_wheel().y == 0 && ctx.mouse_motion().x == 0);
            CHECK(!ctx.cursor_locked());
            auto [origin, direction] = ctx.screen_ray(ctx.window_size() * 0.5f);
            auto camera = ctx.camera3d();
            CHECK(origin.x == camera.position.x && origin.y == camera.position.y && origin.z == camera.position.z);
            CHECK(std::fabs(std::sqrt(direction.x * direction.x + direction.y * direction.y + direction.z * direction.z) - 1.0f) < 1e-4f);
            CHECK(ctx.models_drawn() == 0);
            ctx.set_gravity({0, 10});
            ctx.set_physics_paused(true);
            CHECK(ctx.is_physics_paused());
            ctx.set_physics_paused(false);
            CHECK(!ctx.is_physics_paused());
            ctx.set_gravity3d({0, -1, 0});
            throws([&] { ctx.set_gravity({NAN, 0}); });
            ctx.warm_render_pipelines(); // nothing to build without a renderer
        })
        .run_headless(3, [&](ponko::ctx& ctx) {
            auto world = ctx.world();
            wall = world.spawn(ponko::transform3d_at({0, 0, 5}), ponko::fixed_body3d(), ponko::box_collider({2, 2, 2}));
            ball = world.spawn(ponko::transform3d_at({0, 0, 10}), ponko::dynamic_body3d(), ponko::sphere_collider(0.5f));
            plain = world.spawn(ponko::transform3d_at({30, 0, 0}));
            auto pole = world.spawn(ponko::transform3d_at({50, 0, 0}), ponko::kinematic_body3d(), ponko::capsule_collider3d(1, 0.25f));
            CHECK(world.get<ponko::rigid_body3d>(pole).kind == PONKO_BODY_KINEMATIC);
        });
    CHECK(asked);
}

// Shader, effect and compute files load once, and the same file gives the same handle.
// Hot reload is off for the files that must fail: with it on (the default in
// debug builds of the engine) a file that doesn't compile loads as a stand-in.
static void shader_files() {
    const auto dir = std::filesystem::temp_directory_path();
    const auto surface = dir / "ponko-cpp-wind.wgsl", effect = dir / "ponko-cpp-grey.wgsl", compute = dir / "ponko-cpp-fade.wgsl";
    std::ofstream(surface) << "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.x += material.custom[0].x; return o; }";
    std::ofstream(effect) << "fn effect(p: Pixel) -> vec3<f32> { return vec3<f32>(dot(p.color, vec3<f32>(0.3, 0.6, 0.1))); }";
    std::ofstream(compute) << "fn compute(c: Cell) -> vec4<f32> { return c.value * 0.9; }";
    ponko::game("shaders", 1, 1).hot_reload(false).run_headless(0, [&](ponko::ctx& ctx) {
        auto a = ctx.load_shader(surface.string().c_str());
        CHECK(a.id == ctx.load_shader(surface.string().c_str()).id);
        auto e = ctx.load_effect(effect.string().c_str());
        CHECK(e.id == ctx.load_effect(effect.string().c_str()).id);
        auto c = ctx.load_compute(compute.string().c_str());
        CHECK(c.id == ctx.load_compute(compute.string().c_str()).id);
        throws([&] { ctx.load_shader("no-such-shader.wgsl"); });
        throws([&] { ctx.load_effect(surface.string().c_str()); }); // a surface shader is no effect
        throws([&] { ctx.load_compute("no-such-compute.wgsl"); });
    });
    // With hot reload on, the file that is no effect still gets a handle: a
    // stand-in, reported as failed and watched. A missing file is still an error.
    ponko::game("stand-ins", 1, 1).hot_reload(true).run_headless(0, [&](ponko::ctx& ctx) {
        (void)ctx.load_effect(surface.string().c_str());
        auto shaders = ctx.shader_status();
        CHECK(shaders.size() == 1 && !shaders[0].ok());
        CHECK(shaders[0].info.kind == PONKO_SHADER_EFFECT && shaders[0].info.outcome == PONKO_SHADER_FAILED);
        CHECK(!shaders[0].errors.empty());
        throws([&] { ctx.load_effect("no-such-effect.wgsl"); });
    });
    for (auto& f : {surface, effect, compute}) std::filesystem::remove(f);
}

static void command_barriers() {
    // A system's commands and events reach the next system together, and
    // nothing is applied while the recording system (or its loop) runs.
    // The same with the scheduler off and on: C++ systems always run alone.
    struct Mark { int from; };
    struct Ping { int value; };
    for (bool parallel : {false, true}) {
        std::vector<std::size_t> seen;
        std::size_t pings = 0;
        auto g = ponko::game("t", 1, 1);
        if (parallel) g.parallel_systems();
        auto w = g
            .sim_system([](ponko::ctx& ctx) {
                const std::size_t before = ctx.world().size();
                // A loop can record commands but can't look at the world it is
                // walking, so it never sees a half-applied change.
                ctx.world().each([&](const Mark&) {
                    ctx.spawn_later(Mark{1});
                    throws([&] { (void)ctx.world().size(); });
                });
                CHECK(ctx.world().size() == before);       // pending after the loop
                ctx.spawn_later(Mark{2});
                ctx.send(Ping{7});
                CHECK(ctx.world().size() == before);       // and until the system returns
            }, "recorder")
            .sim_system([&](ponko::ctx& ctx) {
                seen.push_back(ctx.world().size());
                for (const Ping& p : ctx.read<Ping>()) { CHECK(p.value == 7); ++pings; }
            }, "next")
            // A command from setup applies before the first tick.
            .run_headless(3, [](ponko::ctx& ctx) { ctx.spawn_later(Mark{0}); });
        CHECK((seen == std::vector<std::size_t>{3, 7, 15}));
        CHECK(pings == 3);
        CHECK(w.size() == 15);
    }
}

static void declared_access() {
    // A system declares what it reads and writes; debug builds of the
    // library stop the game if it uses anything else (tests/system_access.rs).
    struct Score { int value; };
    auto mover = ponko::access().write<Pos>().read<Vel>().read_service(ponko::service::time);
    auto scorer = ponko::access().write_resource<Score>().read_events<Died>();
    auto killer = ponko::access().read<Hp>().send_events<Died>().commands();
    CHECK(mover.conflicts_before(scorer) == 0);
    // The scorer would see the killer's events and applied commands.
    CHECK(killer.conflicts_before(scorer) == 2);
    CHECK(scorer.conflicts_before(killer) == 0);
    CHECK(scorer.conflicts_before(killer, false) == 2);
    ponko::access copy = killer;
    CHECK(copy.conflicts_before(scorer) == 2);
    copy = mover;
    CHECK(copy.conflicts_before(mover) == 1);
    int scored = 0;
    auto g = ponko::game("t", 1, 1);
    g.add(ponko::phase::after_physics, mover, [](ponko::ctx& ctx) {
         ctx.world().each([](Pos& p, const Vel& v) { p.x += v.x; });
     }, "mover")
     .add(ponko::phase::after_physics, killer, [](ponko::ctx& ctx) {
         ctx.world().each([&](ponko::entity e, const Hp& hp) {
             if (hp.value <= 0) { ctx.send(Died{e, ctx.sim_tick()}); ctx.despawn_later(e); }
         });
     }, "killer")
     .add(ponko::phase::after_physics, scorer, [&](ponko::ctx& ctx) {
         scored += static_cast<int>(ctx.read<Died>().size());
     }, "scorer");
    const std::string report = g.access_report();
    CHECK(report.find("mover + killer: could run together") == std::string::npos);
    CHECK(report.find("mover + killer: no conflict, but a `Ctx` system stays sequential") != std::string::npos);
    CHECK(report.find("killer + scorer: conflict on events") != std::string::npos);
    // With the scheduler on, C++ systems keep running one at a time: they take the whole ctx.
    g.parallel_systems();
    const std::string plan = g.schedule_plan();
    CHECK(plan.find("  mover (alone: takes `&mut Ctx`)") != std::string::npos);
    CHECK(plan.find("  scorer (alone: takes `&mut Ctx`)") != std::string::npos);
    auto w = g.run_headless(2, [](ponko::ctx& ctx) {
        ctx.world().set_resource(Score{0});
        ctx.world().spawn(Pos{0, 0}, Vel{1, 0});
        ctx.world().spawn(Hp{0});
    });
    CHECK(scored == 1);
    CHECK(w.size() == 1);
}

// Action ids, input scenarios, asset sources, resource_scope and the try_ loaders (#262).
static ponko::action_map jump_map() {
    ponko::action_map map;
    map.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_SPACE)}));
    return map;
}
static void native_coverage() {
    // Action ids: resolve once, query without the name.
    ponko::game("ids", 1, 1).run_headless(0, [](ponko::ctx& ctx) {
        ctx.set_actions(jump_map());
        const auto jump = ctx.action_id("jump");
        CHECK(!jump.is_none() && jump != ponko::action_id::none());
        throws_kind([&] { ctx.action_id("fly"); }, PONKO_ERROR_NOT_FOUND);
        auto [again] = ctx.action_ids({"jump"});
        CHECK(again == jump);
        ctx.feed_input(ponko::input_event::key(PONKO_KEY_SPACE, true));
        CHECK(ctx.action_down(jump) && ctx.action_pressed(jump) && !ctx.action_released(jump));
        CHECK(ctx.action_axis(jump) == 0 && !ctx.action_down(ponko::action_id::none()));

        // Pressed this frame: a rebinding screen binds the answer directly (Space went down above).
        ctx.feed_input(ponko::input_event::button(ponko::gamepad{1}, PONKO_GAMEPAD_WEST, true));
        const auto fresh = ctx.pressed_this_frame();
        CHECK(fresh.size() == 2 && fresh[0].is_key() && fresh[0].key() == PONKO_KEY_SPACE);
        CHECK(fresh[1].is_gamepad_button() && fresh[1].pad().slot == 1 && fresh[1].button() == PONKO_GAMEPAD_WEST);
        auto rebound = ctx.actions();
        rebound.set("jump", ponko::action::button({fresh[1].binding()}));
        ctx.set_actions(rebound);
        CHECK(ctx.action_down("jump"));
        throws_kind([&] { ctx.pressed_this_frame(-1.0f); }, PONKO_ERROR_INVALID);

        // try_ loaders: the error says what and where.
        auto missing = ctx.try_load_texture("no/such.png");
        CHECK(!missing && missing.kind() == PONKO_ERROR_NOT_FOUND);
        CHECK(missing.error().path() == "no/such.png");
        CHECK(std::string(ponko::kind_name(missing.kind())) == "not found");
        CHECK(missing.value_or(ponko::texture{}).id == 0);
        CHECK(!ctx.try_load_font("no/such.ttf") && !ctx.try_load_sound("no/such.ogg"));
        CHECK(!ctx.try_load_stream("no/such.ogg") && !ctx.try_load_mesh("no/such.glb"));
        bool thrown = false;
        try { ctx.load_texture("no/such.png"); } catch (const ponko::error& e) {
            thrown = e.kind() == PONKO_ERROR_NOT_FOUND && e.path() == "no/such.png";
        }
        CHECK(thrown);

        // Asset sources, then your own decoder.
        ctx.add_asset_source(ponko::memory_source("embedded").with("level.txt", std::string("##.")).with("bytes.bin", {1, 2, 3}));
        CHECK(ctx.load_asset_with("bytes.bin", [](const std::uint8_t* data, std::size_t len) { return len == 3 && data[2] == 3; }));
        int asked = 0;
        ctx.add_asset_source("generated", [&](const char* name) -> std::optional<std::vector<std::uint8_t>> {
            ++asked;
            if (std::string(name) != "blob.bin") return std::nullopt;
            return std::vector<std::uint8_t>{1, 2, 3, 4};
        });
        auto walls = ctx.load_asset_with("level.txt", [](const std::uint8_t* data, std::size_t len) {
            return std::count(data, data + len, '#');
        });
        CHECK(walls == 2);
        CHECK(ctx.load_asset_with("blob.bin", [](const std::uint8_t*, std::size_t len) { return len; }) == 4);
        CHECK(asked >= 1);
        throws_kind([&] { ctx.load_asset_with("missing.bin", [](const std::uint8_t*, std::size_t) { return 0; }); },
                    PONKO_ERROR_NOT_FOUND);
        throws([&] { ctx.load_asset_with("level.txt", [](const std::uint8_t*, std::size_t) -> int { throw std::runtime_error("bad"); }); });
    });

    // resource_scope: the resource and the world, lent together.
    ponko::world w;
    w.set_resource(game::Score{1, 2});
    auto red = w.resource_scope<game::Score>([](ponko::world_view& world, game::Score& score) {
        CHECK(!world.try_resource<game::Score>());  // lent out
        score.red += 41;
        return score.red;
    });
    CHECK(red == 42 && w.resource<game::Score>().red == 42);
    w.resource_scope<game::Score>([](ponko::world_view& world, game::Score&) { world.set_resource(game::Score{0, 0}); });
    CHECK(w.resource<game::Score>().red == 42);  // the original wins
    throws([&] { w.resource_scope<game::Score>([](ponko::world_view&, game::Score&) { throw std::runtime_error("scope"); }); });
    CHECK(w.resource<game::Score>().red == 42);
    throws_kind([&] { w.resource_scope<game::Tuning>([](ponko::world_view&, game::Tuning&) {}); }, PONKO_ERROR_NOT_FOUND);

    // Input scenarios: Space held on ticks 2-4, checked through the action.
    using ponko::scenario_check;
    ponko::input_scenario s;
    s.at(2, ponko::input_event::key(PONKO_KEY_SPACE, true))
        .at(5, ponko::input_event::key(PONKO_KEY_SPACE, false))
        .expect(2, scenario_check::action("jump").pressed(true).down(true))
        .expect(5, scenario_check::action("jump").down(false).released(true))
        .expect(5, scenario_check::resource("game::Score", "/red", "3"))
        .named("jump").duration(6).release_when_done();
    CHECK(s.ticks() == 6 && s.step_count() == 5);
    auto copy = ponko::input_scenario::from_json(s.to_json());
    CHECK(copy.to_json() == s.to_json());
    throws_kind([] { ponko::input_scenario::from_json("{"); }, PONKO_ERROR_JSON);
    auto run = ponko::game("scenario", 1, 1).run_headless_scenario(copy, [](ponko::ctx& ctx) {
        ctx.set_actions(jump_map());
        auto world = ctx.world();
        world.set_resource(game::Score{3, 0});
        world.reflect_resource<game::Score>({ponko::field("red", &game::Score::red), ponko::field("blue", &game::Score::blue)});
    });
    CHECK(run.report.passed() && run.report.checks_passed == 3 && run.report.inputs_applied == 2);
    CHECK(run.world.scenario_report() && run.world.scenario_report_json().find("\"passed\"") != std::string::npos);
    ponko::world fresh;
    CHECK(!fresh.scenario_report() && fresh.scenario_report_json().empty());
    // Started and cancelled from a system.
    bool started = false, cancelled = false;
    ponko::game("live", 1, 1)
        .sim_system([&](ponko::ctx& ctx) {
            if (ctx.sim_tick() == 0) { ctx.start_scenario(ponko::input_scenario().duration(10)); started = true; }
            if (ctx.sim_tick() == 2) cancelled = ctx.cancel_scenario();
        })
        .run_headless(3);
    CHECK(started && cancelled);
}

// The small wrappers added for full C coverage: defaults, math and mesh data.
static void wrapper_coverage() {
    // Query cache policies: the default a world starts with, and off.
    ponko::world w;
    const auto start = w.query_cache_policy();
    const auto defaults = ponko::query_cache_policy_default();
    CHECK(start.min_archetypes == defaults.min_archetypes && start.max_signatures == defaults.max_signatures);
    const auto off = ponko::query_cache_policy_off();
    w.set_query_cache_policy(off);
    CHECK(w.query_cache_policy().max_signatures == off.max_signatures);
    w.spawn(Pos{1, 2});
    CHECK(w.count<Pos>() == 1 && w.query_cache_stats().indexed == 0);

    // Quaternions: identity, and the inline operators agree with the C math.
    const ponko::quat id = ponko::quat_identity();
    CHECK(id.x == 0 && id.y == 0 && id.z == 0 && id.w == 1);
    const ponko::quat turn = ponko::quat_from_rotation_y(1.0f), tilt = ponko::quat_from_rotation_x(0.5f);
    const ponko::quat both = turn * tilt, c_both = ponko_quat_mul(turn, tilt);
    CHECK(std::fabs(both.x - c_both.x) < 1e-6f && std::fabs(both.w - c_both.w) < 1e-6f);
    const ponko::vec3 v = turn * ponko::vec3{1, 0, 0}, c_v = ponko_quat_rotate(turn, {1, 0, 0});
    CHECK(std::fabs(v.x - c_v.x) < 1e-6f && std::fabs(v.z - c_v.z) < 1e-6f);
    CHECK(id * ponko::vec3{1, 2, 3} == (ponko::vec3{1, 2, 3}));
    CHECK(ponko::bits(ponko::entity_of(0x0000000500000007ull)) == ponko_entity_bits(ponko::entity_of(0x0000000500000007ull)));

    // Tyres: the usual curves are what a new wheel has.
    const auto forward = ponko::friction_curve_forward(), sideways = ponko::friction_curve_sideways();
    CHECK(forward.extremum_slip == 0.4f && forward.extremum_value == 1.0f && forward.asymptote_value == 0.5f);
    CHECK(sideways.extremum_slip == 0.2f && sideways.asymptote_value == 0.75f);
    const auto w3 = ponko::wheel({0, 0, 0}, 0.3f);
    CHECK(w3.forward_friction.asymptote_slip == forward.asymptote_slip);
    CHECK(w3.sideways_friction.asymptote_slip == sideways.asymptote_slip);
    CHECK(ponko::evaluate(forward, forward.extremum_slip) == forward.extremum_value * forward.stiffness);

    // Collision groups: everything, as a new collider has.
    const auto all = ponko::collision_groups_all();
    CHECK(all.memberships == UINT32_MAX && all.filter == UINT32_MAX);
    const auto ball = ponko::sphere_collider(1);
    CHECK(ball.groups.memberships == all.memberships && ball.groups.filter == all.filter);

    // World text: the C++ defaults are the engine's.
    const ponko::text3d text("hi");
    const PonkoText3D c_text = ponko_text3d_default();
    CHECK(text.size == c_text.size && text.outline_width == c_text.outline_width && text.softness == c_text.softness);
    CHECK(static_cast<std::uint32_t>(text.align) == static_cast<std::uint32_t>(c_text.align));
    CHECK(text.billboard == c_text.billboard && text.color.a == c_text.color.a && text.outline.a == c_text.outline.a);

    // Morph targets: count them, then clear them (frames go with them).
    auto sheet = ponko::mesh_data::plane(1.0);
    CHECK(sheet.morph_target_count() == 0);
    sheet.add_morph_target({"dent", std::vector<ponko::vec3>(4, {0, 0, -0.5f}), {}});
    sheet.add_morph_target({"tip", std::vector<ponko::vec3>(4), {}});
    sheet.add_morph_frame(0, 1, 1.0f);
    CHECK(sheet.morph_target_count() == 2);
    sheet.clear_morph_targets();
    CHECK(sheet.morph_target_count() == 0);
    sheet.add_morph_target({"again", std::vector<ponko::vec3>(4), {}});
    sheet.add_morph_frame(0, 0, 1.0f); // channel 0 is empty again, so this frame is its first
    CHECK(sheet.morph_target_count() == 1);
}

// Headless frames: a form typed into and clicked by node name.
static void headless_frames_ui() {
    using ponko::scenario_check;
    ponko::input_scenario s;
    s.type_text(2, "form.name", "Ada")
        .expect(2, scenario_check::ui("form.name").exists(true).shown(true).focused(true).text("Ada"))
        .click(4, "form.ok")
        .expect(4, scenario_check::ui("form").shown(false))
        .duration(6);
    int closed = 0;
    auto run = ponko::game("c++ frames", 640, 480)
                   .headless_frames(true)
                   .system([&](ponko::ctx& ctx) {
                       auto ui = ctx.ui_retained();
                       auto clicked = ui.take_click();
                       if (clicked && clicked == ui.find("form.ok")) {
                           ui.pop_screen();
                           ++closed;
                       }
                   })
                   .run_headless_scenario(s, [](ponko::ctx& ctx) {
                       namespace u = ponko::ui;
                       auto ui = ctx.ui_retained();
                       auto root = ui.add_panel(u::none, u::fill());
                       ui.set_name(root, "form");
                       ui.set_name(ui.add_text_input(root, "", u::fixed(200, 30)), "form.name");
                       ui.set_name(ui.add_button(root, "OK", u::fixed(200, 30)), "form.ok");
                       CHECK(!ui.is_shown(root));
                       ui.push_screen(root);
                       CHECK(ui.is_shown(root));
                   });
    CHECK(run.report.passed() && run.report.inputs_applied == 2 && closed == 1);
}

static void levels_and_tilemaps() {
    ponko::game("c++ levels", 1, 1).physics({0, 700}).run_headless(0, [](ponko::ctx& ctx) {
        ponko::level_options options;
        options.solid = {"Ground"};
        options.z = 2.0f;
        const ponko::entity root = ponko::spawn_level(ctx, "levels/meadow.tmx", {0, 0}, options);
        CHECK(ponko::level_property_json(ctx, root, "title") == "\"Meadow\"");
        throws_kind([&] { (void)ponko::level_property_json(ctx, root, "missing"); }, PONKO_ERROR_NOT_FOUND);
        auto world = ctx.world();
        int coins = 0, bodies = 0;
        ponko::entity ground = 0, start = 0;
        for (ponko::entity e : ponko::level_entities(ctx, root)) {
            if (world.try_get<ponko::rigid_body>(e)) ++bodies;
            try {
                const ponko::level_object o = ponko::level_object_of(ctx, e);
                const std::string cls = ponko::level_object_class(ctx, e);
                if (cls == "Coin") { ++coins; CHECK(o.has_tile && o.shape == PONKO_LEVEL_SHAPE_RECT); }
                if (cls == "Spawn") { start = e; CHECK(o.shape == PONKO_LEVEL_SHAPE_POINT && o.position.x == 48.0f); }
                CHECK(ponko::level_object_layer(ctx, e) == "Objects");
                continue;
            } catch (const ponko::error&) {}
            if (ground == 0) ground = e;
        }
        CHECK(coins == 6 && bodies > 0 && start != 0);
        CHECK(ponko::level_object_name(ctx, start) == "Start");
        throws([&] { (void)ponko::level_object_of(ctx, root); });

        // A map built in code: autotile, flips, animation, parallax, solid rectangles.
        const ponko::entity map = world.spawn(ponko::transform_at({0, 0}));
        auto tiles = world.add_tilemap(map, world.tilemap(ground).style().sheet, 4, 3);
        ponko::autotile_rule rule;
        for (std::uint16_t i = 0; i < 16; ++i) rule.frames.push_back(i);
        rule.outside_filled = false;
        std::vector<bool> filled(12, false);
        filled[1 * 4 + 1] = filled[1 * 4 + 2] = true;
        ponko::tilemap_autotile(ctx, map, rule, filled);
        CHECK(tiles.get(1, 1) == std::uint16_t(2) && tiles.get(2, 1) == std::uint16_t(8) && !tiles.get(0, 0));
        ponko::tilemap_paint_autotile(ctx, map, rule, 1, 0, true);
        CHECK(tiles.get(1, 0) == std::uint16_t(4) && tiles.get(1, 1) == std::uint16_t(3));
        ponko::tilemap_set_flipped(ctx, map, 3, 2, 5, PONKO_TILE_FLIP_X | PONKO_TILE_FLIP_DIAGONAL);
        CHECK(ponko::tilemap_flip(ctx, map, 3, 2) == (PONKO_TILE_FLIP_X | PONKO_TILE_FLIP_DIAGONAL) && tiles.get(3, 2) == std::uint16_t(5));
        ponko::tilemap_animate(ctx, map, 5, {{5, 0.2f}, {6, 0.2f}});
        ponko::tilemap_set_parallax(ctx, map, {0.5f, 0.5f}, {10, 10});
        const auto rects = ponko::tilemap_solid_rects(ctx, map, {2, 3, 4, 8});
        CHECK(rects.size() == 2);
        CHECK(ponko::tilemap_spawn_colliders(ctx, map, {2, 3, 4, 8}) == 2);

        // Saves leave the level out; a game's entity is saved unless left out.
        CHECK(!world.saved(root) && !world.saved(ground) && world.saved(map));
        world.set_saved(map, false);
        CHECK(!world.saved(map));
        world.set_saved(map, true);
        ponko::level_options after_load = options;
        after_load.without_saved_objects = true; // meadow has no prefab objects: all of it again
        const ponko::entity again = ponko::spawn_level(ctx, "levels/meadow.tmx", {0, 0}, after_load);
        CHECK(ponko::level_entities(ctx, again).size() == ponko::level_entities(ctx, root).size());
        CHECK(ponko::despawn_level(ctx, again));

        CHECK(ponko::despawn_level(ctx, root));
        CHECK(!ponko::despawn_level(ctx, root));
        throws([&] { (void)ponko::spawn_level(ctx, "levels/nowhere.tmx", {0, 0}); });
    });
}

// The bindings of 0.2 that were Rust-only (#387): action read-back and set_action, built-ins and their
// declarations, LOD hysteresis and instance levels, scene compute passes and target effects, keyed audio
// curves, save slots, the immediate UI's font, border and scale, and the ECS diagnostics.
static void remaining_bindings() {
    // An action map reads back by name.
    ponko::action_map map;
    map.set("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_SPACE), ponko::action_binding::mouse(PONKO_MOUSE_LEFT)}));
    map.set("move", ponko::action::axis2({ponko::action_binding::keys2(PONKO_KEY_A, PONKO_KEY_D, PONKO_KEY_W, PONKO_KEY_S)}));
    CHECK(map.size() == 2);
    CHECK((map.names() == std::vector<std::string>{"jump", "move"}));
    auto jump = map.get("jump");
    CHECK(jump && jump->kind == 0 && jump->bindings.size() == 2 && jump->bindings[0].first == static_cast<std::uint32_t>(PONKO_KEY_SPACE));
    CHECK(map.get("move")->bindings[0].fourth == static_cast<std::uint32_t>(PONKO_KEY_S));
    CHECK(!map.get("fire"));

    // Built-ins: their declarations, and adding one back where you want it.
    auto navmesh = ponko::access::builtin(ponko::systems::step_navmesh3d);
    CHECK(navmesh.conflicts_before(ponko::access(), false) == 0);
    ponko::access copy = navmesh;
    CHECK(copy.conflicts_before(navmesh, false) == navmesh.conflicts_before(navmesh, false) && copy.conflicts_before(navmesh, false) > 0);
    CHECK(ponko::access::builtin(ponko::systems::draw_sprites).conflicts_before(ponko::access(), false) > 0);
    throws_invalid([] { ponko::access::builtin(ponko::system_id{999}); }, "PONKO_SYSTEM_");
    auto g = ponko::game("bindings", 1, 1);
    g.remove(ponko::systems::draw_tilemaps).add_builtin(ponko::phase::late, ponko::systems::draw_tilemaps);
    const std::string plan = g.schedule_plan();
    CHECK(plan.find("Late\n  ponko::tilemap::draw_tilemaps") != std::string::npos);
    throws_invalid([&] { g.add_builtin(ponko::phase::late, ponko::system_id{999}); }, "PONKO_SYSTEM_");

    // LOD levels for an instance batch.
    const auto near = ponko::lod_renderer_of(ponko::mesh{4});
    const auto far = ponko::lod_renderer_of(ponko::mesh{5});
    const float fov = 1.0f;
    auto levels = ponko::lod_instance_levels(2.0f, {{0.2f, 0, {near}, false, {}}, {0.05f, 0, {far}, false, {}}}, 1.0f, fov, {1.0f, true});
    CHECK(levels && levels->size() == 2 && (*levels)[0].mesh.id == 4);
    CHECK((*levels)[0].max_distance == ponko::lod_distance_at(2.0f, 0.2f, fov));
    auto tinted = near; tinted.color.g = 0.5f;
    CHECK(!ponko::lod_instance_levels(2.0f, {{0.2f, 0, {tinted}, false, {}}}, 1.0f, fov, {1.0f, true}));

    int diagnosed = 0;
    g.add(ponko::phase::after_physics, [&](ponko::ctx& ctx) {
        const std::string json = ctx.ecs_diagnostics();
        diagnosed += json.find("\"entities\":1") != std::string::npos && json.find("\"name\":\"diagnose\"") != std::string::npos;
    }, "diagnose");
    const auto dir = std::filesystem::temp_directory_path() / "ponko-cpp-remaining-bindings";
    std::filesystem::remove_all(dir);
    g.data_dir(dir.string().c_str());
    g.run_headless(1, [](ponko::ctx& ctx) {
        // set_action: one action changes, its id stays, the others stay.
        ctx.set_actions(jump_map());
        const auto id = ctx.action_id("jump");
        ctx.set_action("jump", ponko::action::button({ponko::action_binding::key(PONKO_KEY_J)}));
        CHECK(ctx.action_id("jump") == id);
        CHECK(ctx.actions().get("jump")->bindings[0].first == static_cast<std::uint32_t>(PONKO_KEY_J));
        throws_invalid([&] { ctx.set_action("jump", ponko::action{0, {ponko::action_binding::keys(PONKO_KEY_A, PONKO_KEY_D).raw}}); }, "match");

        // LOD hysteresis.
        auto e = ctx.world().spawn(Pos{0, 0});
        CHECK(ctx.lod_hysteresis(e) == 0.0f);
        ctx.set_lod_hysteresis(e, 0.1f);
        CHECK(ctx.lod_hysteresis(e) == 0.1f);
        throws_invalid([&] { ctx.set_lod_hysteresis(e, 2.0f); }, "between 0 and 1");

        // Scene compute passes and target effects.
        auto light = ctx.create_render_target(16, 9);
        auto meter = ctx.create_compute("meter", "fn compute(c: Cell) -> vec4<f32> { return vec4<f32>(scene_color(c.uv * frame.screen.xy), 1.0); }");
        ctx.set_scene_compute_passes({ponko::compute_pass_of(meter, light)});
        CHECK(ctx.scene_compute_passes().size() == 1 && ctx.compute_passes().empty());
        auto far_effect = ctx.create_effect("far", "fn effect(p: Pixel) -> vec3<f32> { return vec3<f32>(p.distance); }");
        auto mask = ctx.create_render_target(32, 32);
        ctx.set_target_effects({ponko::target_effect_of(far_effect, mask)});
        auto effects = ctx.target_effects();
        CHECK(effects.size() == 1 && effects[0].target.id == mask.id && !effects[0].has_material);
        CHECK(ctx.effects().empty());

        // Keyed distance curves and the audio lock.
        std::vector<float> samples(800, 0.1f);
        auto beep = ctx.create_sound(8000, samples.data(), samples.size());
        const std::vector<ponko::audio_key> curve{{0.0f, 1.0f}, {0.5f, 0.3f}, {1.0f, 0.0f}};
        ctx.play_sound_at(beep, {3, 0, 0}, ponko::audio_settings3d_defaults(), curve);
        auto speaker = ctx.world().spawn(ponko::transform3d_at({3, 0, 0}));
        ctx.set_audio_source3d(speaker, beep, ponko::audio_settings3d_defaults(), curve);
        throws_invalid([&] { ctx.play_sound_at(beep, {0, 0, 0}, ponko::audio_settings3d_defaults(), {{0.5f, 1.0f}, {0.0f, 0.0f}}); }, "sorted");
        CHECK(ctx.audio_unlocked());

        // Save slots.
        ctx.save_world("b");
        ctx.save_world("a");
        CHECK((ctx.storage_world_slots() == std::vector<std::string>{"a", "b"}));
        ctx.delete_world("a");
        CHECK((ctx.storage_world_slots() == std::vector<std::string>{"b"}));
        CHECK(ctx.storage_is_persistent() && !ctx.storage_is_saving());

        // The immediate widgets.
        CHECK(ctx.ui_immediate_font().id == 0);
        ctx.set_ui_immediate_font(ponko::font{});
        ctx.set_ui_immediate_border_width(2.0f);
        CHECK(ctx.ui_immediate_border_width() == 2.0f);
        ctx.ui_retained().set_scale(2.0f);
        CHECK(ctx.ui_immediate_scale() == 2.0f);
        ctx.world().despawn(speaker);
    });
    CHECK(diagnosed == 1);
    std::filesystem::remove_all(dir);
}

// A menu that opens on Play, a HUD that Escape leaves up, and a glTF floor
// made solid in one call.
static void focus_huds_and_collision_files() {
    const auto file = std::filesystem::temp_directory_path() / "ponko-cpp-solid-floor.gltf";
    std::ofstream(file) << R"({"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}],
        "buffers": [{"byteLength": 60, "uri": "data:application/octet-stream;base64,AAAAwAAAAAAAAABAAAAAQAAAAAAAAABAAAAAQAAAAAAAAADAAAAAwAAAAAAAAADAAAABAAIAAAACAAMA"}],
        "bufferViews": [{"buffer": 0, "byteLength": 48}, {"buffer": 0, "byteOffset": 48, "byteLength": 12}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [-2, 0, -2], "max": [2, 0, 2]},
            {"bufferView": 1, "componentType": 5123, "count": 6, "type": "SCALAR"}]})";
    const std::string path = file.string();
    ponko::game("focus and huds", 1, 1).physics3d({0, -9.81f, 0}).run_headless(0, [&](ponko::ctx& ctx) {
        auto ui = ctx.ui_retained();
        const ponko::vec2 window{800, 600};
        namespace u = ponko::ui;
        auto hud = ui.add_panel(u::none, u::layout().size(300, 40));
        auto count = ui.add_label(hud, "0 / 5", u::layout().size(120, 30));
        ui.show_hud(hud);
        CHECK(ui.is_hud(hud) && ui.is_shown(count) && !ui.wants_keyboard());
        throws([&] { ui.push_screen(hud); });

        auto menu = ui.add_panel(u::none, u::layout().column().size(300, 200));
        auto quit = ui.add_button(menu, "Quit", u::layout().size(120, 40).shrink(0));
        auto play = ui.add_button(menu, "Play", u::layout().size(120, 40).shrink(0));
        ui.set_initial_focus(play);
        ui.push_screen(menu, true);
        CHECK(ui.focus() == play);
        auto enter = u::default_input();
        enter.confirm = true;
        ui.frame(window, enter);
        auto events = ui.events();
        CHECK(events.size() == 1 && events[0].kind == u::event_kind::clicked && events[0].node == play);
        ui.set_focus(quit);
        CHECK(ui.focus() == quit);
        throws([&] { ui.set_focus(count); });
        ui.clear_focus();
        CHECK(!ui.focus());

        auto escape = u::default_input();
        escape.cancel = true;
        ui.frame(window, escape);
        CHECK(!ui.is_shown(play) && ui.is_shown(count));
        ui.hide_hud(hud);
        CHECK(!ui.is_hud(hud) && !ui.is_shown(count));

        auto floor = ctx.load_collision_mesh(path.c_str());
        CHECK(ctx.load_collision_mesh(path.c_str()).id == floor.id);
        CHECK(ctx.load_collision_mesh(path.c_str(), {2, 1, 2}).id != floor.id);
        throws([&] { ctx.load_collision_mesh(path.c_str(), {1, 1, 1}, true); });
        throws([&] { ctx.load_collision_mesh("no/such.glb"); });
    });
    std::filesystem::remove(file);
}

int main() {
    levels_and_tilemaps();
    abi_version_is_checked();
    declared_access();
    command_barriers();
    worlds();
    names_and_reflection();
    resources();
    data_files();
    described_schemas_reach_capabilities();
    scenes_and_prefabs();
    world_text();
    three_d();
    skeletons();
    lod_groups();
    morph_targets();
    sound3d();
    animators();
    animator_layers_and_blend_trees();
    headless_wrapper_calls();
    shader_files();
    inverse_kinematics();
    streaming();
    streamed_worlds();
    model_material_overrides();
    handle_lifecycle();
    asset_streaming();
    parallel_queries();
    change_detection();
    presence_filters();
    query_cache_hints();
    wide_queries();
    presence_filters_allocate_nothing();
    games();
    attachments();
    attachments3d();
    physics();
    joints3d();
    vehicles();
    phases();
    plugins_setups_and_system_ids();
    lockstep();
    messages();
    replication();
    navigation();
    navigation_carving();
    particles3d();
    particles2d();
    ragdolls();
    contacts3d();
    ribbons_and_decals();
    retained_ui();
    retained_ui_presets();
    retained_ui_access();
    ui_panels();
    events();
    profiling();
    rng_matches_rust();
    grid_views_match_engine_queries();
    native_coverage();
    wrapper_coverage();
    headless_frames_ui();
    remaining_bindings();
    focus_huds_and_collision_files();
    std::cout << "C++ wrapper tests passed\n";
}
