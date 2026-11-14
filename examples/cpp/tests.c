#include "ponko.h"
#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#define CHECK(x) do { if (!(x)) { fprintf(stderr, "failed: %s\n", #x); abort(); } } while (0)
typedef struct { int x; } Position;
typedef struct { PonkoWorld* world; size_t seen; uint64_t entity; } Context;
static void visit(const PonkoChunk* chunk, void* user) {
    Context* c = user;
    CHECK(chunk->column_count == 1);
    for (size_t i = 0; i < chunk->count; ++i) {
        CHECK(ponko_entity_bits(chunk->entities[i]) == c->entity);
        ((Position*)chunk->columns[0])[i].x += 2;
        ++c->seen;
    }
    size_t len;
    CHECK(ponko_world_len(c->world, &len) == PONKO_BUSY);
    CHECK(ponko_world_free(c->world) == PONKO_BUSY);
}
typedef struct { size_t ticks; float dt; PonkoStatus busy_quit; } Run;
static void setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoGpuBuffer buffer;
    const float items[2][4] = {{1, 2, 3, 4}, {0, 0, 0, 0}};
    CHECK(ponko_ctx_create_gpu_buffer(ctx, 2, items, 2, &buffer) == PONKO_OK);
    CHECK(ponko_ctx_write_gpu_buffer(ctx, buffer, 1, items, 1) == PONKO_OK);
    CHECK(ponko_ctx_write_gpu_buffer(ctx, buffer, 2, items, 1) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_create_gpu_buffer(ctx, 0, NULL, 0, &buffer) == PONKO_INVALID_ARGUMENT);
    PonkoMaterial batch_material;
    PonkoMaterialDesc batch_desc = ponko_material_desc((PonkoColor){1,1,1,1});
    CHECK(ponko_ctx_create_material(ctx, &batch_desc, &batch_material) == PONKO_OK);
    PonkoMeshData* batch_data = ponko_mesh_data_cube(1.0f);
    PonkoMesh batch_mesh;
    CHECK(ponko_ctx_create_mesh(ctx, batch_data, batch_material, &batch_mesh) == PONKO_OK);
    ponko_mesh_data_free(batch_data);
    const float matrices[1][16] = {{1,0,0,0,0,1,0,0,0,0,1,0,0,0,0,1}};
    PonkoInstanceBatch batch;
    CHECK(ponko_ctx_create_instance_batch(ctx,batch_mesh,matrices,1,(PonkoInstanceBatchOptions){32,100,true,{1,1,1,1}},&batch) == PONKO_OK);
    CHECK(ponko_ctx_update_instance_batch(ctx,batch,matrices,1) == PONKO_OK);
    for (uint32_t mode=0; mode<3; ++mode) CHECK(ponko_ctx_set_instance_batch_culling(ctx,batch,mode) == PONKO_OK);
    CHECK(ponko_ctx_set_instance_batch_culling(ctx,batch,3) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_free_instance_batch(ctx,batch) == PONKO_OK);
    CHECK(ponko_ctx_set_instance_batch_culling(ctx,batch,0) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_update_instance_batch(ctx,batch,matrices,1) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_free_gpu_buffer(ctx, buffer) == PONKO_OK);
    CHECK(ponko_ctx_free_gpu_buffer(ctx, buffer) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_write_gpu_buffer(ctx, buffer, 0, items, 1) == PONKO_INVALID_ARGUMENT);
    /* One lifecycle for every handle kind: a kind and an id. */
    bool live = true;
    CHECK(ponko_ctx_handle_is_live(ctx, PONKO_HANDLE_GPU_BUFFER, buffer.id, &live) == PONKO_OK && !live);
    CHECK(ponko_ctx_handle_is_live(ctx, PONKO_HANDLE_MESH, batch_mesh.id, &live) == PONKO_OK && live);
    PonkoLiveCounts counts;
    CHECK(ponko_ctx_live_counts(ctx, &counts) == PONKO_OK && counts.gpu_buffers == 0);
    size_t meshes = counts.meshes;
    CHECK(ponko_ctx_handle_free(ctx, PONKO_HANDLE_MESH, batch_mesh.id) == PONKO_OK);
    CHECK(ponko_ctx_handle_free(ctx, PONKO_HANDLE_MESH, batch_mesh.id) == PONKO_OK);
    CHECK(ponko_ctx_handle_is_live(ctx, PONKO_HANDLE_MESH, batch_mesh.id, &live) == PONKO_OK && !live);
    CHECK(ponko_ctx_live_counts(ctx, &counts) == PONKO_OK && counts.meshes == meshes - 1);
    CHECK(ponko_ctx_handle_is_live(ctx, PONKO_HANDLE_TEXTURE, PONKO_HANDLE_NONE, &live) == PONKO_OK && !live);
    CHECK(ponko_ctx_handle_free(ctx, PONKO_HANDLE_MATERIAL, PONKO_HANDLE_NONE) == PONKO_OK);
    CHECK(ponko_ctx_handle_free(ctx, 99, 0) == PONKO_INVALID_ARGUMENT);
    /* Asset accounting: counts and bytes by class, each handle's age and owner. */
    PonkoAssetCount textures_before, textures_after;
    CHECK(ponko_ctx_asset_count(ctx, PONKO_ASSET_CLASS_TEXTURE, &textures_before) == PONKO_OK);
    const uint8_t pixels[2 * 2 * 4] = {0};
    PonkoTexture made;
    CHECK(ponko_ctx_create_texture(ctx, 2, 2, pixels, sizeof pixels, &made) == PONKO_OK);
    CHECK(ponko_ctx_asset_count(ctx, PONKO_ASSET_CLASS_TEXTURE, &textures_after) == PONKO_OK);
    CHECK(textures_after.live == textures_before.live + 1 && textures_after.bytes == textures_before.bytes + 16);
    size_t record_count = 0;
    CHECK(ponko_ctx_asset_handles(ctx, PONKO_ASSET_CLASS_TEXTURE, NULL, 0, &record_count) == PONKO_OK);
    CHECK(record_count == textures_after.live);
    PonkoAssetRecord records[64];
    CHECK(record_count <= 64);
    CHECK(ponko_ctx_asset_handles(ctx, PONKO_ASSET_CLASS_TEXTURE, records, 64, &record_count) == PONKO_OK);
    bool found_made = false;
    for (size_t i = 0; i < record_count; i++) {
        if (records[i].id == made.id) {
            found_made = records[i].owner == PONKO_ASSET_OWNER_CODE && records[i].bytes == 16;
        }
    }
    CHECK(found_made);
    size_t owner_len = 99;
    CHECK(ponko_ctx_asset_owner(ctx, PONKO_ASSET_CLASS_TEXTURE, made.id, NULL, 0, &owner_len) == PONKO_OK && owner_len == 0);
    CHECK(ponko_ctx_asset_count(ctx, 99, &textures_after) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_handle_free(ctx, PONKO_HANDLE_TEXTURE, made.id) == PONKO_OK);
    PonkoMesh house;
    CHECK(ponko_ctx_load_mesh(ctx, "models/house.glb", &house) == PONKO_OK);
    char owner[64];
    CHECK(ponko_ctx_asset_owner(ctx, PONKO_ASSET_CLASS_MESH, house.id, owner, sizeof owner, &owner_len) == PONKO_OK);
    CHECK(owner_len == strlen("models/house.glb") && strcmp(owner, "models/house.glb") == 0);
    bool unloaded = false;
    CHECK(ponko_ctx_unload_mesh(ctx, "models/house.glb", &unloaded) == PONKO_OK && unloaded);
    CHECK(ponko_ctx_handle_is_live(ctx, PONKO_HANDLE_MESH, house.id, &live) == PONKO_OK && !live);
    CHECK(ponko_ctx_unload_mesh(ctx, "models/house.glb", &unloaded) == PONKO_OK && !unloaded);
    bool culling = true;
    CHECK(ponko_ctx_occlusion_culling(ctx, &culling) == PONKO_OK && !culling);
    CHECK(ponko_ctx_set_occlusion_culling(ctx, true) == PONKO_OK);
    CHECK(ponko_ctx_occlusion_culling(ctx, &culling) == PONKO_OK && culling);
    CHECK(ponko_ctx_set_occlusion_culling(ctx, false) == PONKO_OK);
    CHECK(ponko_ctx_occlusion_culling(NULL, &culling) == PONKO_INVALID_ARGUMENT);
    PonkoLighting light;
    CHECK(ponko_ctx_lighting(ctx, &light) == PONKO_OK && !light.has_equator);
    light.has_equator = true;
    light.equator_color = (PonkoColor){0, 1, 0, 1};
    CHECK(ponko_ctx_set_lighting(ctx, &light) == PONKO_OK);
    PonkoLighting lit;
    CHECK(ponko_ctx_lighting(ctx, &lit) == PONKO_OK && lit.has_equator && lit.equator_color.g == 1);
    light.has_equator = false;
    CHECK(ponko_ctx_set_lighting(ctx, &light) == PONKO_OK);
    /* Shader status: what compiled, and where an error is in the game's WGSL. */
    size_t shaders_before = 0;
    CHECK(ponko_ctx_shader_status_count(ctx, &shaders_before) == PONKO_OK);
    PonkoShader broken_shader;
    CHECK(ponko_ctx_create_shader(ctx, "broken.wgsl", "// tint\nfn surface(s: Surface) -> Surface {\n    return 1.0;\n}", &broken_shader) == PONKO_INVALID_ARGUMENT);
    size_t shaders = 0;
    CHECK(ponko_ctx_shader_status_count(ctx, &shaders) == PONKO_OK && shaders == shaders_before + 1);
    PonkoShaderStatus shader_status;
    CHECK(ponko_ctx_shader_status(ctx, shaders - 1, &shader_status) == PONKO_OK);
    CHECK(shader_status.kind == PONKO_SHADER_MATERIAL && shader_status.outcome == PONKO_SHADER_FAILED && !shader_status.ok);
    CHECK(shader_status.line == 3 && shader_status.column == 12 && !shader_status.in_engine_code);
    char shader_text[1024];
    size_t shader_len = 0;
    CHECK(ponko_ctx_shader_status_name(ctx, shaders - 1, shader_text, sizeof shader_text, &shader_len) == PONKO_OK);
    CHECK(strcmp(shader_text, "broken.wgsl") == 0 && shader_len == 11);
    CHECK(ponko_ctx_shader_status_text(ctx, shaders - 1, shader_text, sizeof shader_text, &shader_len) == PONKO_OK);
    CHECK(strncmp(shader_text, "broken.wgsl:3:12: error: ", 25) == 0);
    CHECK(ponko_ctx_shader_status_json(ctx, NULL, 0, &shader_len) == PONKO_OK && shader_len > 0);
    /* A schema described from C is what the capabilities answer reports. */
    PonkoWorld* described_world = ponko_ctx_world(ctx);
    uint32_t described = 0;
    CHECK(ponko_component_register(described_world, "DescribedInC", sizeof(float), _Alignof(float), &described) == PONKO_OK);
    PonkoField described_hp = {"hp", PONKO_FIELD_F32, 0};
    CHECK(ponko_component_reflect(described_world, described, &described_hp, 1) == PONKO_OK);
    CHECK(ponko_world_describe_component(described_world, "DescribedInC",
        "{\"Object\":[[\"hp\",\"Number\"],[\"team\",{\"Enum\":[\"MadeInC\"]}]]}") == PONKO_OK);
    size_t caps_len = 0;
    CHECK(ponko_ctx_capabilities_json(ctx, NULL, 0, &caps_len) == PONKO_OK && caps_len > 0);
    char* caps = malloc(caps_len + 1);
    CHECK(caps != NULL);
    CHECK(ponko_ctx_capabilities_json(ctx, caps, caps_len + 1, &caps_len) == PONKO_OK);
    CHECK(strstr(caps, "\"DescribedInC\"") != NULL && strstr(caps, "\"MadeInC\"") != NULL);
    free(caps);
    CHECK(ponko_ctx_capabilities_json(NULL, NULL, 0, &caps_len) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_shader_status(ctx, shaders, &shader_status) == PONKO_INVALID_ARGUMENT);
    /* Global illumination: off by default, its settings checked when on. */
    CHECK(!light.gi_enabled && light.gi_spacing == 2 && light.gi_probes_per_frame == 4);
    light.gi_enabled = true;
    light.gi_probes_per_frame = 0;
    CHECK(ponko_ctx_set_lighting(ctx, &light) == PONKO_INVALID_ARGUMENT);
    light.gi_probes_per_frame = 8;
    CHECK(ponko_ctx_set_lighting(ctx, &light) == PONKO_OK);
    CHECK(ponko_ctx_lighting(ctx, &lit) == PONKO_OK && lit.gi_enabled && lit.gi_probes_per_frame == 8);
    light.gi_enabled = false;
    CHECK(ponko_ctx_set_lighting(ctx, &light) == PONKO_OK);
    PonkoPost post;
    CHECK(ponko_ctx_post(ctx, &post) == PONKO_OK);
    CHECK(!post.ao_enabled && post.anti_aliasing == 0 && post.ao_radius == 1);
    post.ao_enabled = true; post.ao_half_resolution = true; post.ao_temporal = true; post.ao_radius = 2; post.ao_intensity = 1.5f;
    post.ao_slices = 4; post.ao_steps = 8; post.anti_aliasing = 1;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_OK);
    PonkoPost back;
    CHECK(ponko_ctx_post(ctx, &back) == PONKO_OK);
    CHECK(back.ao_half_resolution && back.ao_temporal && back.ao_enabled && back.ao_radius == 2 && back.anti_aliasing == 1);
    post.anti_aliasing = 4;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_INVALID_ARGUMENT);
    CHECK(back.render_scale == 1 && back.sharpen == 0);
    post.anti_aliasing = 1;
    post.render_scale = 0.1f;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_INVALID_ARGUMENT);
    post.render_scale = 0.5f; post.sharpen = 0.3f;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_OK);
    CHECK(ponko_ctx_post(ctx, &back) == PONKO_OK && back.render_scale == 0.5f && back.sharpen == 0.3f);
    PonkoGraphicsQuality quality, now;
    uint32_t level = 99;
    CHECK(ponko_graphics_quality_preset(4, &quality) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_graphics_quality_preset(0, &quality) == PONKO_OK);
    CHECK(quality.shadow_cascades == 2 && quality.light_shadow_views == 0 && quality.render_scale < 1);
    CHECK(quality.light_shadow_atlas == 1024);
    CHECK(ponko_graphics_quality_level(&quality, &level) == PONKO_OK && level == 0);
    CHECK(ponko_ctx_set_graphics_quality(ctx, &quality) == PONKO_OK);
    CHECK(ponko_ctx_graphics_quality(ctx, &now) == PONKO_OK && now.shadow_resolution == 1024);
    CHECK(ponko_ctx_post(ctx, &back) == PONKO_OK && back.render_scale == quality.render_scale);
    quality.shadow_resolution = 100;
    CHECK(ponko_ctx_set_graphics_quality(ctx, &quality) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_graphics_quality_preset(3, &quality) == PONKO_OK);
    CHECK(ponko_ctx_set_graphics_quality(ctx, &quality) == PONKO_OK);
    CHECK(ponko_ctx_graphics_quality(ctx, &now) == PONKO_OK && now.shadow_resolution == 4096 && now.ao_steps == 8 && now.shadow_refresh == 1 && now.shadow_filter == 3 && now.ssr == 1 && now.gi);
    CHECK(now.light_shadow_atlas == 8192 && now.light_shadow_views == 24);
    quality.light_shadow_atlas = 3000; /* not a power of two */
    CHECK(ponko_ctx_set_graphics_quality(ctx, &quality) == PONKO_INVALID_ARGUMENT);
    /* GPU tiers: reports and presets need no GPU; a headless ctx has none. */
    {
        size_t len = 0;
        bool on = true;
        uint32_t tier = 99;
        char small[8];
        PonkoGraphicsQuality web;
        CHECK(ponko_ctx_gpu_capabilities(ctx, NULL, 0, &len) == PONKO_NOT_FOUND);
        CHECK(ponko_ctx_gpu_tier(ctx, &tier) == PONKO_NOT_FOUND && tier == 99);
        CHECK(ponko_ctx_gpu_feature(ctx, PONKO_GPU_FEATURE_DECALS, &on) == PONKO_NOT_FOUND);
        CHECK(ponko_ctx_recommended_graphics_quality(ctx, &web) == PONKO_NOT_FOUND);
        CHECK(ponko_gpu_tier_report(PONKO_GPU_TIER_BASELINE, NULL, 0, &len) == PONKO_OK && len > 100);
        CHECK(ponko_gpu_tier_report(PONKO_GPU_TIER_BASELINE, small, sizeof small, &len) == PONKO_OK && small[0] == 0);
        CHECK(ponko_gpu_tier_report(3, NULL, 0, &len) == PONKO_INVALID_ARGUMENT);
        CHECK(ponko_gpu_tier_feature(PONKO_GPU_TIER_WEBGL2, PONKO_GPU_FEATURE_TEMPORAL_ANTIALIASING, &on) == PONKO_OK && !on);
        CHECK(ponko_gpu_tier_feature(PONKO_GPU_TIER_BASELINE, PONKO_GPU_FEATURE_TEMPORAL_ANTIALIASING, &on) == PONKO_OK && on);
        CHECK(ponko_gpu_tier_feature(PONKO_GPU_TIER_BASELINE, PONKO_GPU_FEATURE_DECALS, &on) == PONKO_OK && !on);
        CHECK(ponko_gpu_tier_feature(PONKO_GPU_TIER_DESKTOP, PONKO_GPU_FEATURE_GLOBAL_ILLUMINATION, &on) == PONKO_OK && on);
        CHECK(ponko_gpu_tier_feature(PONKO_GPU_TIER_DESKTOP, 13, &on) == PONKO_INVALID_ARGUMENT);
        CHECK(ponko_graphics_quality_for_tier(PONKO_GPU_TIER_WEBGL2, &web) == PONKO_OK);
        CHECK(web.anti_aliasing == 1 && !web.ambient_occlusion && web.ssr == 0 && !web.gi && web.shadow_cascades == 2);
        CHECK(ponko_graphics_quality_for_tier(PONKO_GPU_TIER_DESKTOP, &web) == PONKO_OK);
        CHECK(ponko_graphics_quality_level(&web, &level) == PONKO_OK && level == 2);
        CHECK(ponko_graphics_quality_for_tier(3, &web) == PONKO_INVALID_ARGUMENT);
    }
    post.anti_aliasing = 3; /* temporal */
    post.taa_sharpness = 2;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_INVALID_ARGUMENT);
    post.taa_sharpness = 0.5f;
    post.contact_shadows = -1;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_INVALID_ARGUMENT);
    post.contact_shadows = 0.3f;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_OK);
    CHECK(post.ssr == 0 && post.ssr_max_distance == 30); /* screen-space reflections: off */
    post.ssr = 2;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_INVALID_ARGUMENT);
    post.ssr = 0.5f;
    post.ssr_max_distance = 0;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_INVALID_ARGUMENT);
    post.ssr_max_distance = 20;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_OK);
    post.ao_enabled = true; post.ao_temporal = true;
    CHECK(ponko_ctx_set_post(ctx, &post) == PONKO_OK);
    CHECK(ponko_ctx_reset_temporal_history(ctx) == PONKO_OK);
    CHECK(ponko_ctx_post(ctx, &back) == PONKO_OK && back.anti_aliasing == 3 && back.taa_sharpness == 0.5f && back.contact_shadows == 0.3f && back.ssr == 0.5f && back.ssr_max_distance == 20);
    uint64_t e;
    CHECK(ponko_spawn(ponko_ctx_world(ctx), NULL, 0, &e) == PONKO_OK);
    CHECK(ponko_ctx_reset_motion(ctx, e) == PONKO_OK);
    CHECK(ponko_world_free(ponko_ctx_world(ctx)) == PONKO_BUSY);
    PonkoText3D t = ponko_text3d_default(), got;
    t.billboard = true; t.align = 2;
    CHECK(ponko_ctx_set_text3d(ctx, e, "Hello\nworld", &t) == PONKO_OK);
    CHECK(ponko_ctx_text3d(ctx, e, &got) == PONKO_OK && got.billboard && got.align == 2);
    char text[32]; size_t len;
    CHECK(ponko_ctx_text3d_text(ctx, e, text, sizeof(text), &len) == PONKO_OK);
    CHECK(len == 11 && strcmp(text, "Hello\nworld") == 0);
    t.font.id = UINT32_MAX;
    CHECK(ponko_ctx_set_text3d(ctx, e, "bad", &t) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_remove_text3d(ctx, e) == PONKO_OK);
    CHECK(ponko_ctx_text3d(ctx, e, &got) == PONKO_NOT_FOUND);

    CHECK(ponko_ctx_set_nav_avoidance(ctx, e, true, 2, 3.6f) == PONKO_NOT_FOUND);
    CHECK(ponko_ctx_set_nav_agent(ctx, e, 3, 0.3f, 10) == PONKO_OK);
    PonkoVec3 center = {0, 1, 0}, half = {1, 1, 1};
    CHECK(!ponko_nav_bake_settings_default().carving && ponko_nav_bake_settings_default().tile_size == 32);
    CHECK(ponko_ctx_set_nav_carve_obstacle(ctx, e, center, half) == PONKO_OK);
    size_t steps = 0; bool pending = false;
    CHECK(ponko_ctx_navmesh_update_carving(ctx, 0, &steps, &pending) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_set_nav_avoidance(ctx, e, true, 2, 3.6f) == PONKO_OK);
    CHECK(ponko_ctx_set_nav_avoidance(ctx, e, false, 0, 1) == PONKO_OK);
    CHECK(ponko_ctx_set_nav_avoidance(ctx, e, true, -1, 3) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_set_nav_avoidance(ctx, e, true, 2, 0) == PONKO_INVALID_ARGUMENT);
}
static void tick(PonkoCtx* ctx, void* user) {
    Run* r = user;
    size_t len;
    CHECK(ponko_world_len(ponko_ctx_world(ctx), &len) == PONKO_OK && len == 1);
    r->dt = ponko_ctx_dt(ctx);
    if (++r->ticks == 3) CHECK(ponko_ctx_quit(ctx) == PONKO_OK);
}
/* A box car on four raycast wheels: spawned and driven through the C API,
   the vehicle as a query column. */
typedef struct { uint32_t transform, body, collider, vehicle; uint64_t car; } Cars;
static void ids(PonkoWorld* w, Cars* c) {
    CHECK(ponko_transform3d_component(w, &c->transform) == PONKO_OK);
    CHECK(ponko_rigid_body3d_component(w, &c->body) == PONKO_OK);
    CHECK(ponko_collider3d_component(w, &c->collider) == PONKO_OK);
    CHECK(ponko_vehicle3d_component(w, &c->vehicle) == PONKO_OK);
}
static void car_setup(PonkoCtx* ctx, void* user) {
    Cars* c = user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    ids(w, c);
    PonkoTransform3D floor_at = ponko_transform3d_at((PonkoVec3){0, -0.5f, 0});
    PonkoRigidBody3D fixed = ponko_rigid_body3d(PONKO_BODY_FIXED);
    PonkoCollider3D floor = ponko_collider3d_box((PonkoVec3){200, 1, 200});
    PonkoValue ground[] = {{c->transform, &floor_at, sizeof floor_at}, {c->body, &fixed, sizeof fixed},
                            {c->collider, &floor, sizeof floor}};
    uint64_t e; CHECK(ponko_spawn(w, ground, 3, &e) == PONKO_OK);
    PonkoTransform3D at = ponko_transform3d_at((PonkoVec3){0, 0.8f, 0});
    PonkoRigidBody3D chassis = ponko_rigid_body3d(PONKO_BODY_DYNAMIC);
    chassis.mass = 1200;
    chassis.has_center_of_mass = true;
    chassis.center_of_mass = (PonkoVec3){0, -0.3f, 0};
    PonkoCollider3D box = ponko_collider3d_box((PonkoVec3){1.8f, 0.6f, 4.2f});
    PonkoVehicle3D car = ponko_vehicle3d();
    const float x[] = {-0.8f, 0.8f, -0.8f, 0.8f}, z[] = {-1.3f, -1.3f, 1.3f, 1.3f};
    for (int i = 0; i < 4; ++i)
        CHECK(ponko_vehicle3d_add_wheel(&car, ponko_wheel3d((PonkoVec3){x[i], -0.3f, z[i]}, 0.35f)) == PONKO_OK);
    PonkoValue values[] = {{c->transform, &at, sizeof at}, {c->body, &chassis, sizeof chassis},
                            {c->collider, &box, sizeof box}, {c->vehicle, &car, sizeof car}};
    CHECK(ponko_spawn(w, values, 4, &c->car) == PONKO_OK);
}
static void throttle(const PonkoChunk* chunk, void* user) {
    (void)user;
    PonkoVehicle3D* cars = chunk->columns[0];
    for (size_t i = 0; i < chunk->count; ++i) {
        cars[i].wheels[2].motor_torque = 500;
        cars[i].wheels[3].motor_torque = 500;
    }
}
static void drive(PonkoCtx* ctx, void* user) {
    Cars* c = user;
    if (ponko_ctx_sim_tick(ctx) > 25) CHECK(ponko_query(ponko_ctx_world(ctx), &c->vehicle, 1, throttle, NULL) == PONKO_OK);
}
static void vehicles(void) {
    Cars c = {0};
    PonkoGame* game = ponko_game_new("c vehicles", 1, 1); CHECK(game);
    CHECK(ponko_game_sim_system(game, "drive", 1, drive, &c) == PONKO_OK);
    CHECK(ponko_game_physics3d(game, (PonkoVec3){0, -9.81f, 0}) == PONKO_OK);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 150, car_setup, &c, &w) == PONKO_OK);
    PonkoVehicle3D car; PonkoTransform3D t;
    CHECK(ponko_get(w, c.car, c.vehicle, &car, sizeof car) == PONKO_OK);
    CHECK(ponko_get(w, c.car, c.transform, &t, sizeof t) == PONKO_OK);
    CHECK(car.wheel_count == 4 && car.wheels[0].grounded && car.wheels[3].angular_velocity > 0);
    CHECK(t.position.z < -3 && t.position.y > 0.7f && t.position.y < 0.9f);
    PonkoTransform3D wheel = ponko_wheel3d_pose(&car.wheels[0], t);
    CHECK(wheel.position.y > 0.3f && wheel.position.y < 0.4f);
    CHECK(ponko_friction_curve_evaluate(ponko_friction_curve_sideways(), 0.2f) == 1);
    CHECK(ponko_world_free(w) == PONKO_OK);
}
/* Phases: a BEFORE_PHYSICS system feeds this tick's step; a sim system,
   even one added before physics3d, runs after the step and sees its result. */
typedef struct { uint32_t transform, velocity; uint64_t ball; float seen_before, seen_after; } Kick;
static void kick_setup(PonkoCtx* ctx, void* user) {
    Kick* k = user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    uint32_t body, collider;
    CHECK(ponko_transform3d_component(w, &k->transform) == PONKO_OK);
    CHECK(ponko_velocity3d_component(w, &k->velocity) == PONKO_OK);
    CHECK(ponko_rigid_body3d_component(w, &body) == PONKO_OK);
    CHECK(ponko_collider3d_component(w, &collider) == PONKO_OK);
    PonkoTransform3D at = ponko_transform3d_at((PonkoVec3){0, 0, 0});
    PonkoRigidBody3D rb = ponko_rigid_body3d(PONKO_BODY_DYNAMIC);
    PonkoCollider3D sphere = ponko_collider3d_sphere(0.5f);
    PonkoVelocity3D still = {{0, 0, 0}, {0, 0, 0}};
    PonkoValue values[] = {{k->transform, &at, sizeof at}, {body, &rb, sizeof rb},
                            {collider, &sphere, sizeof sphere}, {k->velocity, &still, sizeof still}};
    CHECK(ponko_spawn(w, values, 4, &k->ball) == PONKO_OK);
}
static void kick(PonkoCtx* ctx, void* user) {
    Kick* k = user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    PonkoTransform3D t; CHECK(ponko_get(w, k->ball, k->transform, &t, sizeof t) == PONKO_OK);
    k->seen_before = t.position.x;
    PonkoVelocity3D v = {{5, 0, 0}, {0, 0, 0}};
    CHECK(ponko_set(w, k->ball, k->velocity, &v, sizeof v) == PONKO_OK);
}
static void watch(PonkoCtx* ctx, void* user) {
    Kick* k = user;
    PonkoTransform3D t; CHECK(ponko_get(ponko_ctx_world(ctx), k->ball, k->transform, &t, sizeof t) == PONKO_OK);
    k->seen_after = t.position.x;
}
static void phases(void) {
    Kick k = {0};
    PonkoGame* game = ponko_game_new("c phases", 1, 1); CHECK(game);
    CHECK(ponko_game_fixed_hz(game, 50) == PONKO_OK);
    CHECK(ponko_game_sim_system(game, "watch", 1, watch, &k) == PONKO_OK);
    CHECK(ponko_game_add_system(game, PONKO_PHASE_BEFORE_PHYSICS, "kick", 1, kick, &k) == PONKO_OK);
    CHECK(ponko_game_add_system(game, 99, "bad", 1, kick, &k) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_game_add_system(game, PONKO_PHASE_FRAME, "bad", 2, kick, &k) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_game_physics3d(game, (PonkoVec3){0, 0, 0}) == PONKO_OK);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 1, kick_setup, &k, &w) == PONKO_OK);
    CHECK(k.seen_before == 0);
    CHECK(fabsf(k.seen_after - 5.0f / 50.0f) < 1e-3f);
    CHECK(ponko_world_free(w) == PONKO_OK);
}
/* System ids, setup callbacks and plugins: each callback logs a letter. */
typedef struct { char text[32]; int n; } Order;
static void put(Order* o, char c) { if (o->n < 31) o->text[o->n++] = c; }
static void mark_a(PonkoCtx* ctx, void* user) { (void)ctx; put(user, 'a'); }
static void mark_b(PonkoCtx* ctx, void* user) { (void)ctx; put(user, 'b'); }
static void mark_s(PonkoCtx* ctx, void* user) { (void)ctx; put(user, 's'); }
static void mark_t(PonkoCtx* ctx, void* user) { (void)ctx; put(user, 't'); }
static void mark_r(PonkoCtx* ctx, void* user) { (void)ctx; put(user, 'r'); }
static void build_area(PonkoGame* game, void* user) {
    CHECK(ponko_game_add_system(game, PONKO_PHASE_BEFORE_PHYSICS, "area", 1, mark_b, user) == PONKO_OK);
}
static void plugins_setups_and_system_ids(void) {
    Order o = {0};
    PonkoGame* game = ponko_game_new("c plugins", 1, 1); CHECK(game);
    uint32_t first = 0, second = 0;
    CHECK(ponko_game_add_system_id(game, PONKO_PHASE_AFTER_PHYSICS, "same", 1, mark_a, &o, &first) == PONKO_OK);
    CHECK(ponko_game_add_system_id(game, PONKO_PHASE_AFTER_PHYSICS, "same", 1, mark_a, &o, &second) == PONKO_OK);
    CHECK(first != 0 && second != 0 && first != second);
    CHECK(ponko_game_remove(game, first) == PONKO_OK);
    CHECK(ponko_game_remove(game, first) == PONKO_OK);   /* twice is fine */
    CHECK(ponko_game_remove(game, PONKO_SYSTEM_DRAW_SPRITES) == PONKO_OK);
    CHECK(ponko_game_remove(game, PONKO_SYSTEM_UPDATE_AUDIO3D) == PONKO_OK);
    CHECK(ponko_game_remove_system(game, "ponko::ecs::draw_sprites") == PONKO_NOT_FOUND);
    CHECK(ponko_game_add_system_id(game, PONKO_PHASE_FRAME, "bad", 2, mark_a, &o, &first) == PONKO_INVALID_ARGUMENT);
    CHECK(first == 0);
    /* Setups and plugins start in the order they were added, before the run setup. */
    CHECK(ponko_game_setup(game, mark_s, &o) == PONKO_OK);
    CHECK(ponko_game_plugin(game, build_area, mark_t, &o) == PONKO_OK);
    CHECK(ponko_game_setup(game, NULL, &o) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_game_plugin(game, NULL, NULL, &o) == PONKO_INVALID_ARGUMENT);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 2, mark_r, &o, &w) == PONKO_OK);
    CHECK(strcmp(o.text, "strbaba") == 0);
    CHECK(ponko_world_free(w) == PONKO_OK);
}
/* The retained UI: a column with a button, laid out and clicked from C. */
static void ui_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoUiLayout column = ponko_ui_layout_default();
    column.flex_direction = PONKO_UI_FLEX_COLUMN;
    column.width.kind = PONKO_UI_LENGTH_POINTS; column.width.value = 300;
    column.height.kind = PONKO_UI_LENGTH_POINTS; column.height.value = 200;
    uint64_t root, button, label;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &column, &root) == PONKO_OK);
    PonkoUiLayout fixed = ponko_ui_layout_default();
    fixed.width.kind = PONKO_UI_LENGTH_POINTS; fixed.width.value = 120;
    fixed.height.kind = PONKO_UI_LENGTH_POINTS; fixed.height.value = 40;
    CHECK(ponko_ctx_ui_add_label(ctx, PONKO_UI_SCREEN, root, "Shop", &fixed, &label) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_button(ctx, PONKO_UI_SCREEN, root, "Buy", &fixed, &button) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, root, false) == PONKO_OK);
    PonkoVec2 window = {800, 600};
    CHECK(ponko_ctx_ui_frame(ctx, PONKO_UI_SCREEN, window, NULL) == PONKO_OK);
    PonkoRect r;
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, button, &r) == PONKO_OK && r.min.y == 40 && r.max.x == 120);
    char text[8]; size_t len;
    CHECK(ponko_ctx_ui_text(ctx, PONKO_UI_SCREEN, button, text, sizeof text, &len) == PONKO_OK && len == 3 && text[0] == 'B');
    PonkoUiInput in = ponko_ui_input_default();
    in.pointer.x = 10; in.pointer.y = 50; in.pressed = true; in.held = true;
    CHECK(ponko_ctx_ui_frame(ctx, PONKO_UI_SCREEN, window, &in) == PONKO_OK);
    in.pressed = false; in.held = false; in.released = true;
    CHECK(ponko_ctx_ui_frame(ctx, PONKO_UI_SCREEN, window, &in) == PONKO_OK);
    PonkoUiEvent event;
    CHECK(ponko_ctx_ui_next_event(ctx, PONKO_UI_SCREEN, &event) == PONKO_OK);
    CHECK(event.kind == PONKO_UI_EVENT_CLICKED && event.id == button);
    CHECK(ponko_ctx_ui_next_event(ctx, PONKO_UI_SCREEN, &event) == PONKO_NOT_FOUND);
    /* The original flat track arrays remain valid; templates add repeat fragments. */
    PonkoUiTrack repeated_track = {PONKO_UI_TRACK_FR, 1};
    PonkoUiTemplateTrack repeated = {0};
    repeated.repetition = 1; repeated.count = 2;
    repeated.tracks = &repeated_track; repeated.track_count = 1;
    PonkoUiTrack rows[] = {{PONKO_UI_TRACK_POINTS, 40}, {PONKO_UI_TRACK_POINTS, 160}};
    PonkoUiLayout named = ponko_ui_layout_default();
    named.display = PONKO_UI_DISPLAY_GRID;
    named.width = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 300};
    named.height = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 200};
    named.grid_template_columns = &repeated; named.grid_template_column_count = 1;
    named.grid_rows = rows; named.grid_row_count = 2;
    uint64_t grid, main;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &named, &grid) == PONKO_OK);
    const char* areas[] = {"header header", "side main"};
    CHECK(ponko_ctx_ui_set_grid_template_areas(ctx, PONKO_UI_SCREEN, grid, areas, 2) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, grid, NULL, &main) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_grid_area(ctx, PONKO_UI_SCREEN, main, "main") == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, grid, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_layout(ctx, PONKO_UI_SCREEN, (PonkoVec2){800, 600}) == PONKO_OK);
    PonkoRect main_rect;
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, main, &main_rect) == PONKO_OK);
    CHECK(main_rect.min.x == 150 && main_rect.min.y == 40 && main_rect.max.x == 300 && main_rect.max.y == 200);
    const char* invalid_areas[] = {"x x", "x y"};
    CHECK(ponko_ctx_ui_set_grid_template_areas(ctx, PONKO_UI_SCREEN, grid, invalid_areas, 2) == PONKO_INVALID_ARGUMENT);
    const char* names[] = {"start", "finish"};
    CHECK(ponko_ctx_ui_set_grid_line_names(ctx, PONKO_UI_SCREEN, grid, 0, names, 2) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_grid_named_line(ctx, PONKO_UI_SCREEN, main, 0, false, "start", 1) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_grid_named_line(ctx, PONKO_UI_SCREEN, main, 0, true, "finish", 1) == PONKO_OK);
    CHECK(ponko_ctx_ui_layout(ctx, PONKO_UI_SCREEN, (PonkoVec2){800, 600}) == PONKO_OK);
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, main, &main_rect) == PONKO_OK);
    CHECK(main_rect.min.x == 0 && main_rect.max.x == 300);
    column.display = 99; /* not a PonkoUiDisplay */
    uint64_t bad;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, root, &column, &bad) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_ui_remove(ctx, PONKO_UI_SCREEN, root) == PONKO_OK);
    CHECK(ponko_ctx_ui_remove(ctx, PONKO_UI_SCREEN, button) == PONKO_NOT_FOUND);
}
/* A world-space UI panel: its UI takes the same calls, with the entity as the target. */
static void panel_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    uint32_t transform; uint64_t entity;
    CHECK(ponko_transform3d_component(w, &transform) == PONKO_OK);
    PonkoTransform3D at = ponko_transform3d_at((PonkoVec3){0, 1.5f, -2});
    PonkoValue value = {transform, &at, sizeof at};
    CHECK(ponko_spawn(w, &value, 1, &entity) == PONKO_OK);
    PonkoTexture texture;
    CHECK(ponko_ctx_create_ui_panel(ctx, entity, 600, 400, (PonkoVec2){1.2f, 0.8f}, &texture) == PONKO_OK);
    CHECK(texture.width == 600 && texture.height == 400);
    PonkoUiLayout fixed = ponko_ui_layout_default();
    fixed.width.kind = PONKO_UI_LENGTH_POINTS; fixed.width.value = 200;
    fixed.height.kind = PONKO_UI_LENGTH_POINTS; fixed.height.value = 60;
    uint64_t button, top;
    CHECK(ponko_ctx_ui_add_button(ctx, entity, PONKO_UI_NONE, "Buy", &fixed, &button) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, entity, button, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_top_screen(ctx, PONKO_UI_SCREEN, &top) == PONKO_OK && top == PONKO_UI_NONE);
    CHECK(ponko_ctx_ui_top_screen(ctx, entity, &top) == PONKO_OK && top == button);
    PonkoVec2 pixels = {600, 400};
    CHECK(ponko_ctx_ui_layout(ctx, entity, pixels) == PONKO_OK);
    PonkoRect r;
    CHECK(ponko_ctx_ui_rect(ctx, entity, button, &r) == PONKO_OK && r.max.x == 200 && r.max.y == 60);
    bool over; PonkoVec2 pixel;
    CHECK(ponko_ctx_ui_panel_pointer(ctx, entity, &over, &pixel) == PONKO_OK && !over);
    CHECK(ponko_ctx_set_ui_panel_interactive(ctx, entity, false) == PONKO_OK);
    CHECK(ponko_ctx_set_ui_pointer_ray(ctx, (PonkoVec3){0, 1.5f, 0}, (PonkoVec3){0, 0, -1}) == PONKO_OK);
    CHECK(ponko_ctx_set_ui_pointer_ray(ctx, (PonkoVec3){0, 0, 0}, (PonkoVec3){0, 0, 0}) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_clear_ui_pointer_ray(ctx) == PONKO_OK);
    CHECK(ponko_ctx_remove_ui_panel(ctx, entity) == PONKO_OK);
    CHECK(ponko_ctx_ui_top_screen(ctx, entity, &top) == PONKO_NOT_FOUND);
}
static void ui_panels(void) {
    PonkoGame* game = ponko_game_new("c panels", 1, 1); CHECK(game);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 0, panel_setup, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
}
/* Layout presets, the theme, titles, names and take_click. */
static void ui_presets_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoUiLayout fill = ponko_ui_layout_fill();
    fill.padding.top = fill.padding.left = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 10};
    PonkoUiLayout row = ponko_ui_layout_row(40), fixed = ponko_ui_layout_fixed(100, 30);
    PonkoUiLayout column = ponko_ui_layout_column(120);
    PonkoUiLayout map = ponko_ui_layout_anchored(0.75f, 0, 0.25f, 0.25f);
    PonkoUiLayout dialog_layout = ponko_ui_layout_centered(200, 100);
    uint64_t root, title, buy, side, corner, dialog, found, clicked;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &fill, &root) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_title(ctx, PONKO_UI_SCREEN, root, "Shop", &row, &title) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_button(ctx, PONKO_UI_SCREEN, root, "Buy", &fixed, &buy) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, root, &column, &side) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, root, &map, &corner) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &dialog_layout, &dialog) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, root, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, dialog, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_layout(ctx, PONKO_UI_SCREEN, (PonkoVec2){800, 600}) == PONKO_OK);
    PonkoRect r;
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, title, &r) == PONKO_OK && r.min.y == 10 && r.max.y == 50);
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, buy, &r) == PONKO_OK && r.min.x == 10 && r.max.x == 110);
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, corner, &r) == PONKO_OK && r.min.x == 600 && r.max.y == 150);
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, dialog, &r) == PONKO_OK && r.min.x == 300 && r.min.y == 250);
    CHECK(ponko_ctx_ui_pop_screen(ctx, PONKO_UI_SCREEN, &found) == PONKO_OK && found == dialog);
    /* The theme: read, changed, followed by titles and the immediate widgets. */
    PonkoUiTheme theme = ponko_ui_theme_default();
    CHECK(ponko_ctx_ui_theme(ctx, PONKO_UI_SCREEN, &theme) == PONKO_OK && theme.title_size > theme.text_size);
    theme.title = (PonkoColor){1, 1, 0, 1};
    theme.title_size = 30;
    theme.accent = (PonkoColor){0, 1, 0, 1};
    CHECK(ponko_ctx_ui_set_theme(ctx, PONKO_UI_SCREEN, &theme) == PONKO_OK);
    PonkoUiPaint paint;
    CHECK(ponko_ctx_ui_paint(ctx, PONKO_UI_SCREEN, title, &paint) == PONKO_OK);
    CHECK(paint.font_size == 30 && paint.color.g == 1 && paint.color.b == 0);
    CHECK(ponko_ctx_ui_paint(ctx, PONKO_UI_SCREEN, buy, &paint) == PONKO_OK && paint.border.g == 1 && paint.border.r == 0);
    PonkoUiStyle style;
    CHECK(ponko_ctx_ui_style(ctx, &style) == PONKO_OK && style.accent.g == 1 && style.accent.r == 0);
    theme.text_size = -1;
    CHECK(ponko_ctx_ui_set_theme(ctx, PONKO_UI_SCREEN, &theme) == PONKO_INVALID_ARGUMENT);
    /* Names instead of kept ids. */
    CHECK(ponko_ctx_ui_set_name(ctx, PONKO_UI_SCREEN, buy, "shop.buy") == PONKO_OK);
    CHECK(ponko_ctx_ui_find(ctx, PONKO_UI_SCREEN, "shop.buy", &found) == PONKO_OK && found == buy);
    CHECK(ponko_ctx_ui_find(ctx, PONKO_UI_SCREEN, "shop.sell", &found) == PONKO_OK && found == PONKO_UI_NONE);
    char name[16]; size_t len;
    CHECK(ponko_ctx_ui_name(ctx, PONKO_UI_SCREEN, buy, name, sizeof name, &len) == PONKO_OK && len == 8 && strcmp(name, "shop.buy") == 0);
    /* take_click: the last click's node, once. */
    PonkoVec2 window = {800, 600};
    PonkoUiInput in = ponko_ui_input_default();
    in.pointer = (PonkoVec2){50, 65}; in.pressed = true; in.held = true;
    CHECK(ponko_ctx_ui_frame(ctx, PONKO_UI_SCREEN, window, &in) == PONKO_OK);
    in.pressed = false; in.held = false; in.released = true;
    CHECK(ponko_ctx_ui_frame(ctx, PONKO_UI_SCREEN, window, &in) == PONKO_OK);
    CHECK(ponko_ctx_ui_take_click(ctx, PONKO_UI_SCREEN, &clicked) == PONKO_OK && clicked == buy);
    CHECK(ponko_ctx_ui_take_click(ctx, PONKO_UI_SCREEN, &clicked) == PONKO_OK && clicked == PONKO_UI_NONE);
    CHECK(ponko_ctx_ui_remove(ctx, PONKO_UI_SCREEN, root) == PONKO_OK);
    CHECK(ponko_ctx_ui_find(ctx, PONKO_UI_SCREEN, "shop.buy", &found) == PONKO_OK && found == PONKO_UI_NONE);
}
/* A screen root's auto margins take the window's free space, as a child's do. */
static void ui_auto_margins_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoUiLength automatic = {PONKO_UI_LENGTH_AUTO, 0};
    PonkoUiLayout across = ponko_ui_layout_fixed(200, 100);
    across.margin.left = across.margin.right = automatic;
    across.margin.top = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 40};
    PonkoUiLayout middle = ponko_ui_layout_fixed(200, 100);
    middle.margin.left = middle.margin.right = middle.margin.top = middle.margin.bottom = automatic;
    PonkoUiLayout pushed = ponko_ui_layout_fixed(200, 100);
    pushed.margin.left = automatic;
    uint64_t a, m, p;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &across, &a) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &middle, &m) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &pushed, &p) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, a, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, m, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, p, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_layout(ctx, PONKO_UI_SCREEN, (PonkoVec2){800, 600}) == PONKO_OK);
    PonkoRect r;
    /* Centered across, 40 points down. */
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, a, &r) == PONKO_OK && r.min.x == 300 && r.min.y == 40 && r.max.x == 500);
    /* Auto on all four sides: the middle of the window. */
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, m, &r) == PONKO_OK && r.min.x == 300 && r.min.y == 250);
    /* Only the left margin auto: pushed to the right edge. */
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, p, &r) == PONKO_OK && r.min.x == 600 && r.max.x == 800 && r.min.y == 0);
    /* A bigger window moves them with it. */
    CHECK(ponko_ctx_ui_layout(ctx, PONKO_UI_SCREEN, (PonkoVec2){1000, 800}) == PONKO_OK);
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, m, &r) == PONKO_OK && r.min.x == 400 && r.min.y == 350);
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, p, &r) == PONKO_OK && r.min.x == 800);
}
/* What a screen reader reads of a UI, and what it can ask of it. */
static void ui_access_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    const uint64_t s = PONKO_UI_SCREEN;
    const PonkoVec2 window = {800, 600};
    PonkoUiLayout fill = ponko_ui_layout_fill(), row = ponko_ui_layout_row(30);
    uint64_t root, play, volume, name;
    CHECK(ponko_ctx_ui_add_panel(ctx, s, PONKO_UI_NONE, &fill, &root) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_button(ctx, s, root, "Play", &row, &play) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_slider(ctx, s, root, 0.5f, &row, &volume) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_text_input(ctx, s, root, "Ann", &row, &name) == PONKO_OK);
    CHECK(ponko_ctx_ui_push_screen(ctx, s, root, false) == PONKO_OK);
    /* Names, a description and a live region. */
    CHECK(ponko_ctx_ui_set_accessible_name(ctx, s, volume, "Volume") == PONKO_OK);
    CHECK(ponko_ctx_ui_set_accessible_description(ctx, s, play, "Starts a run") == PONKO_OK);
    CHECK(ponko_ctx_ui_set_live_region(ctx, s, play, PONKO_UI_LIVE_POLITE) == PONKO_OK);
    char text[512]; size_t len = 0;
    CHECK(ponko_ctx_ui_accessible_name(ctx, s, volume, text, sizeof text, &len) == PONKO_OK && strcmp(text, "Volume") == 0);
    CHECK(ponko_ctx_ui_accessible_description(ctx, s, play, text, sizeof text, &len) == PONKO_OK && len == 12);
    PonkoUiLive live = PONKO_UI_LIVE_OFF;
    CHECK(ponko_ctx_ui_live_region(ctx, s, play, &live) == PONKO_OK && live == PONKO_UI_LIVE_POLITE);
    CHECK(ponko_ctx_ui_frame(ctx, s, window, NULL) == PONKO_OK);
    CHECK(ponko_ctx_ui_access_text(ctx, s, text, sizeof text, &len) == PONKO_OK);
    CHECK(strstr(text, "group\n  button \"Play\" description=\"Starts a run\" live=polite focusable\n") == text);

    /* The snapshot: every node at first, then only what changed. */
    PonkoUiAccess* access = ponko_ui_access_new(); CHECK(access);
    PonkoUiAccessChanges changes;
    CHECK(ponko_ui_access_update(access, ctx, s, &changes) == PONKO_OK && changes.any && changes.changed_count == 4);
    size_t count = 0;
    CHECK(ponko_ui_access_count(access, &count) == PONKO_OK && count == 4);
    PonkoUiAccessNode node;
    CHECK(ponko_ui_access_node(access, 0, &node) == PONKO_OK && node.id == root && node.role == PONKO_UI_ROLE_GROUP && node.child_count == 3);
    uint64_t child = 0;
    CHECK(ponko_ui_access_child(access, root, 1, &child) == PONKO_OK && child == volume);
    CHECK(ponko_ui_access_find(access, volume, &node) == PONKO_OK && node.role == PONKO_UI_ROLE_SLIDER);
    CHECK(node.value_kind == PONKO_UI_ACCESS_VALUE_RANGE && node.value == 0.5f && node.focusable && !node.focused);
    CHECK(ponko_ui_access_find(access, name, &node) == PONKO_OK && node.has_selection && node.selection_caret == 3);
    CHECK(ponko_ui_access_name(access, volume, text, sizeof text, &len) == PONKO_OK && strcmp(text, "Volume") == 0);
    CHECK(ponko_ui_access_description(access, play, text, sizeof text, &len) == PONKO_OK && len == 12);
    CHECK(ponko_ui_access_value_text(access, name, text, sizeof text, &len) == PONKO_OK && strcmp(text, "Ann") == 0);
    uint64_t focus = 0;
    CHECK(ponko_ui_access_focus(access, &focus) == PONKO_OK && focus == PONKO_UI_NONE);
    CHECK(ponko_ui_access_update(access, ctx, s, &changes) == PONKO_OK && !changes.any);

    /* A request does what the key does: the event, the value, the focus. */
    CHECK(ponko_ctx_ui_access_action(ctx, s, PONKO_UI_ACCESS_ACTION_ACTIVATE, play, 0, NULL) == PONKO_OK);
    CHECK(ponko_ctx_ui_access_action(ctx, s, PONKO_UI_ACCESS_ACTION_INCREMENT, volume, 0, NULL) == PONKO_OK);
    CHECK(ponko_ctx_ui_access_action(ctx, s, PONKO_UI_ACCESS_ACTION_SET_TEXT, name, 0, "Bea") == PONKO_OK);
    CHECK(ponko_ctx_ui_frame(ctx, s, window, NULL) == PONKO_OK);
    PonkoUiEvent event;
    CHECK(ponko_ctx_ui_next_event(ctx, s, &event) == PONKO_OK && event.kind == PONKO_UI_EVENT_CLICKED && event.id == play);
    CHECK(ponko_ui_access_update(access, ctx, s, &changes) == PONKO_OK && changes.any && changes.focus_changed);
    CHECK(changes.changed_count == 3 && changes.removed_count == 0);
    uint64_t changed = 0;
    CHECK(ponko_ui_access_changed(access, 0, &changed) == PONKO_OK && changed == play);
    CHECK(ponko_ui_access_focus(access, &focus) == PONKO_OK && focus == play);
    CHECK(ponko_ui_access_find(access, volume, &node) == PONKO_OK && node.value > 0.54f && node.value < 0.56f);
    CHECK(ponko_ui_access_value_text(access, name, text, sizeof text, &len) == PONKO_OK && strcmp(text, "Bea") == 0);

    /* A removed node is reported and no longer found. */
    CHECK(ponko_ctx_ui_remove(ctx, s, volume) == PONKO_OK);
    CHECK(ponko_ctx_ui_frame(ctx, s, window, NULL) == PONKO_OK);
    CHECK(ponko_ui_access_update(access, ctx, s, &changes) == PONKO_OK && changes.removed_count == 1);
    uint64_t removed = 0;
    CHECK(ponko_ui_access_removed(access, 0, &removed) == PONKO_OK && removed == volume);
    CHECK(ponko_ui_access_find(access, volume, &node) == PONKO_NOT_FOUND);
    CHECK(ponko_ui_access_reset(access) == PONKO_OK);
    CHECK(ponko_ui_access_update(access, ctx, s, &changes) == PONKO_OK && changes.changed_count == 3);
    CHECK(ponko_ctx_ui_set_live_region(ctx, s, play, 9) == PONKO_INVALID_ARGUMENT);
    ponko_ui_access_free(access);
}
static void retained_ui(void) {
    PonkoGame* game = ponko_game_new("c ui", 1, 1); CHECK(game);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 0, ui_setup, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    game = ponko_game_new("c ui presets", 1, 1); CHECK(game);
    CHECK(ponko_game_run_headless(game, 0, ui_presets_setup, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    game = ponko_game_new("c ui auto margins", 1, 1); CHECK(game);
    CHECK(ponko_game_run_headless(game, 0, ui_auto_margins_setup, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    game = ponko_game_new("c ui access", 1, 1); CHECK(game);
    CHECK(ponko_game_run_headless(game, 0, ui_access_setup, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
}
/* Ribbon and line defaults, and a decal cut from a floor. */
static void ribbons_and_decals(void) {
    PonkoTrail3D trail = ponko_trail3d_new(0.25f, 0.05f);
    CHECK(trail.lifetime == 0.25f && trail.width == 0.05f && trail.max_points == 64 && trail.emitting && !trail.has_texture);
    PonkoLine3D line = ponko_line3d_new(0.1f, (PonkoColor){1, 0, 0, 1});
    CHECK(line.width == 0.1f && line.point_count == 0 && line.points == NULL && line.texture_length == 0);
    PonkoMeshData* floor = ponko_mesh_data_plane(4);
    float down[16] = {0.5f, 0, 0, 0, 0, 0, -0.2f, 0, 0, 0.5f, 0, 0, 1, 0, 0, 1};
    PonkoMeshData* mark = ponko_mesh_data_decal(floor, down, 0.002f);
    CHECK(mark);
    PonkoMeshView view;
    CHECK(ponko_mesh_data_view(mark, &view) == PONKO_OK && view.index_count >= 3);
    ponko_mesh_data_free(mark);
    CHECK(ponko_mesh_data_decal(floor, NULL, 0.002f) == NULL);
    ponko_mesh_data_free(floor);
}
static void far_ground(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoMaterial ground; CHECK(ponko_ctx_default_material(ctx, &ground) == PONKO_OK);
    static float heights[33 * 33];
    for (int i = 0; i < 33 * 33; ++i) heights[i] = 1.0f;
    PonkoVec2 corner = {-256, -256};
    PonkoFarTerrain* far = ponko_far_terrain_new(heights, 33 * 33, 33, corner, 16, 64, ground); CHECK(far);
    CHECK(ponko_far_terrain_update(far, ctx) == PONKO_OK);
    PonkoCell cell = {0, 0}; bool cut = true; size_t meshes = 0;
    CHECK(ponko_far_terrain_is_cut(far, cell, &cut, &meshes) == PONKO_OK && !cut && meshes == 1);
    CHECK(ponko_far_terrain_cut(far, cell, true) == PONKO_OK);
    CHECK(ponko_far_terrain_update(far, ctx) == PONKO_OK);
    CHECK(ponko_far_terrain_is_cut(far, cell, &cut, NULL) == PONKO_OK && cut);
    CHECK(ponko_far_terrain_clear(far, ctx) == PONKO_OK);
    ponko_far_terrain_free(far);
    PonkoCollisionMeshData* data = ponko_collision_mesh_data_heightfield(heights, 33 * 33, 64); CHECK(data);
    PonkoCollisionMesh mesh;
    CHECK(ponko_ctx_physics3d_add_mesh(ctx, data, &mesh) == PONKO_OK);
    ponko_collision_mesh_data_free(data);
    PonkoTerrainCollider solid = ponko_terrain_collider(mesh);
    CHECK(solid.friction == .5f && solid.mesh.id == mesh.id);
    PonkoColliderFocus focus = ponko_collider_focus(100, 150);
    CHECK(focus.radius == 100 && focus.keep_radius == 150);
    CHECK(ponko_ctx_physics3d_stream_colliders(ctx, true, 4) == PONKO_OK);
    bool active = true;
    CHECK(ponko_ctx_physics3d_is_collider_active(ctx, 0, &active) == PONKO_OK && !active);
}

static void streamed_worlds(void) {
    PonkoGame* game = ponko_game_new("far", 1, 1); CHECK(game);
    CHECK(ponko_game_physics3d(game, (PonkoVec3){0, -9.81f, 0}) == PONKO_OK);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 0, far_ground, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    float flat[4] = {0, 0, 0, 0};
    PonkoMaterial none = {0};
    CHECK(!ponko_far_terrain_new(flat, 4, 2, (PonkoVec2){0, 0}, 1, 1.5f, none));
    CHECK(!ponko_collision_mesh_data_heightfield(flat, 3, 1));
}

static void recolor(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    uint32_t transform, model;
    CHECK(ponko_transform3d_component(w, &transform) == PONKO_OK && ponko_model_component(w, &model) == PONKO_OK);
    PonkoMaterialDesc look = ponko_material_desc((PonkoColor){1, 0.8f, 0.2f, 1});
    PonkoMaterial gold, wood;
    CHECK(ponko_ctx_create_material(ctx, &look, &gold) == PONKO_OK);
    CHECK(ponko_ctx_create_material(ctx, &look, &wood) == PONKO_OK);
    PonkoMeshData* cube = ponko_mesh_data_cube(1);
    PonkoMesh mesh;
    CHECK(ponko_ctx_create_plain_mesh(ctx, cube, &mesh) == PONKO_OK);
    CHECK(ponko_ctx_create_plain_mesh(ctx, NULL, &mesh) == PONKO_INVALID_ARGUMENT);
    ponko_mesh_data_free(cube);
    PonkoTransform3D t = ponko_transform3d_at((PonkoVec3){0, 0, 0});
    PonkoModel m = ponko_model(mesh);
    PonkoValue parts[] = {{transform, &t, sizeof t}, {model, &m, sizeof m}};
    uint64_t e;
    CHECK(ponko_spawn(w, parts, 2, &e) == PONKO_OK);

    PonkoMaterial found; bool has = true;
    CHECK(ponko_ctx_model_material(ctx, e, &found, &has) == PONKO_OK && !has);
    CHECK(ponko_ctx_set_model_material(ctx, e, gold, true) == PONKO_OK);
    CHECK(ponko_ctx_model_material(ctx, e, &found, &has) == PONKO_OK && has && found.id == gold.id);
    CHECK(ponko_ctx_set_model_material(ctx, e, (PonkoMaterial){99}, true) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_set_model_material(ctx, e, gold, false) == PONKO_OK);
    CHECK(ponko_ctx_model_material(ctx, e, &found, &has) == PONKO_OK && !has);

    uint32_t numbers[] = {3, 0, 3};
    PonkoMaterial materials[] = {gold, wood, wood};
    CHECK(ponko_ctx_set_model_part_materials(ctx, e, numbers, materials, 3) == PONKO_OK);
    uint32_t got_parts[2]; PonkoMaterial got[2]; size_t n = 0;
    CHECK(ponko_ctx_model_part_materials(ctx, e, NULL, NULL, 0, &n) == PONKO_OK && n == 2);
    CHECK(ponko_ctx_model_part_materials(ctx, e, got_parts, got, 2, &n) == PONKO_OK && n == 2);
    CHECK(got_parts[0] == 0 && got_parts[1] == 3 && got[0].id == wood.id && got[1].id == wood.id);
    CHECK(ponko_ctx_set_model_part_materials(ctx, e, numbers, NULL, 2) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_set_model_part_materials(ctx, e, NULL, NULL, 0) == PONKO_OK);
    CHECK(ponko_ctx_model_part_materials(ctx, e, NULL, NULL, 0, &n) == PONKO_OK && n == 0);
    // Left on for the frames after setup.
    CHECK(ponko_ctx_set_model_material(ctx, e, gold, true) == PONKO_OK);
    CHECK(ponko_ctx_set_model_part_materials(ctx, e, numbers, materials, 1) == PONKO_OK);
}
static void model_material_overrides(void) {
    PonkoGame* game = ponko_game_new("overrides", 64, 64); CHECK(game);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 2, recolor, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    CHECK(ponko_ctx_set_model_material(NULL, 0, (PonkoMaterial){0}, true) == PONKO_INVALID_ARGUMENT);
}

static void lod_helpers(void) {
    float fov = 60 * 3.14159265f / 180;
    float h = ponko_lod_screen_height_at(2, 10, fov);
    CHECK(h > 0.172f && h < 0.174f);
    float d = ponko_lod_distance_at(2, h, fov);
    CHECK(d > 9.99f && d < 10.01f);
    PonkoLodRenderer r = ponko_lod_renderer((PonkoMesh){3});
    CHECK(r.mesh.id == 3 && r.color.r == 1 && r.offset.scale.y == 1);
    PonkoLodLevel level = {0.1f, 0, &r, 1, false, NULL, 0};
    CHECK(ponko_ctx_set_lod_group(NULL, 0, 1, (PonkoVec3){0, 0, 0}, &level, 1) == PONKO_INVALID_ARGUMENT);
}
static void streaming_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    CHECK(ponko_ctx_set_deterministic_uploads(ctx, true) == PONKO_OK);
    CHECK(ponko_ctx_set_upload_budget(ctx, true, 1 << 20) == PONKO_OK);
    PonkoTexture t = {0};
    bool ready = true;
    CHECK(ponko_ctx_stream_texture(ctx, "crab.png", &t, &ready) == PONKO_OK && !ready);
    uint32_t state = 0;
    CHECK(ponko_ctx_asset_state(ctx, PONKO_RESIDENCY_TEXTURE, "crab.png", &state) == PONKO_OK);
    CHECK(state == PONKO_ASSET_LOADING);
    PonkoMesh m = {0};
    CHECK(ponko_ctx_stream_mesh(ctx, "models/house.glb", &m, &ready) == PONKO_OK && !ready);
    CHECK(ponko_ctx_finish_uploads(ctx) == PONKO_OK);
    CHECK(ponko_ctx_stream_texture(ctx, "crab.png", &t, &ready) == PONKO_OK && ready && t.width > 0);
    CHECK(ponko_ctx_asset_state(ctx, PONKO_RESIDENCY_TEXTURE, "crab.png", &state) == PONKO_OK);
    CHECK(state == PONKO_ASSET_RESIDENT);
    CHECK(ponko_ctx_stream_mesh(ctx, "models/house.glb", &m, &ready) == PONKO_OK && ready);
    PonkoUploadStats uploads;
    CHECK(ponko_ctx_upload_stats(ctx, &uploads) == PONKO_OK);
    CHECK(uploads.has_budget && uploads.budget == 1 << 20 && uploads.pending == 0 && uploads.total_uploads >= 2);
    /* Budgets: the mesh is pinned, so a zero budget can't evict it. */
    PonkoMesh pinned = {0};
    CHECK(ponko_ctx_resident_mesh(ctx, "models/house.glb", true, &pinned) == PONKO_OK && pinned.id == m.id);
    CHECK(ponko_ctx_set_residency_budget(ctx, PONKO_RESIDENCY_MESH, true, 0) == PONKO_OK);
    PonkoResidencyStats stats;
    CHECK(ponko_ctx_residency(ctx, PONKO_RESIDENCY_MESH, &stats) == PONKO_OK);
    CHECK(stats.has_budget && stats.resident == 1 && stats.pinned == 1 && stats.over_budget_bytes > 0);
    bool resident = false;
    CHECK(ponko_ctx_is_resident(ctx, PONKO_RESIDENCY_MESH, "models/house.glb", &resident) == PONKO_OK && resident);
    CHECK(ponko_ctx_unpin_resident(ctx, PONKO_RESIDENCY_MESH, "models/house.glb") == PONKO_OK);
    PonkoTexture crab = {0};
    CHECK(ponko_ctx_resident_texture(ctx, "crab.png", false, &crab) == PONKO_OK && crab.id == t.id);
    /* A missing file fails with a message. */
    CHECK(ponko_ctx_stream_texture(ctx, "nope.png", &t, &ready) == PONKO_OK && !ready);
    CHECK(ponko_ctx_finish_uploads(ctx) == PONKO_OK);
    CHECK(ponko_ctx_asset_state(ctx, PONKO_RESIDENCY_TEXTURE, "nope.png", &state) == PONKO_OK);
    CHECK(state == PONKO_ASSET_FAILED);
    char message[256];
    size_t len = 0;
    CHECK(ponko_ctx_asset_error(ctx, PONKO_RESIDENCY_TEXTURE, "nope.png", message, sizeof message, &len) == PONKO_OK);
    CHECK(len > 0 && strstr(message, "nope.png"));
    CHECK(ponko_ctx_asset_state(ctx, 7, "nope.png", &state) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_resident_texture(ctx, "nope.png", false, &crab) == PONKO_INVALID_ARGUMENT);
}

static void asset_streaming(void) {
    PonkoGame* game = ponko_game_new("streaming", 1, 1); CHECK(game);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 0, streaming_setup, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

static void steam_messages_validation(void) {
    PonkoNetMessages* endpoint = NULL;
    PonkoSteamLobby* lobby = NULL;
    CHECK(ponko_net_messages_steam_host(480, 0, &endpoint) == PONKO_INVALID_ARGUMENT && !endpoint);
    CHECK(ponko_net_messages_steam_join(480, 0, &endpoint) == PONKO_INVALID_ARGUMENT && !endpoint);
    CHECK(ponko_steam_lobby_create(480, 3, 2, &lobby) == PONKO_INVALID_ARGUMENT && !lobby);
    CHECK(ponko_steam_lobby_join(480, 0, &lobby) == PONKO_INVALID_ARGUMENT && !lobby);
    CHECK(ponko_net_messages_steam_host(480, 2, &endpoint) != PONKO_OK && !endpoint);
    CHECK(ponko_steam_lobby_create(480, 0, 2, &lobby) != PONKO_OK && !lobby);
    CHECK(ponko_steam_lobby_poll(NULL) != PONKO_OK);
    CHECK(ponko_steam_lobby_leave(NULL) != PONKO_OK);
    CHECK(ponko_steam_lobby_set_metadata(NULL, "version", "1") != PONKO_OK);
    ponko_steam_lobby_free(NULL);
}

static void morph_clip_api(void) {
    PonkoClip* clip = NULL;
    CHECK(ponko_clip_new("face", &clip) == PONKO_OK);
    float times[] = {0, 2}, values[] = {0, 1};
    CHECK(ponko_clip_set_morph_track(clip, "smile", times, values, 2, 1) == PONKO_OK);
    CHECK(ponko_clip_duration(clip) == 2);
    float cubic[] = {0, 0, 1, 1, 1, 0};
    CHECK(ponko_clip_set_morph_track(clip, "smile", times, cubic, 2, 2) == PONKO_OK);
    CHECK(ponko_clip_set_morph_track(clip, "smile", times, values, 2, 9) == PONKO_INVALID_ARGUMENT);
    ponko_clip_free(clip);
    PonkoMeshData* mesh = ponko_mesh_data_plane(1);
    PonkoVec3 deltas[4] = {{0, 1, 0}, {0, 1, 0}, {0, 1, 0}, {0, 1, 0}};
    CHECK(ponko_mesh_data_add_morph_target(mesh, "full", deltas, NULL, 4) == PONKO_OK);
    CHECK(ponko_mesh_data_add_morph_target(mesh, "half", deltas, NULL, 4) == PONKO_OK);
    CHECK(ponko_mesh_data_add_morph_frame(mesh, 0, 1, 0.5f) == PONKO_OK);
    CHECK(ponko_mesh_data_add_morph_frame(mesh, 0, 0, 1) == PONKO_OK);
    CHECK(ponko_mesh_data_add_morph_frame(mesh, 0, 0, 1) == PONKO_INVALID_ARGUMENT);
    ponko_mesh_data_free(mesh);
}
/* Replication helpers: sequence numbers, authority, snapshots, interest. */
static void replication(void) {
    CHECK(ponko_seq_newer(1, 0) && ponko_seq_newer(0, 65535) && !ponko_seq_newer(7, 7));
    CHECK(!ponko_seq_newer(65535, 0) && ponko_seq_diff(1, 65535) == 2 && ponko_seq_diff(65535, 1) == -2);
    PonkoUpdateStamp old = {0, 60000}, next = {1, 0};
    CHECK(ponko_stamp_newer(next, old) && !ponko_stamp_newer(old, next) && !ponko_stamp_newer(old, old));

    PonkoAuthority a = ponko_authority_new(0);
    uint16_t epoch = 99;
    bool changed = false;
    PonkoAuthorityCheck check;
    CHECK(ponko_authority_check(&a, 0, 0, &check) == PONKO_OK && check == PONKO_AUTHORITY_ACCEPT);
    CHECK(ponko_authority_handoff(&a, 1, &epoch) == PONKO_OK && epoch == 1 && a.owner == 1);
    CHECK(ponko_authority_check(&a, 0, 0, &check) == PONKO_OK && check == PONKO_AUTHORITY_STALE);
    CHECK(ponko_authority_check(&a, 0, 1, &check) == PONKO_OK && check == PONKO_AUTHORITY_WRONG_OWNER);
    CHECK(ponko_authority_check(&a, 1, 2, &check) == PONKO_OK && check == PONKO_AUTHORITY_AHEAD);
    CHECK(ponko_authority_adopt(&a, 2, 1, &changed) == PONKO_OK && !changed && a.owner == 1); /* race: lower wins */
    CHECK(ponko_authority_adopt(&a, 3, 2, &changed) == PONKO_OK && changed && a.owner == 3 && a.epoch == 2);
    CHECK(ponko_authority_adopt(NULL, 1, 1, &changed) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_authority_check(&a, 1, 1, NULL) == PONKO_INVALID_ARGUMENT);

    PonkoLatestWins* filter = ponko_latest_wins_new();
    bool ok = false;
    PonkoUpdateStamp s1 = {0, 1}, s3 = {0, 3};
    CHECK(ponko_latest_wins_accept(filter, 9, s3, &ok) == PONKO_OK && ok);
    CHECK(ponko_latest_wins_accept(filter, 9, s1, &ok) == PONKO_OK && !ok);
    CHECK(ponko_latest_wins_accept(filter, 9, s3, &ok) == PONKO_OK && !ok);
    CHECK(ponko_latest_wins_accept(filter, 10, s1, &ok) == PONKO_OK && ok && ponko_latest_wins_len(filter) == 2);
    CHECK(ponko_latest_wins_forget(filter, 9) == PONKO_OK);
    CHECK(ponko_latest_wins_accept(filter, 9, s1, &ok) == PONKO_OK && ok);
    CHECK(ponko_latest_wins_clear(filter) == PONKO_OK && ponko_latest_wins_len(filter) == 0);
    CHECK(ponko_latest_wins_accept(NULL, 1, s1, &ok) == PONKO_INVALID_ARGUMENT);
    ponko_latest_wins_free(filter);
    ponko_latest_wins_free(NULL);

    /* A sender 5000 s ahead walks along x at 20 units/s, 20 snapshots a second. */
    PonkoSnapshotStream* stream = NULL;
    CHECK(ponko_snapshot_stream_new(NULL, &stream) == PONKO_OK && stream);
    PonkoPose pose = {0}, shown = {0};
    pose.rotation.w = 1;
    pose.has_velocity = true;
    pose.velocity.x = 20;
    PonkoSnapshotState state;
    PonkoSnapshotPush result;
    bool found = true;
    CHECK(ponko_snapshot_stream_sample(stream, 0, &shown, &state, &found) == PONKO_OK && !found);
    for (int n = 0; n < 20; n++) {
        pose.position.x = (float)n;
        CHECK(ponko_snapshot_stream_push(stream, 5000 + n * 0.05, n * 0.05 + 0.03, &pose, &result) == PONKO_OK);
        CHECK(result == PONKO_SNAPSHOT_ADDED);
    }
    CHECK(ponko_snapshot_stream_push(stream, 5000, 1.0, &pose, &result) == PONKO_OK && result == PONKO_SNAPSHOT_DUPLICATE);
    CHECK(ponko_snapshot_stream_push(stream, NAN, 1.0, &pose, &result) == PONKO_OK && result == PONKO_SNAPSHOT_INVALID);
    CHECK(ponko_snapshot_stream_len(stream) == 20);
    CHECK(ponko_snapshot_stream_sample(stream, 1.0, &shown, &state, &found) == PONKO_OK && found);
    CHECK(state == PONKO_SNAPSHOT_INTERPOLATED && shown.position.x > 17.3f && shown.position.x < 17.5f && shown.has_velocity);
    /* Packets stop: predicted for 250 ms, then held where the prediction ended. */
    CHECK(ponko_snapshot_stream_sample(stream, 1.2, &shown, &state, &found) == PONKO_OK && state == PONKO_SNAPSHOT_EXTRAPOLATED);
    CHECK(ponko_snapshot_stream_sample(stream, 5.0, &shown, &state, &found) == PONKO_OK && state == PONKO_SNAPSHOT_HELD);
    CHECK(shown.position.x > 23.9f && shown.position.x < 24.1f); /* 19 + 0.25 s * 20 */
    double offset = 0;
    CHECK(ponko_snapshot_stream_clock_offset(stream, &offset, &found) == PONKO_OK && found && offset < -4999.9 && offset > -5000.0);
    CHECK(ponko_snapshot_stream_set_delay(stream, 0.0) == PONKO_OK);
    CHECK(ponko_snapshot_stream_set_delay(stream, -1.0) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_snapshot_stream_clear(stream) == PONKO_OK && ponko_snapshot_stream_len(stream) == 0);
    CHECK(ponko_snapshot_stream_sample(stream, 1.0, &shown, NULL, &found) == PONKO_OK && !found);
    CHECK(ponko_snapshot_stream_reset(stream) == PONKO_OK);
    CHECK(ponko_snapshot_stream_clock_offset(stream, &offset, &found) == PONKO_OK && !found);
    CHECK(ponko_snapshot_stream_push(stream, 1, 1, NULL, &result) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_snapshot_stream_sample(NULL, 1.0, &shown, &state, &found) == PONKO_INVALID_ARGUMENT);
    ponko_snapshot_stream_free(stream);
    ponko_snapshot_stream_free(NULL);
    PonkoSnapshotConfig bad = ponko_snapshot_config_default();
    bad.capacity = 1;
    CHECK(ponko_snapshot_stream_new(&bad, &stream) == PONKO_INVALID_ARGUMENT);
    bad = ponko_snapshot_config_default();
    bad.delay = -1;
    CHECK(ponko_snapshot_stream_new(&bad, &stream) == PONKO_INVALID_ARGUMENT);

    PonkoInterest* interest = NULL;
    PonkoInterestConfig config = ponko_interest_config_default();
    CHECK(config.tier_count == 2 && config.tiers[0].radius == 40 && config.tiers[1].every == 4);
    CHECK(ponko_interest_new(&config, &interest) == PONKO_OK);
    PonkoInterestViewer viewer = {1, {0, 0, 0}};
    PonkoInterestEntity world[3] = {{10, {10, 0, 0}}, {20, {100, 0, 0}}, {30, {900, 0, 0}}};
    CHECK(ponko_interest_update(interest, 0, &viewer, 1, world, 3) == PONKO_OK);
    PonkoInterestEntry entries[4];
    size_t count = 0;
    CHECK(ponko_interest_entries(interest, 1, entries, 4, &count) == PONKO_OK && count == 2);
    CHECK(entries[0].entity == 10 && entries[0].tier == 0 && entries[0].send);
    CHECK(entries[1].entity == 20 && entries[1].tier == 1 && entries[1].send && entries[1].distance == 100);
    PonkoInterestChange changes[4];
    CHECK(ponko_interest_changes(interest, changes, 4, &count) == PONKO_OK && count == 2);
    CHECK(changes[0].kind == PONKO_INTEREST_ENTERED && changes[0].peer == 1 && changes[0].entity == 10);
    /* The mid entity wobbles across the edge of the near ring (40): hysteresis keeps it in its ring. */
    for (uint64_t tick = 1; tick < 20; tick++) {
        world[1].position.x = 40 + (tick % 2 ? 3.0f : -3.0f);
        CHECK(ponko_interest_update(interest, tick, &viewer, 1, world, 2) == PONKO_OK);
        CHECK(ponko_interest_entries(interest, 1, entries, 4, &count) == PONKO_OK && count == 2 && entries[1].tier == 1);
        CHECK(ponko_interest_changes(interest, changes, 4, &count) == PONKO_OK && count == 0);
    }
    world[1].position.x = 500;
    CHECK(ponko_interest_update(interest, 20, &viewer, 1, world, 2) == PONKO_OK);
    CHECK(ponko_interest_changes(interest, changes, 4, &count) == PONKO_OK && count == 1);
    CHECK(changes[0].kind == PONKO_INTEREST_LEFT && changes[0].entity == 20);
    CHECK(ponko_interest_entries(interest, 99, NULL, 0, &count) == PONKO_OK && count == 0);
    CHECK(ponko_interest_remove_peer(interest, 1) == PONKO_OK);
    CHECK(ponko_interest_update(interest, 21, NULL, 0, NULL, 0) == PONKO_OK);
    CHECK(ponko_interest_update(interest, 21, NULL, 1, NULL, 0) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_interest_entries(NULL, 1, NULL, 0, &count) == PONKO_INVALID_ARGUMENT);
    ponko_interest_free(interest);
    interest = NULL;
    ponko_interest_free(NULL);
    config.tiers[1].radius = 10; /* not increasing */
    CHECK(ponko_interest_new(&config, &interest) == PONKO_INVALID_ARGUMENT && !interest);
    config = ponko_interest_config_default();
    config.tier_count = 0;
    CHECK(ponko_interest_new(&config, &interest) == PONKO_INVALID_ARGUMENT);
    config.tier_count = 9;
    CHECK(ponko_interest_new(&config, &interest) == PONKO_INVALID_ARGUMENT);
    config = ponko_interest_config_default();
    config.hysteresis = 40;
    CHECK(ponko_interest_new(&config, &interest) == PONKO_INVALID_ARGUMENT);
}

typedef struct { size_t rows; uint64_t last; } Rows;
static void count_rows(const PonkoChunk* chunk, void* user) {
    Rows* r = user;
    r->rows += chunk->count;
    if (chunk->count) r->last = ponko_entity_bits(chunk->entities[chunk->count - 1]);
}
static void bump(const PonkoChunk* chunk, void* user) {
    (void)user;
    for (size_t i = 0; i < chunk->count; ++i) ((Position*)chunk->columns[0])[i].x += 1;
}
static size_t rows_of(PonkoWorld* w, uint32_t id, PonkoQueryOptions options, uint64_t* last) {
    Rows rows = {0, 0};
    CHECK(ponko_query_with(w, &id, 1, &options, count_rows, &rows) == PONKO_OK);
    if (last) *last = rows.last;
    return rows.rows;
}
static void change_detection(void) {
    PonkoWorld* w = ponko_world_new(); CHECK(w);
    uint32_t p, other;
    CHECK(ponko_component_register(w, "Pos", sizeof(Position), _Alignof(Position), &p) == PONKO_OK);
    CHECK(ponko_component_register(w, "Other", sizeof(Position), _Alignof(Position), &other) == PONKO_OK);
    CHECK(ponko_component_track_changes(w, p) == PONKO_OK);
    CHECK(ponko_component_track_changes(w, 99) == PONKO_INVALID_ARGUMENT);
    Position value = {1}; PonkoValue pos = {p, &value, sizeof(value)};
    uint64_t a, b, c, seen, tick, last;
    CHECK(ponko_spawn(w, &pos, 1, &a) == PONKO_OK && ponko_spawn(w, &pos, 1, &b) == PONKO_OK);
    CHECK(ponko_world_advance_change_tick(w, &seen) == PONKO_OK && seen == 1);
    CHECK(ponko_world_change_tick(w, &tick) == PONKO_OK && tick == 2);

    // The defaults are ponko_query: every column may be written.
    PonkoQueryOptions read = ponko_query_options_default();
    CHECK(read.writes == UINT32_MAX && !read.changed && !read.added && !read.any && !read.parallel);
    CHECK(read.since == PONKO_SINCE_LAST_RUN);
    read.writes = 0;
    PonkoQueryOptions changed = read; changed.changed = 1; changed.since = seen;
    PonkoQueryOptions added = read; added.added = 1; added.since = seen;
    // A read-only query changes nothing.
    CHECK(rows_of(w, p, read, NULL) == 2 && rows_of(w, p, changed, NULL) == 0);
    // set and mark_changed do.
    value.x = 5; CHECK(ponko_set(w, b, p, &value, sizeof(value)) == PONKO_OK);
    CHECK(rows_of(w, p, changed, &last) == 1 && last == b && rows_of(w, p, added, NULL) == 0);
    CHECK(ponko_mark_changed(w, a, p) == PONKO_OK && rows_of(w, p, changed, NULL) == 2);
    CHECK(ponko_mark_changed(w, a, other) == PONKO_NOT_FOUND);
    CHECK(ponko_mark_changed(w, a, 99) == PONKO_INVALID_ARGUMENT);
    // The plain query marks every row it visits; a new entity is added.
    CHECK(ponko_world_advance_change_tick(w, &seen) == PONKO_OK);
    changed.since = added.since = seen;
    CHECK(rows_of(w, p, changed, NULL) == 0);
    CHECK(ponko_query(w, &p, 1, bump, NULL) == PONKO_OK && rows_of(w, p, changed, NULL) == 2);
    CHECK(ponko_spawn(w, &pos, 1, &c) == PONKO_OK);
    CHECK(rows_of(w, p, added, &last) == 1 && last == c && rows_of(w, p, changed, NULL) == 3);
    // Writing through a filtered query, on all cores.
    PonkoQueryOptions write_new = added; write_new.writes = 1; write_new.parallel = true;
    CHECK(ponko_query_with(w, &p, 1, &write_new, bump, NULL) == PONKO_OK);
    Position out;
    CHECK(ponko_get(w, c, p, &out, sizeof(out)) == PONKO_OK && out.x == 6);
    CHECK(ponko_get(w, a, p, &out, sizeof(out)) == PONKO_OK && out.x == 2);
    // PONKO_SINCE_LAST_RUN reads the world's own tick (a game sets it per system).
    changed.since = PONKO_SINCE_LAST_RUN;
    CHECK(rows_of(w, p, changed, NULL) == 3);
    CHECK(ponko_world_advance_change_tick(w, &seen) == PONKO_OK);
    CHECK(ponko_world_set_changes_since(w, seen) == PONKO_OK);
    CHECK(ponko_world_changes_since(w, &tick) == PONKO_OK && tick == seen);
    CHECK(rows_of(w, p, changed, NULL) == 0);
    // Removals: a removed component and a despawned entity.
    uint64_t gone[2] = {0, 0}; size_t n = 9;
    CHECK(ponko_removed(w, p, seen, NULL, 0, &n) == PONKO_OK && n == 0);
    CHECK(ponko_remove(w, a, p) == PONKO_OK && ponko_despawn(w, b) == PONKO_OK);
    CHECK(ponko_removed(w, p, PONKO_SINCE_LAST_RUN, gone, 1, &n) == PONKO_OK && n == 2 && gone[0] == a && gone[1] == 0);
    CHECK(ponko_removed(w, p, seen, gone, 2, &n) == PONKO_OK && n == 2 && gone[1] == b);
    CHECK(ponko_removed(w, other, 0, gone, 2, &n) == PONKO_INVALID_ARGUMENT);   // not tracked
    CHECK(ponko_removed(w, p, 0, NULL, 2, &n) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_query_with(w, &p, 1, &read, NULL, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_query_with(w, &p, 1, NULL, count_rows, &(Rows){0, 0}) == PONKO_OK);
    CHECK(ponko_world_change_tick(w, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

/* Query filters: with, without, OR over arms, AND groups inside an OR. One
 * entity per combination of four markers; bit i of an entity's label says
 * whether it has marker i, so every filter has a plain C predicate to compare
 * against. */
enum { PLAYER_BIT = 1, ENEMY_BIT = 2, DEAD_BIT = 4, GHOST_BIT = 8 };
typedef struct { uint32_t mask; size_t rows; } Labels;
static void collect_labels(const PonkoChunk* chunk, void* user) {
    Labels* l = user;
    const uint32_t* label = chunk->columns[0];
    for (size_t i = 0; i < chunk->count; ++i) { l->mask |= 1u << label[i]; ++l->rows; }
}
static uint32_t filtered_labels(PonkoWorld* w, uint32_t label, const PonkoFilter* filter, PonkoStatus* status) {
    Labels l = {0, 0};
    PonkoQueryOptions options = ponko_query_options_default();
    options.writes = 0;
    *status = ponko_query_filtered(w, &label, 1, filter, &options, collect_labels, &l);
    return l.mask;
}
/* A parallel callback fills row i of the query into slot i: no two threads share one. */
static void fill_slots(const PonkoChunk* chunk, void* user) {
    uint32_t* slots = user;
    const uint32_t* label = chunk->columns[0];
    for (size_t i = 0; i < chunk->count; ++i) slots[chunk->first + i] = label[i];
}
static uint32_t labels_where(int (*predicate)(unsigned)) {
    uint32_t mask = 0;
    for (unsigned bits = 0; bits < 16; ++bits) if (predicate(bits)) mask |= 1u << bits;
    return mask;
}
static int is_player(unsigned b) { return b & PLAYER_BIT; }
static int not_dead(unsigned b) { return !(b & DEAD_BIT); }
static int player_alive(unsigned b) { return (b & PLAYER_BIT) && !(b & DEAD_BIT); }
static int player_or_enemy(unsigned b) { return b & (PLAYER_BIT | ENEMY_BIT); }
static int living_fighter(unsigned b) { return player_or_enemy(b) && !(b & DEAD_BIT); }
static int both_or_alive(unsigned b) { return ((b & PLAYER_BIT) && (b & ENEMY_BIT)) || !(b & DEAD_BIT); }
static int nested(unsigned b) { return ((b & PLAYER_BIT) && ((b & DEAD_BIT) || ((b & GHOST_BIT) && !(b & ENEMY_BIT)))) || (b & ENEMY_BIT); }
static int anything(unsigned b) { (void)b; return 1; }
static int nothing(unsigned b) { (void)b; return 0; }
static void check_filter(PonkoWorld* w, uint32_t label, const PonkoFilter* filter, int (*predicate)(unsigned)) {
    PonkoStatus status;
    CHECK(ponko_filter_check(w, filter) == PONKO_OK);
    uint32_t want = labels_where(predicate);
    CHECK(filtered_labels(w, label, filter, &status) == want && status == PONKO_OK);
    /* The same rows on all cores. */
    uint32_t slots[16], rows = 0;
    for (int i = 0; i < 16; ++i) slots[i] = UINT32_MAX;
    PonkoQueryOptions par = ponko_query_options_default();
    par.writes = 0; par.parallel = true;
    CHECK(ponko_query_filtered(w, &label, 1, filter, &par, fill_slots, slots) == PONKO_OK);
    uint32_t seen = 0;
    for (int i = 0; i < 16; ++i) if (slots[i] != UINT32_MAX) { seen |= 1u << slots[i]; ++rows; }
    uint32_t count = 0;
    for (uint32_t m = want; m; m &= m - 1) ++count;
    CHECK(seen == want && rows == count);
}
static void query_filters(void) {
    PonkoWorld* w = ponko_world_new(); CHECK(w);
    const char* names[] = {"Player", "Enemy", "Dead", "Ghost"};
    uint32_t ids[4], label;
    for (int i = 0; i < 4; ++i) CHECK(ponko_component_register(w, names[i], sizeof(uint32_t), _Alignof(uint32_t), &ids[i]) == PONKO_OK);
    CHECK(ponko_component_register(w, "Label", sizeof(uint32_t), _Alignof(uint32_t), &label) == PONKO_OK);
    for (uint32_t bits = 0; bits < 16; ++bits) {
        PonkoValue values[5] = {{label, &bits, sizeof bits}};
        size_t n = 1;
        for (int i = 0; i < 4; ++i) if (bits >> i & 1) values[n++] = (PonkoValue){ids[i], &bits, sizeof bits};
        uint64_t e;
        CHECK(ponko_spawn(w, values, n, &e) == PONKO_OK);
    }
    PonkoFilter* player = ponko_filter_with(ids[0]);
    PonkoFilter* enemy = ponko_filter_with(ids[1]);
    PonkoFilter* dead = ponko_filter_with(ids[2]);
    PonkoFilter* ghost = ponko_filter_with(ids[3]);
    PonkoFilter* alive = ponko_filter_without(ids[2]);
    PonkoFilter* not_enemy = ponko_filter_without(ids[1]);
    CHECK(player && enemy && dead && ghost && alive && not_enemy);

    check_filter(w, label, player, is_player);
    check_filter(w, label, alive, not_dead);
    const PonkoFilter* player_alive_parts[] = {player, alive};
    PonkoFilter* player_alive_filter = ponko_filter_all(player_alive_parts, 2);
    check_filter(w, label, player_alive_filter, player_alive);
    const PonkoFilter* fighter_parts[] = {player, enemy};
    PonkoFilter* fighter = ponko_filter_any(fighter_parts, 2);
    check_filter(w, label, fighter, player_or_enemy);
    /* (Player OR Enemy) AND NOT Dead: two filters at once. */
    const PonkoFilter* living_parts[] = {fighter, alive};
    PonkoFilter* living = ponko_filter_all(living_parts, 2);
    check_filter(w, label, living, living_fighter);
    /* AND group inside an OR: (Player AND Enemy) OR NOT Dead. */
    const PonkoFilter* both_parts[] = {player, enemy};
    PonkoFilter* both = ponko_filter_all(both_parts, 2);
    const PonkoFilter* either_parts[] = {both, alive};
    PonkoFilter* either = ponko_filter_any(either_parts, 2);
    check_filter(w, label, either, both_or_alive);
    /* Nesting: (Player AND (Dead OR (Ghost AND NOT Enemy))) OR Enemy. */
    const PonkoFilter* ghost_parts[] = {ghost, not_enemy};
    PonkoFilter* ghost_alone = ponko_filter_all(ghost_parts, 2);
    const PonkoFilter* dead_or_parts[] = {dead, ghost_alone};
    PonkoFilter* dead_or_ghost = ponko_filter_any(dead_or_parts, 2);
    const PonkoFilter* player_arm_parts[] = {player, dead_or_ghost};
    PonkoFilter* player_arm = ponko_filter_all(player_arm_parts, 2);
    const PonkoFilter* nested_parts[] = {player_arm, enemy};
    PonkoFilter* nested_filter = ponko_filter_any(nested_parts, 2);
    check_filter(w, label, nested_filter, nested);
    /* Freeing the parts of a filter doesn't change it. */
    ponko_filter_free(player_arm); ponko_filter_free(dead_or_ghost); ponko_filter_free(ghost_alone);
    check_filter(w, label, nested_filter, nested);
    /* always matches everything, never nothing; null is no filter. */
    PonkoFilter* always = ponko_filter_always();
    PonkoFilter* never = ponko_filter_never();
    check_filter(w, label, always, anything);
    check_filter(w, label, never, nothing);
    PonkoStatus status;
    CHECK(filtered_labels(w, label, NULL, &status) == 0xFFFFu && status == PONKO_OK);
    CHECK(ponko_query_filtered(w, &label, 1, living, NULL, NULL, NULL) == PONKO_INVALID_ARGUMENT);
    /* A filter is checked in the world that runs it. */
    PonkoFilter* unknown = ponko_filter_with(9999);
    CHECK(unknown && ponko_filter_check(w, unknown) == PONKO_INVALID_ARGUMENT);
    CHECK(strstr(ponko_last_error(), "not registered") != NULL);
    CHECK(filtered_labels(w, label, unknown, &status) == 0 && status == PONKO_INVALID_ARGUMENT);
    /* Building errors are null: nothing to combine, a null part. */
    CHECK(ponko_filter_all(NULL, 0) == NULL && ponko_filter_any(NULL, 0) == NULL);
    const PonkoFilter* with_null[] = {player, NULL};
    CHECK(ponko_filter_any(with_null, 2) == NULL);
    /* A repeat is one slot and a contradiction matches nothing, as for a Rust tuple. */
    const PonkoFilter* contradiction[] = {player, ponko_filter_without(ids[0])};
    PonkoFilter* contradictory = ponko_filter_all(contradiction, 2);
    CHECK(contradictory);
    check_filter(w, label, contradictory, nothing);
    const PonkoFilter* repeat[] = {player, player};
    PonkoFilter* repeated = ponko_filter_all(repeat, 2);
    CHECK(repeated);
    check_filter(w, label, repeated, is_player);
    ponko_filter_free(contradictory); ponko_filter_free(repeated);
    ponko_filter_free((PonkoFilter*)contradiction[1]);
    /* Limits are errors too: 9 with components (8 are fine) and 5 `any` clauses (4 are). */
    PonkoFilter* many[9];
    for (int i = 0; i < 9; ++i) {
        char name[16]; uint32_t id;
        snprintf(name, sizeof name, "Extra%d", i);
        CHECK(ponko_component_register(w, name, 4, 4, &id) == PONKO_OK);
        many[i] = ponko_filter_with(id);
    }
    const PonkoFilter* nine_parts[9];
    for (int i = 0; i < 9; ++i) nine_parts[i] = many[i];
    PonkoFilter* eight = ponko_filter_all(nine_parts, 8);
    PonkoFilter* nine = ponko_filter_all(nine_parts, 9);
    CHECK(eight && nine && ponko_filter_check(w, eight) == PONKO_OK && ponko_filter_check(w, nine) == PONKO_INVALID_ARGUMENT);
    CHECK(strstr(ponko_last_error(), "at most 8 `with`") != NULL);
    PonkoFilter* clauses[5];
    for (int i = 0; i < 5; ++i) {
        const PonkoFilter* one[] = {many[i]};
        clauses[i] = ponko_filter_any(one, 1);
    }
    const PonkoFilter* clause_parts[5] = {clauses[0], clauses[1], clauses[2], clauses[3], clauses[4]};
    PonkoFilter* four_clauses = ponko_filter_all(clause_parts, 4);
    PonkoFilter* five_clauses = ponko_filter_all(clause_parts, 5);
    CHECK(four_clauses && five_clauses);
    CHECK(ponko_filter_check(w, four_clauses) == PONKO_OK && ponko_filter_check(w, five_clauses) == PONKO_INVALID_ARGUMENT);
    CHECK(strstr(ponko_last_error(), "at most 4 `Or`") != NULL);
    ponko_filter_free(four_clauses); ponko_filter_free(five_clauses);
    for (int i = 0; i < 5; ++i) ponko_filter_free(clauses[i]);
    ponko_filter_free(eight); ponko_filter_free(nine);
    for (int i = 0; i < 9; ++i) ponko_filter_free(many[i]);
    ponko_filter_free(unknown); ponko_filter_free(never); ponko_filter_free(always);
    ponko_filter_free(nested_filter); ponko_filter_free(either); ponko_filter_free(both);
    ponko_filter_free(living); ponko_filter_free(fighter); ponko_filter_free(player_alive_filter);
    ponko_filter_free(not_enemy); ponko_filter_free(alive); ponko_filter_free(ghost);
    ponko_filter_free(dead); ponko_filter_free(enemy); ponko_filter_free(player);
    ponko_filter_free(NULL);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

/* Wide queries: a C query takes 32 columns (Rust tuples hold 12; nest them for more), also with a filter. */
static void count_wide(const PonkoChunk* chunk, void* user) {
    CHECK(chunk->column_count == 32);
    for (size_t column = 0; column < 32; ++column)
        for (size_t i = 0; i < chunk->count; ++i) CHECK(((const uint32_t*)chunk->columns[column])[i] == column);
    *(size_t*)user += chunk->count;
}
static void wide_queries(void) {
    PonkoWorld* w = ponko_world_new(); CHECK(w);
    uint32_t ids[33], values[33];
    PonkoValue spawn[33];
    for (uint32_t i = 0; i < 33; ++i) {
        char name[16];
        snprintf(name, sizeof name, "Wide%u", i);
        CHECK(ponko_component_register(w, name, sizeof(uint32_t), _Alignof(uint32_t), &ids[i]) == PONKO_OK);
        values[i] = i;
        spawn[i] = (PonkoValue){ids[i], &values[i], sizeof(uint32_t)};
    }
    uint64_t e;
    CHECK(ponko_spawn(w, spawn, 32, &e) == PONKO_OK && ponko_spawn(w, spawn, 33, &e) == PONKO_OK);
    size_t rows = 0;
    CHECK(ponko_query(w, ids, 32, count_wide, &rows) == PONKO_OK && rows == 2);
    CHECK(ponko_query(w, ids, 33, count_wide, &rows) == PONKO_INVALID_ARGUMENT);
    /* The 33rd component is a filter, not a column. */
    PonkoFilter* last = ponko_filter_with(ids[32]);
    rows = 0;
    CHECK(ponko_query_filtered(w, ids, 32, last, NULL, count_wide, &rows) == PONKO_OK && rows == 1);
    CHECK(ponko_query_filtered(w, ids, 33, last, NULL, count_wide, &rows) == PONKO_INVALID_ARGUMENT);
    ponko_filter_free(last);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

/* The archetype index: 128 archetypes (one per subset of seven markers), so
 * the default policy applies. The per-query hint, the world's off switch and
 * the automatic policy all visit the same rows. */
static uint32_t cache_slots[256];
static uint32_t cached_rows(PonkoWorld* w, uint32_t label, const PonkoFilter* filter, uint32_t cache, bool parallel) {
    Rows rows = {0, 0};
    PonkoQueryOptions options = ponko_query_options_default();
    options.writes = 0; options.parallel = parallel;
    /* A parallel callback may not share a counter: count on one thread, and
     * only check that the parallel query runs. */
    if (parallel) { CHECK(ponko_query_cached(w, &label, 1, filter, &options, cache, fill_slots, cache_slots) == PONKO_OK); return 0; }
    CHECK(ponko_query_cached(w, &label, 1, filter, &options, cache, count_rows, &rows) == PONKO_OK);
    return (uint32_t)rows.rows;
}
static void query_cache(void) {
    PonkoWorld* w = ponko_world_new(); CHECK(w);
    uint32_t markers[7], label;
    char name[16];
    for (int i = 0; i < 7; ++i) {
        snprintf(name, sizeof name, "Marker%d", i);
        CHECK(ponko_component_register(w, name, sizeof(uint32_t), _Alignof(uint32_t), &markers[i]) == PONKO_OK);
    }
    CHECK(ponko_component_register(w, "Label", sizeof(uint32_t), _Alignof(uint32_t), &label) == PONKO_OK);
    for (uint32_t bits = 0; bits < 128; ++bits) {
        PonkoValue values[8] = {{label, &bits, sizeof bits}};
        size_t n = 1;
        for (int i = 0; i < 7; ++i) if (bits >> i & 1) values[n++] = (PonkoValue){markers[i], &bits, sizeof bits};
        uint64_t e;
        CHECK(ponko_spawn(w, values, n, &e) == PONKO_OK);
    }
    /* Marker 0 and marker 1, without marker 2: 16 of the 128. */
    PonkoFilter* a = ponko_filter_with(markers[0]);
    PonkoFilter* b = ponko_filter_with(markers[1]);
    PonkoFilter* c = ponko_filter_without(markers[2]);
    const PonkoFilter* parts[] = {a, b, c};
    PonkoFilter* filter = ponko_filter_all(parts, 3);
    CHECK(a && b && c && filter);

    PonkoQueryCacheStats stats;
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.signatures == 0 && stats.hits == 0);
    /* Never: the index is not touched. */
    CHECK(cached_rows(w, label, filter, PONKO_QUERY_CACHE_NEVER, false) == 16);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.signatures == 0);
    /* Automatic, through the ordinary query function: indexed from its second run. */
    Rows rows = {0, 0};
    for (int i = 0; i < 3; ++i) CHECK(ponko_query_filtered(w, &label, 1, filter, NULL, count_rows, &rows) == PONKO_OK);
    CHECK(rows.rows == 3 * 16);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK);
    CHECK(stats.signatures == 1 && stats.indexed == 1 && stats.indices == 16 && stats.hits == 2 && stats.bytes > 0);
    /* Always: a query the policy would scan (it matches every archetype) is indexed at once. */
    CHECK(cached_rows(w, label, NULL, PONKO_QUERY_CACHE_AUTO, false) == 128);
    CHECK(cached_rows(w, label, NULL, PONKO_QUERY_CACHE_AUTO, false) == 128);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.signatures == 2 && stats.indexed == 1);
    CHECK(cached_rows(w, label, NULL, PONKO_QUERY_CACHE_ALWAYS, false) == 128);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.indexed == 2 && stats.indices == 16 + 128);
    /* The same rows, in the same order, on all cores. */
    for (int i = 0; i < 128; ++i) cache_slots[i] = UINT32_MAX;
    cached_rows(w, label, filter, PONKO_QUERY_CACHE_ALWAYS, true);
    uint32_t expect = 0, filled = 0;
    for (uint32_t bits = 0; bits < 128; ++bits) {
        if ((bits & 7) != 3) continue;
        ++filled;
    }
    for (int i = 0; i < 128; ++i) if (cache_slots[i] != UINT32_MAX) { CHECK((cache_slots[i] & 7) == 3); ++expect; }
    CHECK(expect == filled && filled == 16);
    /* A new archetype is listed before the next query. */
    uint32_t extra, value = 3;
    CHECK(ponko_component_register(w, "Extra", sizeof(uint32_t), _Alignof(uint32_t), &extra) == PONKO_OK);
    PonkoValue grown[4] = {{label, &value, sizeof value}, {markers[0], &value, sizeof value}, {markers[1], &value, sizeof value}, {extra, &value, sizeof value}};
    uint64_t e;
    CHECK(ponko_spawn(w, grown, 4, &e) == PONKO_OK);
    CHECK(cached_rows(w, label, filter, PONKO_QUERY_CACHE_AUTO, false) == 17);
    CHECK(cached_rows(w, label, filter, PONKO_QUERY_CACHE_NEVER, false) == 17);

    /* The policy: read, change, and turn the index off for the world. */
    PonkoQueryCachePolicy policy, off = ponko_query_cache_policy_off(), standard = ponko_query_cache_policy_default();
    CHECK(ponko_world_query_cache_policy(w, &policy) == PONKO_OK);
    CHECK(policy.min_archetypes == standard.min_archetypes && policy.promote_after == standard.promote_after);
    CHECK(off.min_archetypes == SIZE_MAX && standard.min_archetypes < 128);
    CHECK(ponko_world_set_query_cache_policy(w, &off) == PONKO_OK);
    CHECK(ponko_world_query_cache_policy(w, &policy) == PONKO_OK && policy.min_archetypes == SIZE_MAX);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.signatures == 0 && stats.bytes == 0);
    uint64_t hits = stats.hits;
    CHECK(cached_rows(w, label, filter, PONKO_QUERY_CACHE_ALWAYS, false) == 17);
    CHECK(cached_rows(w, label, filter, PONKO_QUERY_CACHE_AUTO, false) == 17);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.signatures == 0 && stats.hits == hits);
    standard.promote_after = 1;
    CHECK(ponko_world_set_query_cache_policy(w, &standard) == PONKO_OK);
    CHECK(cached_rows(w, label, filter, PONKO_QUERY_CACHE_AUTO, false) == 17);
    CHECK(ponko_world_query_cache_stats(w, &stats) == PONKO_OK && stats.indexed == 1 && stats.hits == hits + 1);

    /* Bad arguments. */
    standard.reserved[0] = 1;
    CHECK(ponko_world_set_query_cache_policy(w, &standard) == PONKO_INVALID_ARGUMENT);
    standard.reserved[0] = 0;
    standard.max_match_percent = 101;
    CHECK(ponko_world_set_query_cache_policy(w, &standard) == PONKO_OK);
    CHECK(ponko_world_query_cache_policy(w, &policy) == PONKO_OK && policy.max_match_percent == 100);
    CHECK(ponko_world_set_query_cache_policy(w, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_world_set_query_cache_policy(NULL, &off) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_world_query_cache_policy(w, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_world_query_cache_stats(w, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_world_query_cache_stats(NULL, &stats) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_query_cached(w, &label, 1, filter, NULL, 3, count_rows, &rows) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_query_cached(w, &label, 1, filter, NULL, PONKO_QUERY_CACHE_ALWAYS, NULL, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_query_cached(NULL, &label, 1, filter, NULL, PONKO_QUERY_CACHE_ALWAYS, count_rows, &rows) == PONKO_INVALID_ARGUMENT);

    ponko_filter_free(a); ponko_filter_free(b); ponko_filter_free(c); ponko_filter_free(filter);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

/* Headless: no renderer, so nothing is uploaded to the GPU. */
static void no_uploads(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoGpuUploads u = {7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7};
    CHECK(ponko_ctx_gpu_uploads(ctx, &u) == PONKO_OK);
    CHECK(u.bytes == 0 && u.instances == 0 && u.uniforms == 0 && u.buffers_created == 0 && u.reallocations == 0);
    CHECK(ponko_ctx_gpu_uploads(ctx, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_gpu_uploads(NULL, &u) == PONKO_INVALID_ARGUMENT);
}

static void profiling_c(void) {
    const PonkoProfileScope* scope = ponko_profile_scope("c scope");
    CHECK(scope && scope == ponko_profile_scope("c scope"));
    CHECK(ponko_profile_scope(NULL) == NULL);
    ponko_profile_end(scope, ponko_profile_begin(scope));
    ponko_note_allocation(16);
    PonkoGame* game = ponko_game_new("c perf", 1, 1); CHECK(game);
    PonkoWorld* world = NULL;
    CHECK(ponko_game_profile_headless(game, 2, 10, NULL, NULL, &world) == PONKO_OK && world);
    CHECK(ponko_world_free(world) == PONKO_OK);
    size_t len = 0;
    CHECK(ponko_last_perf_report(NULL, 0, &len) == PONKO_OK && len > 10);
    char* json = malloc(len + 1);
    CHECK(ponko_last_perf_report(json, len + 1, &len) == PONKO_OK && json[0] == '{');
    size_t problems = 1;
    CHECK(ponko_perf_check(json, "{\"frame_p99_ms\": 100000}", NULL, 0, &problems) == PONKO_OK && problems == 0);
    CHECK(ponko_perf_check(json, "{\"systems\": {\"missing\": 1}}", NULL, 0, &problems) == PONKO_OK && problems > 0);
    CHECK(ponko_perf_report_text("nope", NULL, 0, &problems) == PONKO_INVALID_ARGUMENT);
    free(json);
    PonkoGame* quiet = ponko_game_new("c uploads", 1, 1); CHECK(quiet);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(quiet, 0, no_uploads, NULL, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

/* A two-joint skin with one clip, "rise": the arm goes up 2 over 1 second. */
static const char RISE_GLTF[] =
    "{\"asset\": {\"version\": \"2.0\"}, \"scene\": 0, \"scenes\": [{\"nodes\": [0]}],"
    "\"nodes\": [{\"name\": \"root\", \"children\": [1]}, {\"name\": \"arm\", \"mesh\": 0}],"
    "\"meshes\": [{\"primitives\": [{\"attributes\": {\"POSITION\": 2}}]}],"
    "\"skins\": [{\"joints\": [0, 1]}],"
    "\"animations\": [{\"name\": \"rise\", \"samplers\": [{\"input\": 0, \"output\": 1}],"
    "\"channels\": [{\"sampler\": 0, \"target\": {\"node\": 1, \"path\": \"translation\"}}]}],"
    "\"buffers\": [{\"byteLength\": 68, \"uri\": \"data:application/octet-stream;base64,"
    "AAAAAAAAgD8AAAAAAAAAAAAAAAAAAAAAAAAAQAAAAAAAAAAAAAAAAAAAAAAAAIA/AAAAAAAAAAAAAAAAAACAPwAAAAA=\"}],"
    "\"bufferViews\": [{\"buffer\": 0, \"byteLength\": 8}, {\"buffer\": 0, \"byteOffset\": 8, \"byteLength\": 24},"
    "{\"buffer\": 0, \"byteOffset\": 32, \"byteLength\": 36}],"
    "\"accessors\": [{\"bufferView\": 0, \"componentType\": 5126, \"count\": 2, \"type\": \"SCALAR\", \"min\": [0], \"max\": [1]},"
    "{\"bufferView\": 1, \"componentType\": 5126, \"count\": 2, \"type\": \"VEC3\"},"
    "{\"bufferView\": 2, \"componentType\": 5126, \"count\": 3, \"type\": \"VEC3\", \"min\": [0, 0, 0], \"max\": [1, 1, 0]}]}";

static const char* temp_dir(void) {
    const char* dir = getenv("TMPDIR");
    if (!dir) dir = getenv("TEMP");
    if (!dir) dir = getenv("TMP");
    return dir ? dir : "/tmp";
}

typedef struct { char path[1024]; } AnimatorRun;

/* A graph with a float parameter, a resting state and a "rise" state with an
   event half way, then an animator on a skeleton, run with manual updates. */
static void animator_setup(PonkoCtx* ctx, void* user) {
    const AnimatorRun* run = user;
    PonkoClip* rise = NULL;
    CHECK(ponko_ctx_load_clip(ctx, run->path, "rise", &rise) == PONKO_OK && rise);
    CHECK(fabsf(ponko_clip_duration(rise) - 1.0f) < 1e-5f);
    PonkoClip* missing = NULL;
    CHECK(ponko_ctx_load_clip(ctx, run->path, "no such clip", &missing) != PONKO_OK && !missing);

    PonkoAnimationGraphBuilder* b = ponko_animation_graph_builder_new();
    size_t speed = 99, still = 99, up = 99, go = 99;
    CHECK(ponko_animation_graph_add_float(b, 0.0f, &speed) == PONKO_OK && speed == 0);
    PonkoMotion* motion = NULL;
    CHECK(ponko_motion_clip(rise, &motion) == PONKO_OK);
    CHECK(ponko_animation_graph_add_state(b, motion, 0.0f, -1, true, &still) == PONKO_OK && still == 0);
    CHECK(ponko_motion_clip(rise, &motion) == PONKO_OK);
    CHECK(ponko_animation_graph_add_state(b, motion, 1.0f, -1, false, &up) == PONKO_OK && up == 1);
    CHECK(ponko_animation_graph_add_event(b, up, 0.5f, "half") == PONKO_OK);
    CHECK(ponko_animation_graph_add_event(b, 9, 0.5f, "nowhere") == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_animation_graph_add_transition(b, (int64_t)still, up, -1.0f, 0.0f, 0.0f, &go) == PONKO_OK);
    CHECK(ponko_animation_graph_add_condition(b, go, PONKO_CONDITION_GREATER, speed, 0.5f) == PONKO_OK);
    PonkoAnimationGraph* graph = NULL;
    CHECK(ponko_animation_graph_build(b, still, &graph) == PONKO_OK && graph);
    /* A graph without states does not build; the builder is used up either way. */
    PonkoAnimationGraphBuilder* empty = ponko_animation_graph_builder_new();
    PonkoAnimationGraph* none = NULL;
    CHECK(ponko_animation_graph_build(empty, 3, &none) != PONKO_OK && !none);

    PonkoWorld* world = ponko_ctx_world(ctx);
    uint32_t transform_id;
    CHECK(ponko_transform3d_component(world, &transform_id) == PONKO_OK);
    PonkoTransform3D at = ponko_transform3d_at((PonkoVec3){0, 0, 0});
    PonkoValue part = {transform_id, &at, sizeof at};
    uint64_t e;
    CHECK(ponko_spawn(world, &part, 1, &e) == PONKO_OK);
    CHECK(ponko_ctx_set_animator(ctx, e, graph) != PONKO_OK); /* no skeleton yet */
    CHECK(ponko_ctx_load_skeleton(ctx, e, run->path) == PONKO_OK);
    CHECK(ponko_ctx_set_animator(ctx, e, graph) == PONKO_OK);
    CHECK(ponko_ctx_animator_set_manual(ctx, e, true) == PONKO_OK);

    size_t state = 99; float time = -1;
    CHECK(ponko_ctx_animator_update(ctx, e, 0.1f) == PONKO_OK);
    CHECK(ponko_ctx_animator_state(ctx, e, &state, &time) == PONKO_OK && state == still);
    CHECK(ponko_ctx_animator_set_float(ctx, e, speed, 1.0f) == PONKO_OK);
    /* A wrong kind or index, or bad values, are invalid arguments. */
    CHECK(ponko_ctx_animator_set_bool(ctx, e, speed, true) == PONKO_INVALID_ARGUMENT); /* it is a float */
    CHECK(ponko_ctx_animator_set_float(ctx, e, 9, 1.0f) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_animator_set_float(ctx, e, speed, NAN) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_animator_set_trigger(ctx, e, speed, true) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_animator_update(ctx, e, 0.0f) == PONKO_OK); /* takes the transition */
    CHECK(ponko_ctx_animator_state(ctx, e, &state, &time) == PONKO_OK && state == up);
    CHECK(ponko_ctx_animator_update(ctx, e, 0.75f) == PONKO_OK);
    CHECK(ponko_ctx_animator_state(ctx, e, &state, &time) == PONKO_OK && fabsf(time - 0.75f) < 1e-4f);
    PonkoTransform3D arm;
    CHECK(ponko_ctx_joint_pose(ctx, e, 1, &arm) == PONKO_OK && fabsf(arm.position.y - 1.5f) < 1e-4f);

    char name[64]; bool found = false;
    CHECK(ponko_ctx_animator_next_event(ctx, e, name, sizeof name, &found) == PONKO_OK && found);
    CHECK(strcmp(name, "half") == 0);
    CHECK(ponko_ctx_animator_next_event(ctx, e, name, sizeof name, &found) == PONKO_OK && !found);
    CHECK(ponko_ctx_animator_crossfade(ctx, e, still, 0.0f, 0.0f) == PONKO_OK);
    CHECK(ponko_ctx_animator_update(ctx, e, 0.0f) == PONKO_OK);
    CHECK(ponko_ctx_animator_state(ctx, e, &state, &time) == PONKO_OK && state == still);
    CHECK(ponko_ctx_animator_crossfade(ctx, e, 7, 0.0f, 0.0f) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_animator_crossfade(ctx, e, still, -1.0f, 0.0f) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_animator_crossfade(ctx, e, still, 0.0f, NAN) == PONKO_INVALID_ARGUMENT);

    ponko_animation_graph_free(graph);
    ponko_clip_free(rise);
}

static void animation_graphs(void) {
    AnimatorRun run;
    snprintf(run.path, sizeof run.path, "%s/ponko-c-animator.gltf", temp_dir());
    FILE* file = fopen(run.path, "wb");
    CHECK(file);
    CHECK(fputs(RISE_GLTF, file) >= 0);
    CHECK(fclose(file) == 0);
    PonkoGame* game = ponko_game_new("animator", 1, 1);
    PonkoWorld* world = NULL;
    CHECK(ponko_game_run_headless(game, 0, animator_setup, &run, &world) == PONKO_OK);
    CHECK(ponko_world_free(world) == PONKO_OK);
    remove(run.path);
}

/* Declared access: a C system says what it reads and writes. */
typedef struct { uint32_t pos; uint64_t entity; size_t hits; } Declared;
static void declared_mover(PonkoCtx* ctx, void* user) {
    Declared* d = user;
    Position at;
    CHECK(ponko_get(ponko_ctx_world(ctx), d->entity, d->pos, &at, sizeof(at)) == PONKO_OK);
    at.x += 1;
    CHECK(ponko_set(ponko_ctx_world(ctx), d->entity, d->pos, &at, sizeof(at)) == PONKO_OK);
    CHECK(ponko_ctx_send_event(ctx, d->pos, &at, sizeof(at)) == PONKO_OK);
}
static void declared_reader(PonkoCtx* ctx, void* user) {
    Declared* d = user;
    const void* data; size_t count;
    CHECK(ponko_ctx_read_events(ctx, d->pos, sizeof(Position), &data, &count) == PONKO_OK);
    d->hits += count;
}
static void declared_setup(PonkoCtx* ctx, void* user) {
    Declared* d = user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    CHECK(ponko_component_register(w, "DeclPos", sizeof(Position), _Alignof(Position), &d->pos) == PONKO_OK);
    Position start = {0}; PonkoValue v = {d->pos, &start, sizeof(start)};
    CHECK(ponko_spawn(w, &v, 1, &d->entity) == PONKO_OK);
}
static void declared_access(void) {
    PonkoAccess* mover = ponko_access_new(); CHECK(mover);
    CHECK(ponko_access_write_component(mover, "DeclPos") == PONKO_OK);
    CHECK(ponko_access_send_events(mover, "DeclPos") == PONKO_OK);
    CHECK(ponko_access_service(mover, PONKO_SERVICE_TIME, false) == PONKO_OK);
    CHECK(ponko_access_service(mover, 99, false) == PONKO_INVALID_ARGUMENT);
    PonkoAccess* reader = ponko_access_new(); CHECK(reader);
    CHECK(ponko_access_read_events(reader, "DeclPos") == PONKO_OK);
    size_t n = 9;
    CHECK(ponko_access_conflicts(reader, mover, true, &n) == PONKO_OK && n == 0);
    CHECK(ponko_access_conflicts(mover, reader, true, &n) == PONKO_OK && n == 1);
    CHECK(ponko_access_conflicts(reader, mover, false, &n) == PONKO_OK && n == 1);
    PonkoAccess* spawner = ponko_access_new(); CHECK(spawner);
    CHECK(ponko_access_commands(spawner) == PONKO_OK);
    CHECK(ponko_access_conflicts(spawner, mover, true, &n) == PONKO_OK && n == 1);
    CHECK(ponko_access_conflicts(mover, spawner, true, &n) == PONKO_OK && n == 0);
    ponko_access_free(spawner);
    Declared d = {0, 0, 0};
    PonkoGame* g = ponko_game_new("declared", 1, 1); CHECK(g);
    CHECK(ponko_game_add_system_with_access(g, PONKO_PHASE_AFTER_PHYSICS, "mover", 1, mover, declared_mover, &d) == PONKO_OK);
    CHECK(ponko_game_add_system_with_access(g, PONKO_PHASE_AFTER_PHYSICS, "reader", 1, reader, declared_reader, &d) == PONKO_OK);
    CHECK(ponko_game_add_system_with_access(g, PONKO_PHASE_AFTER_PHYSICS, "bad", 0, reader, declared_reader, &d) == PONKO_INVALID_ARGUMENT);
    ponko_access_free(mover);
    ponko_access_free(reader);
    size_t len = 0;
    CHECK(ponko_game_access_report(g, NULL, 0, &len) == PONKO_OK && len > 0);
    char* text = malloc(len + 1);
    CHECK(ponko_game_access_report(g, text, len + 1, &len) == PONKO_OK);
    CHECK(strcmp(text, "mover + reader: conflict on events `DeclPos`\n") == 0);
    free(text);
    /* The scheduler may be on: C systems still run one at a time, and the plan says why. */
    CHECK(ponko_game_parallel_systems(g, true) == PONKO_OK);
    CHECK(ponko_game_schedule_plan(g, NULL, 0, &len) == PONKO_OK && len > 0);
    text = malloc(len + 1);
    CHECK(ponko_game_schedule_plan(g, text, len + 1, &len) == PONKO_OK);
    CHECK(strstr(text, "  mover (alone: takes `&mut Ctx`)\n") != NULL);
    CHECK(strstr(text, "  reader (alone: takes `&mut Ctx`)\n") != NULL);
    free(text);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(g, 3, declared_setup, &d, &w) == PONKO_OK && w);
    Position at;
    CHECK(ponko_get(w, d.entity, d.pos, &at, sizeof(at)) == PONKO_OK && at.x == 3);
    CHECK(d.hits == 3);
    CHECK(ponko_world_free(w) == PONKO_OK);
}

/* Barriers: a system's commands and events reach the next system, and
 * nothing is applied while the recording system (or its query) still runs. */
typedef struct {
    PonkoCtx* ctx; uint32_t mark;
    size_t in_callback, after_recording, next_sees, pings, runs;
} Barrier;
static void barrier_visit(const PonkoChunk* chunk, void* user) {
    Barrier* b = user;
    Position p = {1}; PonkoValue v = {b->mark, &p, sizeof(p)};
    for (size_t i = 0; i < chunk->count; ++i) CHECK(ponko_ctx_spawn_later(b->ctx, &v, 1) == PONKO_OK);
    /* A callback can record commands but can't look at the world while the
     * query walks it, so it never sees a half-applied change. */
    size_t len;
    CHECK(ponko_world_len(ponko_ctx_world(b->ctx), &len) == PONKO_BUSY);
    ++b->in_callback;
}
static void barrier_recorder(PonkoCtx* ctx, void* user) {
    Barrier* b = user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    size_t before; CHECK(ponko_world_len(w, &before) == PONKO_OK);
    b->ctx = ctx;
    b->in_callback = 0;
    CHECK(ponko_query(w, &b->mark, 1, barrier_visit, b) == PONKO_OK);
    CHECK(b->in_callback > 0);
    /* The commands recorded in the query are still pending after it. */
    size_t after_query; CHECK(ponko_world_len(w, &after_query) == PONKO_OK && after_query == before);
    Position p = {2}; PonkoValue v = {b->mark, &p, sizeof(p)};
    CHECK(ponko_ctx_spawn_later(ctx, &v, 1) == PONKO_OK);
    CHECK(ponko_ctx_send_event(ctx, b->mark, &p, sizeof(p)) == PONKO_OK);
    /* And still pending when the system is about to return. */
    CHECK(ponko_world_len(w, &b->after_recording) == PONKO_OK);
    CHECK(b->after_recording == before);
}
static void barrier_next(PonkoCtx* ctx, void* user) {
    Barrier* b = user;
    CHECK(ponko_world_len(ponko_ctx_world(ctx), &b->next_sees) == PONKO_OK);
    const void* data; size_t count;
    CHECK(ponko_ctx_read_events(ctx, b->mark, sizeof(Position), &data, &count) == PONKO_OK);
    b->pings += count;
    ++b->runs;
    /* Both commands of the recorder are in the world, together with its event:
     * one entity before the first tick, doubled plus one by every recorder run. */
    size_t expected = 1;
    for (size_t i = 0; i < b->runs; ++i) expected = expected * 2 + 1;
    CHECK(b->next_sees == expected);
    CHECK(count == 1);
}
static void barrier_setup(PonkoCtx* ctx, void* user) {
    Barrier* b = user;
    PonkoWorld* w = ponko_ctx_world(ctx);
    CHECK(ponko_component_register(w, "BarrierMark", sizeof(Position), _Alignof(Position), &b->mark) == PONKO_OK);
    Position p = {0}; PonkoValue v = {b->mark, &p, sizeof(p)};
    /* A command from setup applies before the first tick. */
    CHECK(ponko_ctx_spawn_later(ctx, &v, 1) == PONKO_OK);
}
static void command_barriers(void) {
    /* The same with the scheduler off and on: C systems always run alone. */
    for (int parallel = 0; parallel < 2; ++parallel) {
        Barrier b = {0};
        PonkoGame* g = ponko_game_new("barriers", 1, 1); CHECK(g);
        CHECK(ponko_game_parallel_systems(g, parallel != 0) == PONKO_OK);
        CHECK(ponko_game_add_system(g, PONKO_PHASE_AFTER_PHYSICS, "recorder", 1, barrier_recorder, &b) == PONKO_OK);
        CHECK(ponko_game_add_system(g, PONKO_PHASE_AFTER_PHYSICS, "next", 1, barrier_next, &b) == PONKO_OK);
        PonkoWorld* w = NULL;
        CHECK(ponko_game_run_headless(g, 3, barrier_setup, &b, &w) == PONKO_OK && w);
        CHECK(b.runs == 3 && b.pings == 3);
        size_t len; CHECK(ponko_world_len(w, &len) == PONKO_OK && len == 15);
        CHECK(ponko_world_free(w) == PONKO_OK);
    }
}

/* Action ids, asset sources, the try_ loaders and named grid spans (#262). */
typedef struct { const char* name; const uint8_t* bytes; size_t len; int frees; } Pak;
static uint32_t pak_read(void* user, const char* name, const uint8_t** data, size_t* len) {
    Pak* pak = user;
    if (strcmp(name, pak->name) != 0) return PONKO_NOT_FOUND;
    *data = pak->bytes; *len = pak->len;
    return PONKO_OK;
}
static void pak_free(void* user) { ((Pak*)user)->frees++; }
typedef struct { size_t len; uint8_t first; } Decoded;
static uint32_t decode_bytes(void* user, const uint8_t* data, size_t len) {
    Decoded* d = user;
    d->len = len; d->first = len ? data[0] : 0;
    return len ? PONKO_OK : PONKO_INVALID_ARGUMENT;
}
static void native_setup(PonkoCtx* ctx, void* user) {
    Pak* pak = user;
    /* Action ids: resolved once, then queried without the name. */
    PonkoActionMap* map = ponko_action_map_new();
    PonkoActionBinding space = {PONKO_ACTION_KEY, 0, PONKO_KEY_SPACE, 0, 0, 0, 0};
    CHECK(ponko_action_map_set(map, "jump", 0, &space, 1) == PONKO_OK);
    CHECK(ponko_ctx_set_actions(ctx, map) == PONKO_OK);
    ponko_action_map_free(map);
    uint32_t jump = PONKO_ACTION_NONE, ids[2];
    const char* names[2] = {"jump", "fly"};
    CHECK(ponko_ctx_action_id(ctx, "jump", &jump) == PONKO_OK && jump != PONKO_ACTION_NONE);
    CHECK(ponko_ctx_action_ids(ctx, names, 2, ids) == PONKO_NOT_FOUND);
    CHECK(ids[0] == jump && ids[1] == PONKO_ACTION_NONE && ponko_last_error_kind() == PONKO_ERROR_NOT_FOUND);
    CHECK(ponko_ctx_feed_input(ctx, 0, 0, PONKO_KEY_SPACE, 1) == PONKO_OK);
    bool down = false, pressed = false, released = true;
    float axis = 1;
    PonkoVec2 axis2 = {1, 1};
    CHECK(ponko_ctx_action_down_id(ctx, jump, &down) == PONKO_OK && down);
    CHECK(ponko_ctx_action_pressed_id(ctx, jump, &pressed) == PONKO_OK && pressed);
    CHECK(ponko_ctx_action_released_id(ctx, jump, &released) == PONKO_OK && !released);
    CHECK(ponko_ctx_action_axis_id(ctx, jump, &axis) == PONKO_OK && axis == 0);
    CHECK(ponko_ctx_action_axis2_id(ctx, PONKO_ACTION_NONE, &axis2) == PONKO_OK && axis2.x == 0);

    /* Pressed this frame: what a rebinding screen reads, ready for ponko_action_map_set. */
    CHECK(ponko_ctx_feed_input(ctx, 0, 0, PONKO_KEY_J, 1) == PONKO_OK);
    CHECK(ponko_ctx_feed_input(ctx, 3, 0, PONKO_GAMEPAD_LEFT_TRIGGER_AXIS, 0.9f) == PONKO_OK);
    PonkoActionBinding fresh[4];
    size_t fresh_count = 0;
    CHECK(ponko_ctx_pressed_this_frame(ctx, 0.0f, fresh, 4, &fresh_count) == PONKO_OK);
    /* Space went down above too: keys in key order, then the axis. */
    CHECK(fresh_count == 3 && fresh[0].kind == PONKO_ACTION_KEY && fresh[0].first == PONKO_KEY_J);
    CHECK(fresh[1].kind == PONKO_ACTION_KEY && fresh[1].first == PONKO_KEY_SPACE);
    CHECK(fresh[2].kind == PONKO_ACTION_GAMEPAD_AXIS && fresh[2].first == PONKO_GAMEPAD_LEFT_TRIGGER_AXIS);
    CHECK(fresh[2].deadzone > 0.0f && fresh[2].deadzone < PONKO_PRESS_THRESHOLD);
    CHECK(ponko_ctx_pressed_this_frame(ctx, -1.0f, fresh, 4, &fresh_count) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_feed_input(ctx, 0, 0, PONKO_KEY_J, 0) == PONKO_OK);
    CHECK(ponko_ctx_feed_input(ctx, 3, 0, PONKO_GAMEPAD_LEFT_TRIGGER_AXIS, 0) == PONKO_OK);

    /* The try_ loaders: kind and path, never a caught panic. */
    PonkoTexture texture;
    PonkoFont font;
    PonkoSound sound;
    PonkoMesh mesh;
    CHECK(ponko_ctx_try_load_texture(ctx, "no/such.png", &texture) == PONKO_NOT_FOUND);
    CHECK(ponko_last_error_kind() == PONKO_ERROR_NOT_FOUND && strcmp(ponko_last_error_path(), "no/such.png") == 0);
    CHECK(strcmp(ponko_error_kind_name(PONKO_ERROR_NOT_FOUND), "not found") == 0);
    CHECK(ponko_ctx_try_load_font(ctx, "no/such.ttf", &font) == PONKO_NOT_FOUND);
    CHECK(ponko_ctx_try_load_sound(ctx, "no/such.ogg", &sound) == PONKO_NOT_FOUND);
    CHECK(ponko_ctx_try_load_stream(ctx, "no/such.ogg", &sound) == PONKO_NOT_FOUND);
    CHECK(ponko_ctx_try_load_mesh(ctx, "no/such.glb", &mesh) == PONKO_NOT_FOUND);

    /* Asset sources: memory and callbacks, then your own decoder. */
    PonkoMemorySource* memory = ponko_memory_source_new("embedded");
    CHECK(memory && ponko_memory_source_insert(memory, "level.txt", (const uint8_t*)"##.", 3) == PONKO_OK);
    CHECK(ponko_ctx_add_memory_source(ctx, memory) == PONKO_OK);
    ponko_memory_source_free(memory);
    CHECK(ponko_ctx_add_asset_source(ctx, "pak", pak_read, NULL, pak_free, pak) == PONKO_OK);
    Decoded decoded = {0, 0};
    CHECK(ponko_ctx_load_asset_with(ctx, "level.txt", decode_bytes, &decoded) == PONKO_OK);
    CHECK(decoded.len == 3 && decoded.first == '#');
    CHECK(ponko_ctx_load_asset_with(ctx, pak->name, decode_bytes, &decoded) == PONKO_OK && decoded.len == pak->len);
    CHECK(ponko_ctx_load_asset_with(ctx, "missing.bin", decode_bytes, &decoded) == PONKO_NOT_FOUND);
    CHECK(strcmp(ponko_last_error_path(), "missing.bin") == 0);

    /* A named span: from line 1 to the second line called "edge". */
    PonkoUiTemplateTrack columns[4];
    for (int i = 0; i < 4; ++i) columns[i] = (PonkoUiTemplateTrack){{PONKO_UI_TRACK_POINTS, 100}, 0, 0, NULL, 0, NULL, 0};
    PonkoUiLayout grid = ponko_ui_layout_default();
    grid.display = PONKO_UI_DISPLAY_GRID;
    grid.grid_template_columns = columns;
    grid.grid_template_column_count = 4;
    uint64_t root, item;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &grid, &root) == PONKO_OK);
    const char* lines[5] = {"", "", "edge", "", "edge"};
    CHECK(ponko_ctx_ui_set_grid_line_names(ctx, PONKO_UI_SCREEN, root, 0, lines, 5) == PONKO_OK);
    PonkoUiLayout cell = ponko_ui_layout_default();
    cell.grid_column_start = (PonkoUiGridLine){PONKO_UI_GRID_LINE, 1};
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, root, &cell, &item) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_grid_named_span(ctx, PONKO_UI_SCREEN, item, 0, true, "edge", 2) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_grid_named_span(ctx, PONKO_UI_SCREEN, item, 0, true, "edge", 0) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, root, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_layout(ctx, PONKO_UI_SCREEN, (PonkoVec2){800, 600}) == PONKO_OK);
    PonkoRect r;
    CHECK(ponko_ctx_ui_rect(ctx, PONKO_UI_SCREEN, item, &r) == PONKO_OK && r.min.x == 0 && r.max.x == 400);
}
typedef struct { uint32_t points; } Points;
static void add_points(PonkoWorld* world, void* value, void* user) {
    Points* points = value;
    Points probe;
    CHECK(ponko_resource_get(world, "Points", &probe, sizeof probe) == PONKO_NOT_FOUND); /* lent out */
    points->points += *(uint32_t*)user;
}
static void scenario_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoActionMap* map = ponko_action_map_new();
    PonkoActionBinding space = {PONKO_ACTION_KEY, 0, PONKO_KEY_SPACE, 0, 0, 0, 0};
    CHECK(ponko_action_map_set(map, "jump", 0, &space, 1) == PONKO_OK);
    CHECK(ponko_ctx_set_actions(ctx, map) == PONKO_OK);
    ponko_action_map_free(map);
    /* Count checks name a reflected component. */
    PonkoWorld* w = ponko_ctx_world(ctx);
    PonkoField field = {"points", PONKO_FIELD_U32, 0};
    uint32_t coin;
    CHECK(ponko_component_register(w, "Coin", 4, 4, &coin) == PONKO_OK);
    CHECK(ponko_component_reflect(w, coin, &field, 1) == PONKO_OK);
}
static void native_coverage(void) {
    static const uint8_t blob[] = {1, 2, 3, 4};
    Pak pak = {"blob.bin", blob, sizeof blob, 0};
    PonkoGame* game = ponko_game_new("c natives", 1, 1);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 0, native_setup, &pak, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    CHECK(pak.frees == 1);

    /* resource_scope: the resource and the world, lent together. */
    w = ponko_world_new();
    Points points = {1};
    uint32_t add = 41;
    CHECK(ponko_resource_set(w, "Points", &points, sizeof points, _Alignof(Points)) == PONKO_OK);
    CHECK(ponko_resource_scope(w, "Points", sizeof points, add_points, &add) == PONKO_OK);
    CHECK(ponko_resource_get(w, "Points", &points, sizeof points) == PONKO_OK && points.points == 42);
    CHECK(ponko_resource_scope(w, "Nothing", 4, add_points, &add) == PONKO_NOT_FOUND);
    CHECK(ponko_world_free(w) == PONKO_OK);

    /* Input scenarios: Space held on ticks 2-4, checked through the action. */
    PonkoInputScenario* s = ponko_input_scenario_new();
    CHECK(ponko_input_scenario_at(s, 2, 0, 0, PONKO_KEY_SPACE, 1) == PONKO_OK);
    CHECK(ponko_input_scenario_at(s, 5, 0, 0, PONKO_KEY_SPACE, 0) == PONKO_OK);
    CHECK(ponko_input_scenario_expect_action(s, 2, "jump", 1, 1, -1) == PONKO_OK);
    CHECK(ponko_input_scenario_expect_action(s, 5, "jump", -1, 0, 1) == PONKO_OK);
    CHECK(ponko_input_scenario_expect_count(s, 5, "Coin", 0) == PONKO_OK);
    CHECK(ponko_input_scenario_expect_resource(s, 5, "Score", "/points", "3") == PONKO_OK);
    CHECK(ponko_input_scenario_named(s, "jump") == PONKO_OK);
    CHECK(ponko_input_scenario_duration(s, 6) == PONKO_OK);
    CHECK(ponko_input_scenario_release_when_done(s) == PONKO_OK);
    uint64_t ticks = 0;
    size_t steps = 0, len = 0;
    CHECK(ponko_input_scenario_info(s, &ticks, &steps) == PONKO_OK && ticks == 6 && steps == 6);
    CHECK(ponko_input_scenario_to_json(s, NULL, 0, &len) == PONKO_OK && len > 0);
    char* json = malloc(len + 1);
    CHECK(ponko_input_scenario_to_json(s, json, len + 1, &len) == PONKO_OK);
    PonkoInputScenario* copy = NULL;
    CHECK(ponko_input_scenario_from_json(json, &copy) == PONKO_OK && copy);
    free(json);
    CHECK(ponko_input_scenario_from_json("{", &copy) == PONKO_INVALID_ARGUMENT && ponko_last_error_kind() == PONKO_ERROR_JSON);
    game = ponko_game_new("c scenario", 1, 1);
    CHECK(ponko_game_input_scenario(game, copy) == PONKO_OK);
    ponko_input_scenario_free(copy);
    ponko_input_scenario_free(s);
    CHECK(ponko_game_run_headless(game, ticks, scenario_setup, NULL, &w) == PONKO_OK);
    PonkoScenarioReport report;
    CHECK(ponko_world_scenario_report(w, &report) == PONKO_OK);
    /* Steps 0-3 held; the Score resource was never added, so step 5 fails on tick 5 (the last). */
    CHECK(report.status == PONKO_SCENARIO_FAILED && report.failed && report.failure_step == 5);
    CHECK(report.inputs_applied == 2 && report.checks_passed == 3);
    CHECK(ponko_world_scenario_report_json(w, NULL, 0, &len) == PONKO_OK && len > 0);
    CHECK(ponko_world_free(w) == PONKO_OK);
    bool was_running = true;
    CHECK(ponko_ctx_cancel_scenario(NULL, &was_running) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_start_scenario(NULL, NULL) == PONKO_INVALID_ARGUMENT);
}

/* Headless frames: a form typed into and clicked by node name. */
static void form_setup(PonkoCtx* ctx, void* user) {
    (void)user;
    PonkoUiLayout fill = ponko_ui_layout_fill(), box = ponko_ui_layout_fixed(200, 30);
    uint64_t root = 0, name = 0, ok = 0;
    CHECK(ponko_ctx_ui_add_panel(ctx, PONKO_UI_SCREEN, PONKO_UI_NONE, &fill, &root) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_text_input(ctx, PONKO_UI_SCREEN, root, "", &box, &name) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_button(ctx, PONKO_UI_SCREEN, root, "OK", &box, &ok) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_name(ctx, PONKO_UI_SCREEN, root, "form") == PONKO_OK);
    CHECK(ponko_ctx_ui_set_name(ctx, PONKO_UI_SCREEN, name, "form.name") == PONKO_OK);
    CHECK(ponko_ctx_ui_set_name(ctx, PONKO_UI_SCREEN, ok, "form.ok") == PONKO_OK);
    bool shown = true;
    CHECK(ponko_ctx_ui_is_shown(ctx, PONKO_UI_SCREEN, ok, &shown) == PONKO_OK && !shown);
    CHECK(ponko_ctx_ui_push_screen(ctx, PONKO_UI_SCREEN, root, false) == PONKO_OK);
    CHECK(ponko_ctx_ui_is_shown(ctx, PONKO_UI_SCREEN, ok, &shown) == PONKO_OK && shown);
    CHECK(ponko_ctx_ui_is_shown(ctx, PONKO_UI_SCREEN, 9999, &shown) == PONKO_NOT_FOUND);
}
/* Frame system: OK closes the form; `user` counts the closes. */
static void form_frame(PonkoCtx* ctx, void* user) {
    uint64_t ok = PONKO_UI_NONE, clicked = PONKO_UI_NONE, root = PONKO_UI_NONE;
    CHECK(ponko_ctx_ui_find(ctx, PONKO_UI_SCREEN, "form.ok", &ok) == PONKO_OK);
    CHECK(ponko_ctx_ui_take_click(ctx, PONKO_UI_SCREEN, &clicked) == PONKO_OK);
    if (clicked != PONKO_UI_NONE && clicked == ok) {
        CHECK(ponko_ctx_ui_pop_screen(ctx, PONKO_UI_SCREEN, &root) == PONKO_OK);
        ++*(int*)user;
    }
}
static void headless_frames_ui(void) {
    PonkoInputScenario* s = ponko_input_scenario_new();
    CHECK(ponko_input_scenario_type_text(s, 2, "form.name", "Ada") == PONKO_OK);
    CHECK(ponko_input_scenario_expect_ui(s, 2, "form.name", 1, 1, 1, "Ada") == PONKO_OK);
    CHECK(ponko_input_scenario_click(s, 4, "form.ok") == PONKO_OK);
    CHECK(ponko_input_scenario_expect_ui(s, 4, "form", -1, 0, -1, NULL) == PONKO_OK);
    CHECK(ponko_input_scenario_expect_ui(s, 4, "form.cancel", 0, -1, -1, NULL) == PONKO_OK);
    CHECK(ponko_input_scenario_expect_ui(s, 4, "form", 2, -1, -1, NULL) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_input_scenario_duration(s, 6) == PONKO_OK);
    int closed = 0;
    PonkoGame* game = ponko_game_new("c frames", 640, 480);
    CHECK(ponko_game_headless_frames(game, true) == PONKO_OK);
    CHECK(ponko_game_system(game, "form", form_frame, &closed) == PONKO_OK);
    CHECK(ponko_game_input_scenario(game, s) == PONKO_OK);
    ponko_input_scenario_free(s);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 6, form_setup, NULL, &w) == PONKO_OK);
    PonkoScenarioReport report;
    CHECK(ponko_world_scenario_report(w, &report) == PONKO_OK);
    CHECK(report.status == PONKO_SCENARIO_PASSED && report.inputs_applied == 2 && report.checks_passed == 3);
    CHECK(closed == 1);
    CHECK(ponko_world_free(w) == PONKO_OK);
    CHECK(ponko_game_headless_frames(NULL, true) == PONKO_INVALID_ARGUMENT);
}

/* A menu that opens on its Play button, a HUD that Escape leaves up, and a
   glTF file made solid in one call. */
static void focus_and_hud_setup(PonkoCtx* ctx, void* user) {
    const char* path = user;
    const uint64_t s = PONKO_UI_SCREEN;
    PonkoUiLayout column = ponko_ui_layout_default();
    column.flex_direction = PONKO_UI_FLEX_COLUMN;
    column.width = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 300};
    column.height = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 200};
    PonkoUiLayout row = ponko_ui_layout_default();
    row.width = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 120};
    row.height = (PonkoUiLength){PONKO_UI_LENGTH_POINTS, 40};
    uint64_t hud, count, menu, play, quit;
    CHECK(ponko_ctx_ui_add_panel(ctx, s, PONKO_UI_NONE, &column, &hud) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_label(ctx, s, hud, "0 / 5", &row, &count) == PONKO_OK);
    CHECK(ponko_ctx_ui_show_hud(ctx, s, hud) == PONKO_OK);
    bool flag = false;
    CHECK(ponko_ctx_ui_is_hud(ctx, s, hud, &flag) == PONKO_OK && flag);
    CHECK(ponko_ctx_ui_wants_keyboard(ctx, &flag) == PONKO_OK && !flag);
    CHECK(ponko_ctx_ui_push_screen(ctx, s, hud, false) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_ctx_ui_show_hud(ctx, s, count) == PONKO_INVALID_ARGUMENT);

    CHECK(ponko_ctx_ui_add_panel(ctx, s, PONKO_UI_NONE, &column, &menu) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_button(ctx, s, menu, "Quit", &row, &quit) == PONKO_OK);
    CHECK(ponko_ctx_ui_add_button(ctx, s, menu, "Play", &row, &play) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_initial_focus(ctx, s, play, true) == PONKO_OK);
    CHECK(ponko_ctx_ui_set_focus(ctx, s, play) == PONKO_INVALID_ARGUMENT); /* not on a screen yet */
    CHECK(ponko_ctx_ui_push_screen(ctx, s, menu, true) == PONKO_OK);
    uint64_t focus = 0;
    CHECK(ponko_ctx_ui_focus(ctx, s, &focus) == PONKO_OK && focus == play);
    CHECK(ponko_ctx_ui_set_focus(ctx, s, count) == PONKO_INVALID_ARGUMENT); /* a label on a HUD */
    CHECK(ponko_ctx_ui_set_focus(ctx, s, 9999) == PONKO_NOT_FOUND);
    PonkoVec2 window = {800, 600};
    PonkoUiInput enter = ponko_ui_input_default();
    enter.confirm = true;
    CHECK(ponko_ctx_ui_frame(ctx, s, window, &enter) == PONKO_OK);
    PonkoUiEvent event;
    CHECK(ponko_ctx_ui_next_event(ctx, s, &event) == PONKO_OK);
    CHECK(event.kind == PONKO_UI_EVENT_CLICKED && event.id == play);
    CHECK(ponko_ctx_ui_set_focus(ctx, s, quit) == PONKO_OK);
    CHECK(ponko_ctx_ui_clear_focus(ctx, s) == PONKO_OK);
    CHECK(ponko_ctx_ui_focus(ctx, s, &focus) == PONKO_OK && focus == PONKO_UI_NONE);

    /* Escape closes the menu; the HUD stays. */
    PonkoUiInput escape = ponko_ui_input_default();
    escape.cancel = true;
    CHECK(ponko_ctx_ui_frame(ctx, s, window, &escape) == PONKO_OK);
    CHECK(ponko_ctx_ui_is_shown(ctx, s, count, &flag) == PONKO_OK && flag);
    CHECK(ponko_ctx_ui_is_shown(ctx, s, play, &flag) == PONKO_OK && !flag);
    CHECK(ponko_ctx_ui_hide_hud(ctx, s, hud) == PONKO_OK);
    CHECK(ponko_ctx_ui_is_shown(ctx, s, count, &flag) == PONKO_OK && !flag);

    /* A 4 x 4 m floor at y = 0, solid in one call, scaled or as a hull. */
    PonkoCollisionMesh floor, again, wide;
    PonkoVec3 one = {1, 1, 1}, twice = {2, 1, 2};
    CHECK(ponko_ctx_load_collision_mesh(ctx, path, one, false, &floor) == PONKO_OK);
    CHECK(ponko_ctx_load_collision_mesh(ctx, path, one, false, &again) == PONKO_OK && again.id == floor.id);
    CHECK(ponko_ctx_load_collision_mesh(ctx, path, twice, false, &wide) == PONKO_OK && wide.id != floor.id);
    CHECK(ponko_ctx_load_collision_mesh(ctx, path, one, true, &wide) == PONKO_INVALID_ARGUMENT); /* a flat hull */
    CHECK(ponko_ctx_load_collision_mesh(ctx, "no/such.glb", one, false, &wide) == PONKO_NOT_FOUND);
    CHECK(ponko_ctx_load_collision_mesh(ctx, path, one, false, NULL) == PONKO_INVALID_ARGUMENT);
}
static void focus_huds_and_collision_files(void) {
    char path[1024];
    snprintf(path, sizeof path, "%s/ponko-c-floor.gltf", temp_dir());
    FILE* f = fopen(path, "w");
    CHECK(f);
    fputs("{\"asset\": {\"version\": \"2.0\"}, \"scene\": 0, \"scenes\": [{\"nodes\": [0]}], \"nodes\": [{\"mesh\": 0}],"
          " \"meshes\": [{\"primitives\": [{\"attributes\": {\"POSITION\": 0}, \"indices\": 1}]}],"
          " \"buffers\": [{\"byteLength\": 60, \"uri\": \"data:application/octet-stream;base64,"
          "AAAAwAAAAAAAAABAAAAAQAAAAAAAAABAAAAAQAAAAAAAAADAAAAAwAAAAAAAAADAAAABAAIAAAACAAMA\"}],"
          " \"bufferViews\": [{\"buffer\": 0, \"byteLength\": 48}, {\"buffer\": 0, \"byteOffset\": 48, \"byteLength\": 12}],"
          " \"accessors\": [{\"bufferView\": 0, \"componentType\": 5126, \"count\": 4, \"type\": \"VEC3\","
          " \"min\": [-2, 0, -2], \"max\": [2, 0, 2]}, {\"bufferView\": 1, \"componentType\": 5123, \"count\": 6, \"type\": \"SCALAR\"}]}",
          f);
    fclose(f);
    PonkoGame* game = ponko_game_new("c focus and huds", 1, 1); CHECK(game);
    CHECK(ponko_game_physics3d(game, (PonkoVec3){0, -9.81f, 0}) == PONKO_OK);
    PonkoWorld* w = NULL;
    CHECK(ponko_game_run_headless(game, 0, focus_and_hud_setup, path, &w) == PONKO_OK);
    CHECK(ponko_world_free(w) == PONKO_OK);
    remove(path);
}
int main(void) {
    change_detection();
    declared_access();
    command_barriers();
    steam_messages_validation();
    morph_clip_api();
    replication();
    CHECK(ponko_abi_version() == 2);
    /* The macro in the header and the loaded library agree. */
    CHECK(ponko_abi_version() == PONKO_ABI_VERSION);
    PonkoWorld* w = ponko_world_new(); CHECK(w);
    uint32_t p; CHECK(ponko_component_register(w, "Pos", sizeof(Position), _Alignof(Position), &p) == PONKO_OK);
    Position initial = {7}; PonkoValue value = {p, &initial, sizeof(initial)};
    uint64_t e; CHECK(ponko_spawn(w, &value, 1, &e) == PONKO_OK);
    Context context = {w, 0, e}; CHECK(ponko_query(w, &p, 1, visit, &context) == PONKO_OK); CHECK(context.seen == 1);
    Position result; CHECK(ponko_get(w, e, p, &result, sizeof(result)) == PONKO_OK && result.x == 9);
    CHECK(ponko_get(w, e, p, &result, 0) == PONKO_INVALID_ARGUMENT);
    uint32_t duplicate[] = {p, p}; CHECK(ponko_query(w, duplicate, 2, visit, &context) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_despawn(w, e) == PONKO_OK); bool alive = true;
    CHECK(ponko_is_alive(w, e, &alive) == PONKO_OK && !alive);
    CHECK(ponko_despawn(w, e) == PONKO_NOT_FOUND);
    CHECK(ponko_world_free(w) == PONKO_OK);

    w = ponko_world_new();
    uint32_t probe_id;
    CHECK(ponko_reflection_probe3d_component(w, &probe_id) == PONKO_OK);
    PonkoReflectionProbe3D probe = ponko_reflection_probe3d_default();
    CHECK(probe.content == 0 && probe.box_projection && probe.size.x == 10);
    PonkoValue probe_value = {probe_id, &probe, sizeof(probe)};
    CHECK(ponko_spawn(w, &probe_value, 1, &e) == PONKO_OK);
    PonkoReflectionProbe3D probe_out;
    CHECK(ponko_get(w, e, probe_id, &probe_out, sizeof(probe_out)) == PONKO_OK && probe_out.blend_distance == 1);
    uint32_t decal_id, receives_id;
    CHECK(ponko_decal3d_component(w, &decal_id) == PONKO_OK);
    CHECK(ponko_receives_decals_component(w, &receives_id) == PONKO_OK);
    PonkoDecal3D decal = ponko_decal3d_default();
    CHECK(decal.opacity == 1 && decal.image == 0 && decal.normal_image == PONKO_DECAL_NO_IMAGE && decal.layers == 0xFFFF);
    decal.size.x = 2; decal.order = 4;
    PonkoReceivesDecals none = {0};
    PonkoValue decal_values[] = {{decal_id, &decal, sizeof(decal)}, {receives_id, &none, sizeof(none)}};
    CHECK(ponko_spawn(w, decal_values, 2, &e) == PONKO_OK);
    PonkoDecal3D decal_out; PonkoReceivesDecals none_out = {1};
    CHECK(ponko_get(w, e, decal_id, &decal_out, sizeof(decal_out)) == PONKO_OK && decal_out.size.x == 2 && decal_out.order == 4);
    CHECK(ponko_get(w, e, receives_id, &none_out, sizeof(none_out)) == PONKO_OK && none_out.layers == 0);
    uint32_t point_id, spot_id;
    CHECK(ponko_point_light3d_component(w, &point_id) == PONKO_OK);
    CHECK(ponko_spot_light3d_component(w, &spot_id) == PONKO_OK);
    PonkoPointLight3D point = {{1, .5f, .1f, 1}, 20, 12, true, 512};
    PonkoSpotLight3D spot = {{.1f, .5f, 1, 1}, 30, 18, .2f, .5f, true};
    PonkoValue lights[] = {{point_id, &point, sizeof(point)}, {spot_id, &spot, sizeof(spot)}};
    CHECK(ponko_spawn(w, lights, 2, &e) == PONKO_OK);
    PonkoPointLight3D point_out;
    PonkoSpotLight3D spot_out;
    CHECK(ponko_get(w, e, point_id, &point_out, sizeof(point_out)) == PONKO_OK && point_out.intensity == 20);
    CHECK(point_out.shadow_resolution == 512 && point_out.shadows);
    CHECK(ponko_get(w, e, spot_id, &spot_out, sizeof(spot_out)) == PONKO_OK && spot_out.outer_angle == .5f);
    CHECK(ponko_world_free(w) == PONKO_OK);

    Run r = {0, 0, PONKO_OK};
    PonkoGame* game = ponko_game_new("c tests", 1, 1); CHECK(game);
    CHECK(ponko_game_fixed_hz(game, 50) == PONKO_OK);
    CHECK(ponko_game_fixed_hz(game, 0) == PONKO_INVALID_ARGUMENT);
    CHECK(ponko_game_sim_system(game, "tick", 1, tick, &r) == PONKO_OK);
    PonkoWorld* sim = NULL;
    CHECK(ponko_game_run_headless(game, 100, setup, NULL, &sim) == PONKO_OK);
    CHECK(r.ticks == 3 && r.dt > .019f && r.dt < .021f && sim);
    CHECK(ponko_world_free(sim) == PONKO_OK);
    PonkoDrawParams params = ponko_draw_params_default();
    CHECK(params.scale.x == 1 && params.pivot.x == .5f && !params.screen);
    vehicles();
    phases();
    plugins_setups_and_system_ids();
    retained_ui();
    ui_panels();
    ribbons_and_decals();
    lod_helpers();
    streamed_worlds();
    model_material_overrides();
    asset_streaming();
    profiling_c();
    animation_graphs();
    query_filters();
    query_cache();
    wide_queries();
    native_coverage();
    headless_frames_ui();
    focus_huds_and_collision_files();
    return 0;
}
