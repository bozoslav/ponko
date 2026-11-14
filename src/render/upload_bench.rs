//! Offscreen benchmark of what the 3D renderer uploads per frame: bytes by
//! kind, buffers made, and the CPU time of preparing the frame, for a few
//! mostly static scenes. Run explicitly, in release:
//! `cargo test --release --lib --features 3d upload_bench -- --ignored --nocapture`.
use super::buffer_render_test::Rig;
use super::traffic;
use crate::d3::{
    Line as Line3D, ParticleEmitter as ParticleEmitter3D, ParticleSettings as ParticleSettings3D,
    Transform as Transform3D,
};
use crate::particles::ParticleSpace as ParticleSpace3D;
use crate::prelude::*;
use crate::render::{AntiAliasing, GpuUploads, MaterialDesc, Mesh, MeshData, Model};

/// `n` × `n` cubes on the ground under a camera, with sun shadows.
fn ground(n: i32) -> Option<(Rig, Vec<Entity>, Mesh)> {
    let mut rig = Rig::new()?;
    let mesh = rig.mesh(
        &MeshData::cube(0.4),
        MaterialDesc {
            color: Color::WHITE,
            ..Default::default()
        },
    );
    let mut cubes = Vec::new();
    for x in 0..n {
        for z in 0..n {
            let at = vec3(x as f32 - n as f32 / 2.0, 0.0, z as f32 - n as f32 / 2.0);
            cubes.push(rig.ctx.world.spawn((Transform3D::at(at), Model::new(mesh))));
        }
    }
    rig.ctx.scene3d.camera.position = vec3(0.0, n as f32 * 0.6, n as f32 * 0.6);
    rig.ctx.scene3d.camera.look_at(Vec3::ZERO);
    Some((rig, cubes, mesh))
}

/// Draws `frames` frames after 10 warm-up ones with the clock running,
/// calling `change` before each; prints the averages per frame.
fn measure(name: &str, rig: &mut Rig, frames: usize, mut change: impl FnMut(&mut Rig, usize)) {
    for i in 0..10 {
        rig.time = (i as f32 / 60.0, 1.0 / 60.0);
        change(rig, i);
        rig.draw();
    }
    traffic::take();
    traffic::take_writes();
    let mut total = GpuUploads::default();
    let mut prepare = Vec::with_capacity(frames);
    let mut writes = Vec::with_capacity(frames);
    for i in 10..10 + frames {
        rig.time = (i as f32 / 60.0, 1.0 / 60.0);
        change(rig, i);
        rig.draw();
        prepare.push(rig.prepare_ms);
        let c = traffic::take();
        writes.push(traffic::take_writes().iter().sum::<u32>());
        total.instances += c.instances;
        total.uniforms += c.uniforms;
        total.lights += c.lights;
        total.static_batches += c.static_batches;
        total.skins += c.skins;
        total.other += c.other;
        total.buffers_created += c.buffers_created;
        total.buffer_bytes_created += c.buffer_bytes_created;
    }
    prepare.sort_by(f64::total_cmp);
    writes.sort_unstable();
    let n = frames as u64;
    println!(
        "{name:<44} {:>9} B/frame  (instances {:>7}, uniforms {:>6}, other {:>6})  buffers made {:>3}  writes p50 {:>4}  prepare p50 {:.3} ms",
        (total.instances + total.uniforms + total.lights + total.static_batches + total.skins + total.other) / n,
        total.instances / n,
        total.uniforms / n,
        total.other / n,
        total.buffers_created,
        writes[frames / 2],
        prepare[frames / 2],
    );
}

/// What a write costs the CPU: `writes` writes of `size` bytes, spread
/// over a buffer, then a submit (which hands the staged copies over);
/// the median over 50 rounds, in microseconds.
fn write_cost(rig: &Rig, writes: usize, size: usize) -> f64 {
    let stride = size.max(1024);
    let buffer = rig.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("write cost"),
        size: (writes * stride) as u64,
        usage: wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let data = vec![7u8; size];
    let mut times = Vec::with_capacity(50);
    for _ in 0..60 {
        let start = std::time::Instant::now();
        for k in 0..writes {
            rig.queue.write_buffer(&buffer, (k * stride) as u64, &data);
        }
        rig.queue.submit([]);
        times.push(start.elapsed().as_secs_f64() * 1e6);
        rig.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
    }
    times.drain(..10);
    times.sort_by(f64::total_cmp);
    times[times.len() / 2]
}

#[test]
#[ignore = "requires a GPU; run explicitly in release for numbers"]
fn upload_benchmark() {
    const FRAMES: usize = 120;
    if let Some(rig) = Rig::new() {
        for (writes, size) in [
            (1, 64),
            (64, 64),
            (256, 64),
            (1, 16 << 10),
            (1, 64 << 10),
            (1, 256 << 10),
        ] {
            println!(
                "{writes:>4} writes of {size:>6} B: {:>8.1} µs",
                write_cost(&rig, writes, size)
            );
        }
    }
    let Some((mut rig, _, _)) = ground(32) else { return };
    measure("still, 1024 models", &mut rig, FRAMES, |_, _| {});

    let Some((mut rig, cubes, _)) = ground(32) else { return };
    rig.ctx.scene3d.post.anti_aliasing = AntiAliasing::Taa;
    measure("1% moving, 1024 models, TAA", &mut rig, FRAMES, |rig, i| {
        for &e in cubes.iter().step_by(100) {
            rig.ctx.world.get_mut::<Transform3D>(e).unwrap().position.y = (i % 20) as f32 * 0.02;
        }
    });

    let Some((mut rig, _, _)) = ground(32) else { return };
    measure("camera panning, 1024 models", &mut rig, FRAMES, |rig, i| {
        let x = (i as f32 * 0.05).sin() * 6.0;
        rig.ctx.scene3d.camera.position = vec3(x, 19.0, 19.0);
        rig.ctx.scene3d.camera.look_at(vec3(x, 0.0, 0.0));
    });

    let Some((mut rig, mut cubes, mesh)) = ground(32) else {
        return;
    };
    measure("5% respawned a frame, 1024 models", &mut rig, FRAMES, |rig, i| {
        for e in cubes.drain(..50) {
            rig.ctx.world.despawn(e);
        }
        for k in 0..50 {
            let at = vec3((k % 10) as f32 - 5.0, 0.0, (k / 10) as f32 - 5.0 + (i % 4) as f32);
            cubes.push(rig.ctx.world.spawn((Transform3D::at(at), Model::new(mesh))));
        }
    });

    let Some((mut rig, _, _)) = ground(16) else { return };
    for k in 0..8 {
        let at = Transform3D::at(vec3(k as f32 - 4.0, 1.0, 0.0));
        let mut e = ParticleEmitter3D::new(ParticleSettings3D {
            rate: 0.0,
            burst: 250,
            max_particles: 250,
            lifetime: (1e4, 1e4),
            ..Default::default()
        });
        e.update(1.0 / 60.0, &at);
        rig.ctx.world.spawn((at, e));
        let line = Line3D::new(
            vec![vec3(-4.0, 0.5, k as f32), vec3(4.0, 0.5, k as f32)],
            0.05,
            Color::RED,
        );
        rig.ctx.world.spawn((Transform3D::default(), line));
    }
    measure("still: 2000 particles, 8 lines", &mut rig, FRAMES, |_, _| {});

    let Some((mut rig, cubes, emitters)) = scattered() else {
        return;
    };
    measure(
        "scattered: 1/17 of 1024 models, 1/2 of 200 emitters",
        &mut rig,
        FRAMES,
        |rig, i| {
            scatter(rig, &cubes, &emitters, i);
        },
    );
}

/// 1,024 cubes and 200 emitters of 10 particles each (each emitter's own
/// particles, which move with it), for [`scatter`].
pub(super) fn scattered() -> Option<(Rig, Vec<Entity>, Vec<Entity>)> {
    let (mut rig, cubes, _) = ground(32)?;
    let mut emitters = Vec::new();
    for k in 0..200 {
        let at = Transform3D::at(vec3((k % 20) as f32 - 10.0, 1.0, (k / 20) as f32 - 5.0));
        let mut e = ParticleEmitter3D::new(ParticleSettings3D {
            rate: 0.0,
            burst: 10,
            max_particles: 10,
            lifetime: (1e4, 1e4),
            space: ParticleSpace3D::Local,
            ..Default::default()
        });
        e.update(1.0 / 60.0, &at);
        emitters.push(rig.ctx.world.spawn((at, e)));
    }
    Some((rig, cubes, emitters))
}

/// Changes things scattered all over the buffers: every 17th cube moves
/// (more unchanged instances between two than are ever written along), and
/// every other emitter.
pub(super) fn scatter(rig: &mut Rig, cubes: &[Entity], emitters: &[Entity], i: usize) {
    let y = (i % 20) as f32 * 0.02;
    for &e in cubes.iter().step_by(17) {
        rig.ctx.world.get_mut::<Transform3D>(e).unwrap().position.y = y;
    }
    for &e in emitters.iter().step_by(2) {
        rig.ctx.world.get_mut::<Transform3D>(e).unwrap().position.y = 1.0 + y;
    }
}
