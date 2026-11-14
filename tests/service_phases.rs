//! What a headless run does and does not do with the frame-level services,
//! and that the sound API stays silent and usable without a device.
//!
//! The windowed frame order is pinned in `src/game/phase_order_tests.rs`;
//! the table of who owns what is in ARCHITECTURE.md.

use ponko::actions::InputEvent;
use ponko::prelude::*;
use ponko::ui::UiWidget;
use std::{cell::RefCell, rc::Rc};

type Log = Rc<RefCell<Vec<&'static str>>>;

fn recorder(log: &Log, what: &'static str) -> impl FnMut(&mut Ctx) + 'static {
    let log = log.clone();
    move |_| log.borrow_mut().push(what)
}

#[test]
fn setup_runs_before_the_first_tick_and_its_commands_are_applied_first() {
    let log: Log = Log::default();
    let l = log.clone();
    Game::new("t", 1, 1)
        .sim_system(move |ctx: &mut Ctx| {
            l.borrow_mut()
                .push(if ctx.world.query::<&d2::Transform>().count() == 1 {
                    "tick sees setup entity"
                } else {
                    "tick misses setup entity"
                });
        })
        .setup({
            let log = log.clone();
            move |ctx| {
                assert!(ctx.time.sim_tick() == 0, "no tick has run during setup");
                log.borrow_mut().push("builder setup");
                ctx.commands.spawn((d2::Transform::default(),));
            }
        })
        .setup({
            let log = log.clone();
            move |_| log.borrow_mut().push("second builder setup")
        })
        .run_headless(2);
    assert_eq!(
        *log.borrow(),
        [
            "builder setup",
            "second builder setup",
            "tick sees setup entity",
            "tick sees setup entity"
        ]
    );
}

#[test]
fn headless_runs_neither_update_the_retained_ui_nor_run_frame_late_or_draw_steps() {
    use ponko::ui::unstable_taffy::{Dimension, Size, Style};
    let log: Log = Log::default();
    let focus = Rc::new(RefCell::new(None));
    let f = focus.clone();
    Game::new("t", 1, 1)
        .add(Phase::Frame, recorder(&log, "frame"))
        .add(Phase::Animate, recorder(&log, "animate"))
        .add(Phase::Late, recorder(&log, "late"))
        .add(Phase::Draw, recorder(&log, "draw"))
        .sim_system(recorder(&log, "sim"))
        .sim_system(move |ctx: &mut Ctx| *f.borrow_mut() = ctx.ui.focus())
        .run_headless(2, |ctx| {
            let size = |w, h| Style {
                size: Size {
                    width: Dimension::length(w),
                    height: Dimension::length(h),
                },
                flex_shrink: 0.0,
                ..Default::default()
            };
            let root = ctx
                .ui
                .add(UiWidget::Panel, ponko::ui::UiLayout::custom(size(300.0, 200.0)));
            ctx.ui
                .add_child(root, UiWidget::Button, ponko::ui::UiLayout::custom(size(70.0, 30.0)));
            ctx.ui.push_screen(root, false);
            ctx.input.feed(InputEvent::Key(Key::Tab, true));
        });
    assert_eq!(*log.borrow(), ["sim", "sim"], "only simulation systems run");
    assert_eq!(
        *focus.borrow(),
        None,
        "the retained UI is a frame step: it never updated"
    );
}

#[test]
fn audio_stays_silent_and_usable_in_headless_runs() {
    Game::new("t", 1, 1).run_headless(1, |ctx| {
        let beep = ctx.audio.create(1000, &[0.0; 1000]);
        assert!((beep.seconds() - 1.0).abs() < 1e-3);
        let bus = ctx.audio.bus("music");
        ctx.audio.set_bus_volume(bus, 0.5);
        let playing = ctx.audio.play(beep);
        let again = ctx.audio.play_ex(
            beep,
            SoundParams {
                looping: true,
                ..Default::default()
            },
        );
        ctx.audio.stop(playing);
        ctx.audio.stop(again);
        // Freeing a sound the game made works and the handle goes stale.
        assert!(ctx.is_live(beep));
        ctx.free(beep);
        assert!(!ctx.is_live(beep));
    });
}
