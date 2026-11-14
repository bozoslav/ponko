//! An endless landscape streamed in around the camera: 256 m terrain
//! tiles built on worker threads ([`Job`]), uploaded a couple a frame,
//! and freed again once they're far behind ([`CellStreamer`]).
//!
//!   `cargo run --release --example streaming --features 3d`
//!
//! The camera flies on by itself; the arrows steer, W and S change speed,
//! Q and E go down and up. `SHOT=path.png` saves frame 400 and quits.
//! `BENCH=1` flies at top speed for 3000 frames, prints what the streaming cost on the
//! game's thread, and quits.

use std::collections::HashMap;

#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::*;
use ponko::prelude::*;
use ponko::render::{Material, MaterialDesc, Sky};
use ponko::terrain::{TerrainTile, TerrainTileData};

const N: usize = 129;
const TILE: f32 = 256.0;
/// Tiles within this distance load; beyond `KEEP` they're freed.
const LOAD: f32 = 1400.0;
const KEEP: f32 = 1700.0;
/// How many tile jobs start, and finished tiles upload, per frame.
const STARTS: usize = 4;
const UPLOADS: usize = 2;

fn noise(x: f32, z: f32) -> f32 {
    let hash = |i: i32, j: i32| {
        let h = (i.wrapping_mul(374_761_393) ^ j.wrapping_mul(668_265_263)) as u32;
        let h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
        (h >> 8) as f32 / (1u32 << 24) as f32
    };
    let (i, j) = (x.floor() as i32, z.floor() as i32);
    let (fx, fz) = (x - x.floor(), z - z.floor());
    let (sx, sz) = (fx * fx * (3.0 - 2.0 * fx), fz * fz * (3.0 - 2.0 * fz));
    let top = hash(i, j) + (hash(i + 1, j) - hash(i, j)) * sx;
    let bottom = hash(i, j + 1) + (hash(i + 1, j + 1) - hash(i, j + 1)) * sx;
    top + (bottom - top) * sz
}

/// Rolling hills with mountain ranges through them.
fn height(x: f32, z: f32) -> f32 {
    let mut h = 0.0;
    let (mut scale, mut amp) = (1.0 / 700.0, 130.0);
    for _ in 0..6 {
        h += (noise(x * scale, z * scale) - 0.5) * amp;
        scale *= 2.1;
        amp *= 0.45;
    }
    let ridge = 1.0 - (noise(x / 1800.0 + 7.3, z / 1800.0 - 2.1) - 0.5).abs() * 2.0;
    h + ridge.powi(3) * 90.0
}

/// The ground's colors from its height and slope: grass, rock, snow.
const GROUND: &str = "
fn surface(s: Surface) -> Surface {
    var out = s;
    let steep = 1.0 - s.normal.y;
    let grain = noise2(s.world.xz * 0.35) * 0.5 + noise2(s.world.xz * 0.05) * 0.5;
    let grass = mix(vec3<f32>(0.20, 0.36, 0.13), vec3<f32>(0.33, 0.42, 0.18), grain);
    let rock = mix(vec3<f32>(0.33, 0.31, 0.29), vec3<f32>(0.48, 0.46, 0.43), grain);
    var c = mix(grass, rock, smoothstep(0.18, 0.32, steep));
    let snow_line = 95.0 + grain * 20.0;
    c = mix(c, vec3<f32>(0.92, 0.94, 0.97), smoothstep(snow_line, snow_line + 8.0, s.world.y) * (1.0 - smoothstep(0.35, 0.55, steep)));
    out.albedo = c;
    out.roughness = 0.9;
    return out;
}";

/// A cell's tile: still being built, or in the world.
enum Tile {
    Building(ponko::streaming::Job<TerrainTileData>),
    Live(Entity, TerrainTile),
}

/// Heights for the tile whose corner is `corner`, 2 m apart.
fn tile_data(corner: Vec3) -> TerrainTileData {
    let heights: Vec<f32> = (0..N * N)
        .map(|i| height(corner.x + (i % N) as f32 * 2.0, corner.z + (i / N) as f32 * 2.0))
        .collect();
    TerrainTileData::new(&heights, TILE)
}

fn main() {
    let bench = std::env::var("BENCH").is_ok();
    let shot = std::env::var("SHOT").ok();
    let mut cells = ponko::streaming::CellStreamer::new(TILE, LOAD, KEEP);
    let mut tiles: HashMap<(i32, i32), Tile> = HashMap::new();
    let mut ground: Option<Material> = None;
    let (mut yaw, mut speed, mut altitude) = (0.4f32, if bench { 600.0f32 } else { 120.0 }, 90.0f32);
    // The streaming's cost on the game's thread, for BENCH.
    let (mut uploaded, mut freed, mut upload_time, mut worst_upload) = (0usize, 0usize, 0.0f64, 0.0f64);
    Game::new("Ponko: streaming", 1280, 720)
        .vsync(!bench)
        .system(move |ctx: &mut Ctx| {
            if ctx.input.key_pressed(Key::Escape) {
                ctx.quit();
            }
            let dt = ctx.time.dt();
            if ctx.input.key_down(Key::Left) {
                yaw += dt * 0.8;
            }
            if ctx.input.key_down(Key::Right) {
                yaw -= dt * 0.8;
            }
            if ctx.input.key_down(Key::W) {
                speed = (speed + 80.0 * dt).min(600.0);
            }
            if ctx.input.key_down(Key::S) {
                speed = (speed - 80.0 * dt).max(0.0);
            }
            if ctx.input.key_down(Key::E) {
                altitude += 40.0 * dt;
            }
            if ctx.input.key_down(Key::Q) {
                altitude = (altitude - 40.0 * dt).max(5.0);
            }
            // Fly on, keeping above the hills ahead.
            let rotation = Quat::from_rotation_y(yaw);
            let mut at = ctx.scene3d.camera.position + rotation * Vec3::NEG_Z * speed * dt;
            let ahead = at + rotation * Vec3::NEG_Z * 120.0;
            let floor = height(at.x, at.z).max(height(ahead.x, ahead.z));
            at.y += (floor + altitude - at.y) * (dt * 1.5).min(1.0);
            ctx.scene3d.camera.position = at;
            ctx.scene3d.camera.rotation = rotation * Quat::from_rotation_x(-0.22);

            // Which tiles to start and drop.
            cells.update(at, STARTS);
            for &cell in cells.to_load() {
                let corner = cells.corner(cell);
                tiles.insert(cell, Tile::Building(ponko::streaming::Job::spawn(move || tile_data(corner))));
            }
            for cell in cells.to_drop() {
                // A job still building is dropped with its result.
                if let Some(Tile::Live(entity, tile)) = tiles.remove(cell) {
                    ctx.world.despawn(entity);
                    ctx.free(tile);
                    freed += 1;
                }
            }
            // Finished tiles go on the GPU, a few a frame.
            let ground = *ground.get_or_insert_with(|| {
                let shader = ctx.gfx.create_shader("ground", GROUND);
                ctx.gfx.create_material(MaterialDesc::color(Color::WHITE).with_shader(shader))
            });
            let mut uploads = 0;
            for (&cell, tile) in tiles.iter_mut() {
                if uploads == UPLOADS {
                    break;
                }
                if let Tile::Building(job) = tile
                    && let Some(data) = job.take()
                {
                    let start = ponko::perf::Instant::now();
                    let made = ctx.gfx.upload_terrain_tile(&data, ground);
                    let entity = ctx.world.spawn((Transform::at(cells.corner(cell)), made));
                    let took = start.elapsed().as_secs_f64() * 1000.0;
                    upload_time += took;
                    worst_upload = worst_upload.max(took);
                    uploaded += 1;
                    uploads += 1;
                    *tile = Tile::Live(entity, made);
                }
            }

            if ctx.time.frame() == 400
                && let Some(path) = &shot
            {
                ctx.gfx.save_screenshot(path);
            }
            if ctx.time.frame() == 402 && shot.is_some() {
                ctx.quit();
            }
            if bench && ctx.time.frame() == 3000 {
                let live = tiles.values().filter(|t| matches!(t, Tile::Live(..))).count();
                println!(
                    "{uploaded} tiles uploaded ({:.2} ms each, worst {worst_upload:.2} ms), {freed} freed, {live} live; {:?}",
                    upload_time / uploaded.max(1) as f64,
                    ctx.perf.render_stats()
                );
                ctx.quit();
            }
        })
        .setup(move |ctx| {
            ctx.scene3d.post.bloom = 0.08;
            ctx.scene3d.lighting.sun_direction = Sky::sun_direction_at(15.5);
            ctx.scene3d.lighting.shadow_distance = 500.0;
            ctx.scene3d.camera.far = LOAD + TILE;
            // Fog thick enough that tiles appear behind it, not at an edge.
            ctx.scene3d.sky.fog_density = 0.0007;
            ctx.scene3d.sky.fog_height = 40.0;
            ctx.scene3d.sky.fog_falloff = 90.0;
            ctx.scene3d.camera.position = vec3(0.0, height(0.0, 0.0) + 60.0, 0.0);
        }).run();
}
