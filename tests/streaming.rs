//! Streaming: textures and meshes asked for without waiting load on
//! worker threads, upload within a per-frame budget in the order asked,
//! stream textures up from a small stand-in, and stay under their
//! residency budgets by dropping back to it. Headless: byte counts are the
//! engine's estimates.

use std::cell::RefCell;
use std::rc::Rc;

#[allow(unused_imports)]
use ponko::d2::{Sprite, Transform as Transform2D};
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::d3::Transform;
#[allow(unused_imports)]
use ponko::inventory::AssetClass;
use ponko::prelude::*;
#[allow(unused_imports)]
#[cfg(feature = "3d")]
use ponko::render::{LodGroup, LodLevel, LodRenderer, Mesh, MeshData, Model};
#[allow(unused_imports)]
use ponko::streaming::AssetState;
#[allow(unused_imports)]
use ponko::streaming::ResidencyKind;

/// A `w` × `h` PNG of one color.
fn png(w: u32, h: u32, shade: u8) -> Vec<u8> {
    let mut bytes = Vec::new();
    image::RgbaImage::from_pixel(w, h, image::Rgba([shade, shade, shade, 255]))
        .write_to(&mut std::io::Cursor::new(&mut bytes), image::ImageFormat::Png)
        .unwrap();
    bytes
}

/// A memory source with `n` 256 × 256 images, `t0.png` to `t{n-1}.png`.
fn images(n: usize) -> MemorySource {
    let mut source = MemorySource::new("mem");
    for i in 0..n {
        source.insert(format!("t{i}.png"), png(256, 256, i as u8 * 10));
    }
    source
}

/// Bytes of a `side` × `side` RGBA8 image.
fn bytes(side: u64) -> u64 {
    side * side * 4
}

type Step = Box<dyn FnMut(&mut Ctx)>;

/// A tick that asks for nothing: what was asked for in the tick before a
/// gap can be evicted after it (assets asked for in this or the previous
/// frame or tick are kept).
fn gap() -> Step {
    Box::new(|_| {})
}

/// Runs one step per simulation tick, in order, after `setup`; the
/// engine's upload step runs before each.
fn run(setup: impl FnOnce(&mut Ctx) + 'static, steps: Vec<Step>) {
    let steps = Rc::new(RefCell::new(steps));
    let ticks = steps.borrow().len() as u64;
    let mut next = 0;
    Game::new("streaming", 1, 1)
        .sim_system(move |ctx| {
            (steps.borrow_mut()[next])(ctx);
            next += 1;
        })
        .run_headless(ticks, setup);
}

/// The texture's GPU resolution, from the diagnostics.
fn gpu_size(ctx: &Ctx, name: &str) -> [u32; 2] {
    ctx.gfx
        .streamed_assets()
        .into_iter()
        .find(|a| a.kind == ResidencyKind::Texture && a.name == name)
        .map_or([0, 0], |a| a.size)
}

fn setup_deterministic(ctx: &mut Ctx) {
    ctx.assets.add_source(images(4));
    ctx.gfx.set_deterministic_uploads(true);
}

/// The state and GPU size of `t0.png` after each frame of asking for it.
fn texture_timeline() -> Vec<(AssetState, [u32; 2])> {
    let log = Rc::new(RefCell::new(Vec::new()));
    let mut steps: Vec<Step> = Vec::new();
    for _ in 0..6 {
        let log = log.clone();
        steps.push(Box::new(move |ctx| {
            let t = ctx.gfx.stream_texture("t0.png");
            if let Some(t) = t {
                assert_eq!((t.width(), t.height()), (256, 256), "the handle has the full size");
                assert!(ctx.is_live(t));
            }
            log.borrow_mut().push((
                ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
                gpu_size(ctx, "t0.png"),
            ));
        }));
    }
    run(setup_deterministic, steps);
    Rc::try_unwrap(log).unwrap().into_inner()
}

#[test]
fn textures_stream_up_from_a_stand_in_one_resolution_per_upload() {
    use AssetState::*;
    assert_eq!(
        texture_timeline(),
        [
            (Loading, [0, 0]),
            (Streaming, [32, 32]),
            (Streaming, [64, 64]),
            (Streaming, [128, 128]),
            (Resident, [256, 256]),
            (Resident, [256, 256]),
        ]
    );
}

#[test]
fn deterministic_uploads_change_states_in_the_same_frames_every_run() {
    let first = texture_timeline();
    for _ in 0..3 {
        assert_eq!(texture_timeline(), first);
    }
}

#[test]
fn stream_texture_returns_at_once_and_decodes_elsewhere() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("big").with("big.png", png(2048, 2048, 7)));
        let start = std::time::Instant::now();
        assert_eq!(ctx.gfx.stream_texture("big.png"), None);
        let asked = start.elapsed();
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "big.png"),
            AssetState::Loading
        );
        ctx.finish_uploads();
        let big = ctx.gfx.stream_texture("big.png").unwrap();
        assert_eq!(big.width(), 2048);
        // Coarse: asking only reads the bytes, it never decodes 16 MiB.
        assert!(asked < std::time::Duration::from_secs(1), "{asked:?}");
    });
}

#[test]
fn uploads_stay_within_the_per_frame_budget() {
    let mut steps: Vec<Step> = Vec::new();
    let max = Rc::new(RefCell::new(0u64));
    for _ in 0..20 {
        let max = max.clone();
        steps.push(Box::new(move |ctx| {
            let stats = ctx.gfx.upload_stats();
            let seen = *max.borrow();
            *max.borrow_mut() = seen.max(stats.frame_bytes);
            for i in 0..4 {
                ctx.gfx.stream_texture(&format!("t{i}.png"));
            }
        }));
    }
    steps.push(Box::new(|ctx| {
        for i in 0..4 {
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Texture, &format!("t{i}.png")),
                AssetState::Resident
            );
        }
        let stats = ctx.gfx.upload_stats();
        assert_eq!(stats.pending, 0);
        assert_eq!(stats.oversized, 0);
        assert_eq!(stats.total_bytes, 4 * (bytes(32) + bytes(64) + bytes(128) + bytes(256)));
    }));
    run(
        |ctx| {
            setup_deterministic(ctx);
            ctx.gfx.set_upload_budget(Some(bytes(256)));
        },
        steps,
    );
    assert!(*max.borrow() <= bytes(256), "{}", max.borrow());
    assert!(*max.borrow() > 0);
}

#[test]
fn an_upload_larger_than_the_budget_goes_alone() {
    let steps: Vec<Step> = vec![
        Box::new(|ctx| {
            ctx.gfx.stream_texture("t0.png");
            ctx.gfx.stream_texture("t1.png");
        }),
        Box::new(|ctx| {
            // One stand-in per frame: t0's in this one, t1's in the next.
            assert_eq!(gpu_size(ctx, "t0.png"), [32, 32]);
            assert_eq!(gpu_size(ctx, "t1.png"), [0, 0]);
            let stats = ctx.gfx.upload_stats();
            assert_eq!((stats.frame_uploads, stats.frame_bytes), (1, bytes(32)));
            assert_eq!(stats.oversized, 1);
        }),
        Box::new(|ctx| assert_eq!(gpu_size(ctx, "t1.png"), [32, 32])),
    ];
    run(
        |ctx| {
            setup_deterministic(ctx);
            ctx.gfx.set_upload_budget(Some(1));
        },
        steps,
    );
}

#[test]
fn the_texture_budget_drops_the_least_recently_used_to_its_stand_in() {
    let held = Rc::new(RefCell::new(None));
    let h = held.clone();
    let steps: Vec<Step> = vec![
        Box::new(move |ctx| {
            ctx.gfx
                .set_residency_budget(ResidencyKind::Texture, Some(2 * bytes(256)));
            *h.borrow_mut() = Some(ctx.gfx.resident_texture("t0.png"));
        }),
        Box::new(|ctx| {
            ctx.gfx.resident_texture("t1.png");
        }),
        Box::new(|ctx| {
            ctx.gfx.resident_texture("t2.png"); // over budget until the next frame
            assert_eq!(ctx.gfx.residency(ResidencyKind::Texture).resident, 3);
        }),
        Box::new(move |ctx| {
            // The frame's check evicted t0, the least recently used.
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
                AssetState::Evicted
            );
            assert!(!ctx.gfx.is_resident(ResidencyKind::Texture, "t0.png"));
            assert_eq!(gpu_size(ctx, "t0.png"), [32, 32]);
            let t0 = held.borrow().unwrap();
            assert!(ctx.is_live(t0), "an evicted texture keeps its handle");
            assert_eq!(t0.width(), 256);
            let s = ctx.gfx.residency(ResidencyKind::Texture);
            assert_eq!((s.resident, s.resident_bytes, s.evictions), (2, 2 * bytes(256), 1));
            assert_eq!(s.evicted_bytes, bytes(256));
            // Asking for it again brings it back under the same handle.
            assert_eq!(ctx.gfx.resident_texture("t0.png"), t0);
            assert_eq!(gpu_size(ctx, "t0.png"), [256, 256]);
            assert_eq!(ctx.gfx.residency(ResidencyKind::Texture).reloads, 1);
        }),
    ];
    run(|ctx| ctx.assets.add_source(images(3)), steps);
}

#[test]
fn textures_asked_for_in_the_same_tick_stay_and_the_overflow_is_reported() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets.add_source(images(3));
        ctx.gfx
            .set_residency_budget(ResidencyKind::Texture, Some(2 * bytes(256)));
        for i in 0..3 {
            ctx.gfx.resident_texture(&format!("t{i}.png"));
        }
        let s = ctx.gfx.residency(ResidencyKind::Texture);
        assert_eq!((s.resident, s.evictions), (3, 0));
        assert_eq!(s.over_budget_bytes(), bytes(256));
    });
}

#[test]
fn textures_sprites_show_and_pinned_ones_are_kept() {
    let steps: Vec<Step> = vec![
        Box::new(|ctx| {
            ctx.gfx.set_residency_budget(ResidencyKind::Texture, Some(bytes(256)));
            let shown = ctx.gfx.resident_texture("t0.png");
            ctx.world.spawn((Transform2D::default(), Sprite::new(shown)));
        }),
        Box::new(|ctx| {
            ctx.gfx.pin_resident_texture("t1.png");
        }),
        Box::new(|ctx| {
            ctx.gfx.resident_texture("t2.png");
        }),
        gap(),
        Box::new(|ctx| {
            ctx.gfx.resident_texture("t3.png"); // only t2 can go
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
                AssetState::Resident
            );
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Texture, "t1.png"),
                AssetState::Resident
            );
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Texture, "t2.png"),
                AssetState::Evicted
            );
            let s = ctx.gfx.residency(ResidencyKind::Texture);
            assert_eq!(s.pinned, 1);
            assert_eq!(s.over_budget_bytes(), 2 * bytes(256));
            ctx.gfx.unpin_resident_texture("t1.png");
        }),
        Box::new(|ctx| {
            ctx.gfx.resident_texture("t3.png");
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Texture, "t1.png"),
                AssetState::Evicted
            );
        }),
    ];
    run(|ctx| ctx.assets.add_source(images(4)), steps);
}

#[test]
fn the_texture_budget_holds_streaming_at_the_resolution_that_fits() {
    let mut steps: Vec<Step> = Vec::new();
    for _ in 0..8 {
        steps.push(Box::new(|ctx| {
            ctx.gfx.stream_texture("t0.png");
        }));
    }
    steps.push(Box::new(|ctx| {
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
            AssetState::Streaming
        );
        assert_eq!(gpu_size(ctx, "t0.png"), [128, 128]);
        let s = ctx.gfx.residency(ResidencyKind::Texture);
        assert!(s.resident_bytes <= bytes(200), "{s}");
        assert_eq!(s.over_budget_bytes(), 0);
        assert!(ctx.gfx.upload_stats().held_by_budget > 0);
        assert_eq!(ctx.gfx.upload_stats().pending, 0, "held textures leave the queue");
        // A bigger budget lets it go on.
        ctx.gfx.set_residency_budget(ResidencyKind::Texture, None);
        ctx.gfx.stream_texture("t0.png");
    }));
    steps.push(Box::new(|ctx| {
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
            AssetState::Resident
        );
    }));
    run(
        |ctx| {
            setup_deterministic(ctx);
            ctx.gfx.set_residency_budget(ResidencyKind::Texture, Some(bytes(200)));
        },
        steps,
    );
}

#[test]
fn an_evicted_streamed_texture_streams_back_up_under_the_same_handle() {
    let held = Rc::new(RefCell::new(None));
    let (h1, h2) = (held.clone(), held.clone());
    let mut steps: Vec<Step> = vec![Box::new(move |ctx| {
        *h1.borrow_mut() = Some(ctx.gfx.resident_texture("t0.png"));
    })];
    steps.push(gap());
    steps.push(Box::new(|ctx| {
        ctx.gfx.resident_texture("t1.png"); // evicts t0 at the next frame
    }));
    steps.push(Box::new(move |ctx| {
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
            AssetState::Evicted
        );
        let back = ctx.gfx.stream_texture("t0.png");
        assert_eq!(back, *h2.borrow(), "the stand-in shows while it reloads");
    }));
    for _ in 0..4 {
        steps.push(Box::new(|ctx| {
            ctx.gfx.stream_texture("t0.png");
        }));
    }
    steps.push(Box::new(|ctx| {
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
            AssetState::Resident
        );
        assert_eq!(gpu_size(ctx, "t0.png"), [256, 256]);
    }));
    run(
        |ctx| {
            setup_deterministic(ctx);
            ctx.gfx
                .set_residency_budget(ResidencyKind::Texture, Some(bytes(256) + bytes(64)));
        },
        steps,
    );
}

/// Runs `frame` once a frame for up to `frames` frames, without
/// deterministic uploads (decodes finish whenever the workers do), each
/// frame taking at least `pause`.
fn run_frames(
    frames: u64,
    pause: std::time::Duration,
    setup: impl FnOnce(&mut Ctx) + 'static,
    mut frame: impl FnMut(&mut Ctx, u64) + 'static,
) {
    let mut tick = 0;
    Game::new("streaming", 1, 1)
        .sim_system(move |ctx| {
            frame(ctx, tick);
            tick += 1;
            std::thread::sleep(pause);
        })
        .run_headless(frames, setup);
}

#[test]
fn an_evicted_texture_asked_for_every_frame_streams_back_while_decodes_take_longer() {
    // Default mode: the upload step never waits for a decode. Frames are
    // far shorter than a decode, and the game asks every frame: asking
    // again must not restart the decode, or it would never finish.
    let log = Rc::new(RefCell::new((None, None, 0u64)));
    let out = log.clone();
    run_frames(
        4000,
        std::time::Duration::from_micros(50),
        |ctx| {
            ctx.assets.add_source(images(4));
            ctx.gfx
                .set_residency_budget(ResidencyKind::Texture, Some(bytes(256) + bytes(64)));
        },
        move |ctx, tick| {
            let mut log = out.borrow_mut();
            match tick {
                0 => log.0 = Some(ctx.gfx.resident_texture("t0.png")),
                1 => {}
                2 => {
                    ctx.gfx.resident_texture("t1.png");
                }
                // The frame's check evicted t0 for t1.
                3 => assert_eq!(
                    ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
                    AssetState::Evicted
                ),
                _ if log.1.is_none() => {
                    let t = ctx.gfx.stream_texture("t0.png");
                    assert_eq!(t, log.0, "the same handle throughout");
                    if ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png") == AssetState::Resident {
                        log.1 = Some(tick);
                        log.2 = ctx.gfx.upload_stats().decode_waits;
                    }
                }
                _ => {}
            }
        },
    );
    let (_, back_at, waits) = *log.borrow();
    assert!(back_at.is_some(), "the evicted texture never streamed back");
    assert_eq!(waits, 0, "the game thread never waited for a decode");
}

#[test]
fn a_texture_asked_for_every_frame_stays_while_shown_ones_keep_the_budget_over() {
    // A sprite's texture fills the budget; another, which only (say) a
    // material uses, is asked for every frame. Checking the budget at the
    // start of each frame must not evict it between two asks.
    let log = Rc::new(RefCell::new(Vec::new()));
    let out = log.clone();
    run_frames(
        12,
        std::time::Duration::ZERO,
        |ctx| {
            ctx.assets.add_source(images(4));
            ctx.gfx.set_deterministic_uploads(true);
            ctx.gfx.set_residency_budget(ResidencyKind::Texture, Some(bytes(256)));
            let shown = ctx.gfx.resident_texture("t0.png");
            ctx.world.spawn((Transform2D::default(), Sprite::new(shown)));
        },
        move |ctx, _| {
            ctx.gfx.resident_texture("t1.png");
            ctx.gfx.stream_texture("t2.png");
            out.borrow_mut().push((
                ctx.gfx.asset_state(ResidencyKind::Texture, "t1.png"),
                ctx.gfx.asset_state(ResidencyKind::Texture, "t2.png"),
                ctx.gfx.residency(ResidencyKind::Texture),
            ));
        },
    );
    let log = log.borrow();
    let (_, _, last) = log.last().unwrap();
    assert_eq!(last.evictions, 0, "{last}");
    assert_eq!(last.reloads, 0, "nothing was read again: {last}");
    assert!(log.iter().all(|(t1, _, _)| *t1 == AssetState::Resident));
    // The streamed one arrives, and the full budget holds it at a lower
    // resolution (nothing else can go), but it is never evicted.
    assert_eq!(log.last().unwrap().1, AssetState::Streaming);
    assert!(log.iter().all(|(_, t2, _)| *t2 != AssetState::Evicted));
    assert!(last.over_budget_bytes() >= bytes(256), "{last}");
}

#[test]
fn a_missing_texture_fails_once_with_its_error() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert_eq!(ctx.gfx.stream_texture("nope.png"), None);
        ctx.finish_uploads();
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "nope.png"),
            AssetState::Failed
        );
        let error = ctx.gfx.asset_error(ResidencyKind::Texture, "nope.png").unwrap();
        assert_eq!(error.kind(), ErrorKind::NotFound);
        assert_eq!(ctx.gfx.stream_texture("nope.png"), None);
        assert_eq!(ctx.gfx.upload_stats().pending, 0, "not retried");
        assert_eq!(ctx.gfx.upload_stats().failures, 1);
        assert_eq!(
            ctx.gfx.try_resident_texture("nope.png").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    });
}

#[test]
fn a_broken_texture_fails_with_a_decode_error() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        ctx.assets
            .add_source(MemorySource::new("m").with("bad.png", b"not a png".to_vec()));
        ctx.gfx.stream_texture("bad.png");
        ctx.finish_uploads();
        let error = ctx.gfx.asset_error(ResidencyKind::Texture, "bad.png").unwrap();
        assert_eq!(error.kind(), ErrorKind::Decode);
    });
}

#[test]
fn a_freed_streamed_texture_is_forgotten_and_loads_afresh() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        setup_deterministic(ctx);
        ctx.gfx.stream_texture("t0.png");
        ctx.finish_uploads();
        let old = ctx.gfx.stream_texture("t0.png").unwrap();
        ctx.free(old);
        // Its bytes leave the budget at once, not when it is next asked for.
        assert_eq!(ctx.gfx.residency(ResidencyKind::Texture).resident_bytes, 0);
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
            AssetState::Unknown
        );
        assert!(!ctx.gfx.is_resident(ResidencyKind::Texture, "t0.png"));
        ctx.gfx.stream_texture("t0.png");
        ctx.finish_uploads();
        let new = ctx.gfx.stream_texture("t0.png").unwrap();
        assert!(ctx.is_live(new) && !ctx.is_live(old), "a new handle");
        assert_eq!(ctx.gfx.residency(ResidencyKind::Texture).resident, 1);
    });
}

#[test]
fn updating_a_streamed_texture_gives_it_full_resolution() {
    let mut steps: Vec<Step> = vec![Box::new(|ctx| {
        ctx.gfx.stream_texture("t0.png");
    })];
    steps.push(Box::new(|ctx| {
        let t = ctx.gfx.stream_texture("t0.png").unwrap();
        assert_eq!(gpu_size(ctx, "t0.png"), [32, 32]);
        ctx.gfx.update_texture(t, &vec![255; 256 * 256 * 4]);
        assert_eq!(gpu_size(ctx, "t0.png"), [256, 256]);
        assert_eq!(
            ctx.gfx.asset_state(ResidencyKind::Texture, "t0.png"),
            AssetState::Resident
        );
        assert_eq!(ctx.gfx.residency(ResidencyKind::Texture).resident_bytes, bytes(256));
    }));
    run(setup_deterministic, steps);
}

#[test]
fn diagnostics_list_every_streamed_asset_with_its_state() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        setup_deterministic(ctx);
        ctx.gfx.resident_texture("t1.png");
        ctx.gfx.stream_texture("t0.png");
        let assets = ctx.gfx.streamed_assets();
        let names: Vec<(&str, AssetState)> = assets.iter().map(|a| (a.name.as_str(), a.state)).collect();
        assert_eq!(
            names,
            [("t0.png", AssetState::Loading), ("t1.png", AssetState::Resident)]
        );
        assert_eq!(assets[1].size, [256, 256]);
        assert!(assets[1].id.is_some() && assets[0].id.is_none());
    });
}

#[test]
fn the_inventory_counts_streamed_textures_at_their_gpu_resolution() {
    let steps: Vec<Step> = vec![
        Box::new(|ctx| {
            ctx.gfx.stream_texture("t0.png");
        }),
        Box::new(|ctx| {
            let textures = ctx
                .asset_inventory()
                .classes
                .iter()
                .find(|c| c.class == AssetClass::Texture)
                .copied()
                .unwrap();
            assert_eq!((textures.live, textures.bytes), (1, bytes(32)));
        }),
    ];
    run(setup_deterministic, steps);
}

#[test]
fn hot_reload_gives_a_streamed_texture_the_edited_pixels_at_full_resolution() {
    let dir = std::env::temp_dir().join(format!("ponko-streaming-hot-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("wall.png");
    std::fs::write(&path, png(256, 256, 10)).unwrap();
    let name = path.to_str().unwrap().to_string();
    let reloaded = Rc::new(RefCell::new(Vec::new()));
    let log = reloaded.clone();
    Game::new("hot", 1, 1)
        .hot_reload(true)
        .sim_system(move |ctx: &mut Ctx| {
            ctx.gfx.set_deterministic_uploads(true);
            let t = ctx.gfx.stream_texture(&name);
            match ctx.time.sim_tick() {
                1 => assert_eq!(gpu_size(ctx, &name), [32, 32]),
                2 => {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    std::fs::write(&path, png(256, 256, 200)).unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(400));
                }
                3 => {
                    assert_eq!(ctx.assets.reloaded_textures(), &[t.unwrap()]);
                    assert_eq!(gpu_size(ctx, &name), [256, 256]);
                    assert_eq!(ctx.gfx.asset_state(ResidencyKind::Texture, &name), AssetState::Resident);
                    assert_eq!(ctx.gfx.residency(ResidencyKind::Texture).resident_bytes, bytes(256));
                    log.borrow_mut().push(ctx.time.sim_tick());
                }
                _ => {}
            }
        })
        .run_headless(5, |_| {});
    assert_eq!(*reloaded.borrow(), [3]);
}

#[cfg(feature = "3d")]
mod meshes {
    use super::*;
    use std::path::PathBuf;

    /// `n` copies of the test house, each its own file, as absolute paths.
    fn houses(test: &str, n: usize) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!("ponko-streaming-{test}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let house = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/models/house.glb");
        (0..n)
            .map(|i| {
                let path = dir.join(format!("house{i}.glb"));
                std::fs::copy(&house, &path).unwrap();
                path.to_str().unwrap().to_string()
            })
            .collect()
    }

    fn deterministic(ctx: &mut Ctx) {
        ctx.gfx.set_deterministic_uploads(true);
    }

    #[test]
    fn streamed_meshes_arrive_in_a_later_frame() {
        let p = Rc::new(houses("arrive", 1));
        let (p1, p2) = (p.clone(), p.clone());
        let steps: Vec<Step> = vec![
            Box::new(move |ctx| {
                assert_eq!(ctx.gfx.stream_mesh(&p1[0]), None);
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p1[0]), AssetState::Loading);
            }),
            Box::new(move |ctx| {
                let mesh = ctx
                    .gfx
                    .stream_mesh(&p2[0])
                    .expect("uploaded at the start of this frame");
                assert!(ctx.is_live(mesh));
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p2[0]), AssetState::Resident);
                assert_eq!(ctx.gfx.residency(ResidencyKind::Mesh).loads, 1);
                assert!(ctx.gfx.upload_stats().total_bytes > 0);
            }),
        ];
        run(deterministic, steps);
    }

    #[test]
    fn an_evicted_mesh_keeps_its_handle_and_comes_back_under_it() {
        let p = Rc::new(houses("handle", 2));
        let held = Rc::new(RefCell::new(Mesh::NONE));
        let (p1, p2, p3, p4) = (p.clone(), p.clone(), p.clone(), p.clone());
        let (h1, h2) = (held.clone(), held.clone());
        let steps: Vec<Step> = vec![
            Box::new(move |ctx| {
                let one = ctx.gfx.resident_mesh(&p1[0]);
                let b = ctx.gfx.residency(ResidencyKind::Mesh).resident_bytes;
                ctx.gfx.set_residency_budget(ResidencyKind::Mesh, Some(b));
                *h1.borrow_mut() = one; // kept outside any entity
            }),
            gap(),
            Box::new(move |ctx| {
                ctx.gfx.resident_mesh(&p2[1]); // evicts the held one at the next frame
            }),
            Box::new(move |ctx| {
                let held = *h2.borrow();
                assert!(!ctx.is_live(held), "an evicted mesh reads as not live");
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p4[0]), AssetState::Evicted);
                // Meshes made meanwhile never take its id.
                let material = ctx.gfx.default_material();
                let other = ctx.gfx.create_mesh(&MeshData::cube(1.0), material);
                assert_ne!(other, held);
                // Showing it is harmless: it draws nothing.
                ctx.world.spawn((Transform::default(), Model::new(held)));
            }),
            Box::new(move |ctx| {
                let back = ctx.gfx.resident_mesh(&p3[0]);
                assert_eq!(back, *held.borrow(), "it comes back under the same handle");
                assert!(ctx.is_live(back));
            }),
        ];
        run(|_| {}, steps);
    }

    #[test]
    fn an_evicted_mesh_streams_back_under_its_handle() {
        let p = Rc::new(houses("stream-back", 2));
        let held = Rc::new(RefCell::new(Mesh::NONE));
        let (p1, p2, p3, p4) = (p.clone(), p.clone(), p.clone(), p.clone());
        let (h1, h2) = (held.clone(), held.clone());
        let steps: Vec<Step> = vec![
            Box::new(move |ctx| {
                let one = ctx.gfx.resident_mesh(&p1[0]);
                let b = ctx.gfx.residency(ResidencyKind::Mesh).resident_bytes;
                ctx.gfx.set_residency_budget(ResidencyKind::Mesh, Some(b));
                *h1.borrow_mut() = one;
            }),
            Box::new(move |ctx| {
                ctx.gfx.resident_mesh(&p2[1]);
            }),
            Box::new(move |ctx| {
                assert_eq!(ctx.gfx.stream_mesh(&p3[0]), None);
            }),
            Box::new(move |ctx| {
                assert_eq!(ctx.gfx.stream_mesh(&p4[0]), Some(*h2.borrow()));
                assert_eq!(ctx.gfx.residency(ResidencyKind::Mesh).reloads, 1);
            }),
        ];
        run(deterministic, steps);
    }

    #[test]
    fn a_missing_mesh_fails_with_its_error() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            assert_eq!(ctx.gfx.stream_mesh("models/nope.glb"), None);
            ctx.finish_uploads();
            assert_eq!(
                ctx.gfx.asset_state(ResidencyKind::Mesh, "models/nope.glb"),
                AssetState::Failed
            );
            let error = ctx.gfx.asset_error(ResidencyKind::Mesh, "models/nope.glb").unwrap();
            assert_eq!(error.kind(), ErrorKind::NotFound);
            assert_eq!(ctx.gfx.stream_mesh("models/nope.glb"), None);
            assert_eq!(ctx.gfx.upload_stats().pending, 0);
        });
    }

    #[test]
    fn a_mesh_the_game_loaded_streams_at_once() {
        Game::new("t", 1, 1).run_headless(0, |ctx| {
            let house = ctx.gfx.load_mesh("models/house.glb");
            assert_eq!(ctx.gfx.stream_mesh("models/house.glb"), Some(house));
            assert_eq!(ctx.gfx.residency(ResidencyKind::Mesh).pinned, 1, "the game holds it");
        });
    }

    /// A streamed two-level group at `distance` from the camera.
    fn tower(ctx: &mut Ctx, files: &[String], distance: f32) -> Entity {
        ctx.scene3d.camera.position = vec3(0.0, 0.0, distance);
        ctx.scene3d.camera.look_at(Vec3::ZERO);
        let levels = vec![
            LodLevel::new(0.3, vec![LodRenderer::new(Mesh::NONE)]),
            LodLevel::new(0.0, vec![LodRenderer::new(Mesh::NONE)]),
        ];
        let files: Vec<&str> = files.iter().map(String::as_str).collect();
        ctx.world.spawn((
            Transform::default(),
            LodGroup::new(4.0, levels),
            ponko::render::StreamedLod::new(&files),
        ))
    }

    fn level_meshes(ctx: &Ctx, e: Entity) -> Vec<Mesh> {
        let g = ctx.world.get::<LodGroup>(e).unwrap();
        g.levels.iter().map(|l| l.renderers[0].mesh).collect()
    }

    #[test]
    fn streamed_lod_levels_fill_in_from_the_coarsest() {
        let p = houses("lod", 2);
        let e = Rc::new(RefCell::new(None));
        let (e1, e2, e3) = (e.clone(), e.clone(), e.clone());
        let (p0, p1) = (p.clone(), p.clone());
        let steps: Vec<Step> = vec![
            Box::new(move |ctx| {
                // Near: level 0 wanted, level 1 always.
                *e1.borrow_mut() = Some(tower(ctx, &p0, 2.0));
            }),
            Box::new(move |ctx| {
                let e = e2.borrow().unwrap();
                // Both asked for last frame; uploaded at the start of this one,
                // and shown from the next pump.
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p1[0]), AssetState::Resident);
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p1[1]), AssetState::Resident);
                let _ = level_meshes(ctx, e);
            }),
            Box::new(move |ctx| {
                let e = e3.borrow().unwrap();
                let m = level_meshes(ctx, e);
                assert!(m.iter().all(|&m| ctx.is_live(m)), "{m:?}");
                assert_ne!(m[0], m[1]);
            }),
        ];
        run(deterministic, steps);
    }

    #[test]
    fn a_far_streamed_lod_shows_its_coarse_level_and_lets_the_fine_one_go() {
        let p = houses("lod-far", 2);
        let e = Rc::new(RefCell::new(None));
        let (e1, e2, e3) = (e.clone(), e.clone(), e.clone());
        let (p0, p1, p2) = (p.clone(), p.clone(), p.clone());
        let steps: Vec<Step> = vec![
            Box::new(move |ctx| {
                *e1.borrow_mut() = Some(tower(ctx, &p0, 2.0));
            }),
            Box::new(|_| {}),
            Box::new(move |ctx| {
                // Both levels in; now the camera goes far away.
                let e = e2.borrow().unwrap();
                assert!(level_meshes(ctx, e).iter().all(|&m| ctx.is_live(m)));
                ctx.scene3d.camera.position = vec3(0.0, 0.0, 500.0);
                let b = ctx.gfx.residency(ResidencyKind::Mesh).resident_bytes;
                ctx.gfx.set_residency_budget(ResidencyKind::Mesh, Some(b / 2));
            }),
            Box::new(|_| {}),
            Box::new(move |ctx| {
                let e = e3.borrow().unwrap();
                let m = level_meshes(ctx, e);
                // The fine level shows the coarse mesh, so the fine one was evicted.
                assert_eq!(m[0], m[1]);
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p1[0]), AssetState::Evicted);
                assert_eq!(ctx.gfx.asset_state(ResidencyKind::Mesh, &p2[1]), AssetState::Resident);
            }),
        ];
        run(deterministic, steps);
    }
}

#[test]
fn the_guide_streaming_example_runs() {
    // ANCHOR: streaming
    fn setup(ctx: &mut Ctx) {
        // Memory caps for what loads by name, and bytes sent to the GPU per frame.
        ctx.gfx.set_residency_budget(ResidencyKind::Texture, Some(256 << 20));
        ctx.gfx.set_residency_budget(ResidencyKind::Mesh, Some(512 << 20));
        ctx.gfx.set_upload_budget(Some(8 << 20));
    }

    fn draw_ground(ctx: &mut Ctx) {
        // Ask every frame you need it: never waits. `None` until a small
        // stand-in is up, then the same texture, sharper each frame.
        if let Some(ground) = ctx.gfx.stream_texture("cells/ground.png") {
            ctx.gfx.draw(ground, vec2(0.0, 0.0));
        }
        if ctx.gfx.asset_state(ResidencyKind::Texture, "cells/ground.png") == AssetState::Failed {
            let why = ctx.gfx.asset_error(ResidencyKind::Texture, "cells/ground.png");
            eprintln!("no ground: {why:?}");
        }
    }
    // ANCHOR_END: streaming
    let states = Rc::new(RefCell::new(Vec::new()));
    let log = states.clone();
    Game::new("guide", 1, 1)
        .sim_system(draw_ground)
        .sim_system(move |ctx: &mut Ctx| {
            log.borrow_mut()
                .push(ctx.gfx.asset_state(ResidencyKind::Texture, "cells/ground.png"));
        })
        .run_headless(8, |ctx| {
            ctx.assets
                .add_source(MemorySource::new("cells").with("cells/ground.png", png(256, 256, 90)));
            ctx.gfx.set_deterministic_uploads(true);
            setup(ctx);
        });
    assert_eq!(states.borrow().last(), Some(&AssetState::Resident));
}
