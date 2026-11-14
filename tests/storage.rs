//! `ctx.storage`: the player's files through a game, save slots for the
//! world, and storage that is off without an app id.

use ponko::prelude::*;
use ponko::storage::Storage;
use std::path::PathBuf;

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("ponko-storage-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

#[derive(Component, Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
struct Health(i32);

#[test]
fn files_written_in_one_run_are_read_back_in_the_next() {
    let dir = temp_dir("runs");
    Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        assert!(ctx.storage.is_available() && ctx.storage.is_persistent());
        ctx.storage.write("settings.json", b"{}").unwrap();
        ctx.storage.write_json("saves/a.json", &[1, 2, 3]).unwrap();
    });
    Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        assert_eq!(ctx.storage.list().unwrap(), ["saves/a.json", "settings.json"]);
        assert_eq!(ctx.storage.read_json::<Vec<i32>>("saves/a.json").unwrap(), [1, 2, 3]);
        assert!(ctx.storage.exists("settings.json"));
        ctx.storage.delete("settings.json").unwrap();
        ctx.storage.delete("settings.json").unwrap(); // already gone: fine
        assert!(!ctx.storage.exists("settings.json"));
        assert_eq!(
            ctx.storage.read("settings.json").unwrap_err().kind(),
            ErrorKind::NotFound
        );
    });
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_app_id_names_the_folder_inside_the_data_folder() {
    // Nothing is written: the player's real data folder stays untouched.
    Game::new("t", 1, 1).app_id("com.example.crabs").run_headless(0, |ctx| {
        assert!(ctx.storage.is_available(), "the test machine has a home folder");
        let dir = ctx.storage.dir().expect("a folder on the desktop");
        assert!(dir.ends_with("com.example.crabs"), "{}", dir.display());
        assert!(dir.is_absolute(), "{}", dir.display());
    });
}

#[test]
fn a_game_without_an_app_id_keeps_no_files_and_says_why() {
    Game::new("t", 1, 1).run_headless(0, |ctx| {
        assert!(!ctx.storage.is_available());
        assert_eq!(ctx.storage.dir(), None);
        let err = ctx.storage.write("a.txt", b"x").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Unsupported);
        assert!(err.message().contains("app_id"), "{err}");
        assert_eq!(ctx.storage.list().unwrap_err().kind(), ErrorKind::Unsupported);
        assert_eq!(ctx.load_world("a").unwrap_err().kind(), ErrorKind::Unsupported);
    });
}

#[test]
#[should_panic(expected = "app id \"../saves\" must be 1 to 128 letters")]
fn an_app_id_that_is_not_a_plain_name_is_refused() {
    let _ = Game::new("t", 1, 1).app_id("../saves");
}

#[test]
fn a_world_saved_in_a_slot_loads_back_with_its_entities() {
    let dir = temp_dir("slots");
    Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        ctx.world.reflect::<Health>();
        ctx.world.spawn((Health(7),));
        ctx.storage.save_world(&ctx.world, "slot 1").unwrap();
        ctx.storage.save_world(&ctx.world, "autosave").unwrap();
        assert_eq!(ctx.storage.world_slots().unwrap(), ["autosave", "slot 1"]);
        assert!(dir.join("saves").join("slot 1.json").is_file());
    });
    let mut world = Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        ctx.world.reflect::<Health>();
        ctx.world.spawn((Health(1),)); // replaced by the load
        ctx.load_world("slot 1").unwrap();
        ctx.storage.delete_world("autosave").unwrap();
        assert_eq!(ctx.storage.world_slots().unwrap(), ["slot 1"]);
    });
    let mut healths: Vec<Health> = world.query::<&Health>().iter().copied().collect();
    healths.sort_by_key(|h| h.0);
    assert_eq!(healths, [Health(7)]);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn loading_a_missing_or_damaged_slot_leaves_the_world_as_it_was() {
    let dir = temp_dir("bad-slot");
    let mut files = Storage::in_folder(&dir);
    files.write("saves/broken.json", b"{ not json").unwrap();
    let mut world = Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        ctx.world.reflect::<Health>();
        ctx.world.spawn((Health(3),));
        assert_eq!(ctx.load_world("nothing").unwrap_err().kind(), ErrorKind::NotFound);
        let err = ctx.load_world("broken").unwrap_err();
        assert_eq!(err.kind(), ErrorKind::Json);
        assert_eq!(err.path(), Some("saves/broken.json"));
        assert_eq!(ctx.load_world("a/b").unwrap_err().kind(), ErrorKind::Invalid);
    });
    assert_eq!(
        world.query::<&Health>().iter().copied().collect::<Vec<_>>(),
        [Health(3)]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_write_the_disk_refuses_leaves_the_old_file() {
    let dir = temp_dir("refused");
    let mut files = Storage::in_folder(&dir);
    files.write("a.json", b"old").unwrap();
    // The new file's place is taken by a folder: the rename can't happen.
    std::fs::create_dir_all(dir.join("b.json")).unwrap();
    assert_eq!(files.write("b.json", b"new").unwrap_err().kind(), ErrorKind::Io);
    assert!(!dir.join("b.json.tmp").exists(), "the temporary file is cleaned up");
    // A folder in the way of the storage folder itself.
    let blocked = Storage::in_folder(dir.join("a.json").join("below"));
    assert_eq!(blocked.list().unwrap_err().kind(), ErrorKind::Io);
    assert_eq!(files.read("a.json").unwrap(), b"old");
    let _ = std::fs::remove_dir_all(dir);
}

// ANCHOR: storage
/// The player's choices, kept between runs.
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(default)] // a file from an older version still loads
struct Options {
    volume: f32,
}

impl Default for Options {
    fn default() -> Self {
        Self { volume: 1.0 }
    }
}

fn load_options(ctx: &mut Ctx) -> Options {
    // A first run has no file (and a damaged one isn't JSON): the defaults.
    ctx.storage.read_json("options.json").unwrap_or_default()
}

fn keep_options(ctx: &mut Ctx, options: &Options) {
    // The old file stays whole until the new one is, then becomes the backup.
    if let Err(e) = ctx.storage.write_json("options.json", options) {
        eprintln!("{e}"); // a full or read-only disk: the game goes on
    }
}

fn quick_save(ctx: &mut Ctx) -> Result<(), Error> {
    ctx.storage.save_world(&ctx.world, "quick") // saves/quick.json
}

fn quick_load(ctx: &mut Ctx) -> Result<(), Error> {
    ctx.load_world("quick")
}

fn space_crab() -> Game {
    // The folder in the player's data folder, and the browser database's name.
    Game::new("Space Crab", 1280, 720).app_id("space-crab")
}
// ANCHOR_END: storage

#[test]
fn the_guide_example_keeps_options_and_a_quick_save_between_runs() {
    let dir = temp_dir("guide");
    let first = space_crab().data_dir(&dir); // a test keeps its files apart
    first.run_headless(0, |ctx| {
        assert_eq!(load_options(ctx).volume, 1.0, "first run: the defaults");
        keep_options(ctx, &Options { volume: 0.25 });
        ctx.world.reflect::<Health>();
        ctx.world.spawn((Health(9),));
        quick_save(ctx).unwrap();
    });
    let mut world = space_crab().data_dir(&dir).run_headless(0, |ctx| {
        assert_eq!(load_options(ctx).volume, 0.25);
        ctx.world.reflect::<Health>();
        quick_load(ctx).unwrap();
    });
    assert_eq!(
        world.query::<&Health>().iter().copied().collect::<Vec<_>>(),
        [Health(9)]
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_save_slot_keeps_an_animations_frame() {
    let dir = temp_dir("animation");
    let mut crab = None;
    Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        let sheet = SpriteSheet::new(ctx.gfx.load_texture("crab.png"), 16, 16);
        let mut walk = ponko::sheet::Animation::new(sheet, 0..4, 10.0);
        walk.time = 0.25; // frame 2
        crab = Some(
            ctx.world
                .spawn((ponko::d2::Transform::default(), sheet.sprite(2), walk)),
        );
        ctx.storage.save_world(&ctx.world, "slot1").unwrap();
    });
    let crab = crab.unwrap();
    let world = Game::new("t", 1, 1).data_dir(&dir).run_headless(0, |ctx| {
        ctx.load_world("slot1").unwrap();
    });
    let walk = world
        .get::<ponko::sheet::Animation>(crab)
        .expect("the animation is in the save");
    assert_eq!((walk.time, walk.current_frame(), walk.looping), (0.25, 2, true));
    assert_eq!(walk.sheet.frame_size, vec2(16.0, 16.0));
    let _ = std::fs::remove_dir_all(dir);
}
