//! [`Assets`]: asset files, as `ctx.assets`.
//!
//! **What lives here.** Where named assets are read from
//! ([`Assets::add_source`], [`AssetSource`], [`MemorySource`]), your own
//! asset types ([`Assets::load_with`]), data files that fill resources and
//! components and follow their files ([`Assets::load_resource`] and the
//! `load_*_json` calls), saving scenes and prefabs
//! ([`Assets::save_scene`], [`Assets::save_prefab`]), and what hot reload
//! changed this frame ([`Assets::reloaded_files`],
//! [`Assets::reloaded_textures`]). Textures, fonts, meshes and shaders load
//! through `ctx.gfx` and sounds through `ctx.audio`; they read the same
//! sources and are watched by the same hot reload.
//! [`Ctx::load_scene`](crate::ctx::Ctx::load_scene) and
//! [`Ctx::spawn_prefab`](crate::ctx::Ctx::spawn_prefab) stay on `Ctx`: they
//! replace or extend the world, load through every service and reset
//! commands and physics.
//!
//! **When to call it.** Add sources first, in setup, before loading what
//! they hold. Load data files in setup too; with hot reload on they are
//! applied again when their files change, at the start of a frame. Ask for
//! the reloaded files from a frame or simulation system. Save from any
//! system.
//!
//! **What it needs.** No window, no GPU and no audio device. Files come
//! from the added sources, then the `assets/` folders on desktop (the
//! files fetched from `assets/index.json` in a browser); saving writes to
//! `assets/` and isn't available in web builds. The calls that read or
//! write entities and resources take the world (`&mut ctx.world`), so
//! `ctx.assets` borrows apart from it.
//!
//! ```
//! use ponko::prelude::*;
//!
//! #[derive(serde::Deserialize)]
//! struct Rules {
//!     speed: f32,
//! }
//!
//! Game::new("assets", 1, 1).run_headless(0, |ctx| {
//!     ctx.assets
//!         .add_source(MemorySource::new("built-in").with("rules.json", r#"{"speed": 30}"#));
//!     ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json");
//!     assert_eq!(ctx.world.resource::<Rules>().speed, 30.0);
//! });
//! ```
//!
//! # Finding and decoding
//!
//! Loading a named asset has three steps, kept apart here and in the
//! services that load:
//! 1. **Acquire** its bytes by name: from the [`AssetSource`]s a game added,
//!    in order, then from the default source (the `assets/` folders on
//!    desktop, the files fetched from `assets/index.json` in a browser).
//! 2. **Decode** them on the CPU (`decode_image`): no GPU, any thread.
//! 3. **Create** the GPU resource on the game thread (`Graphics::create_texture`).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use ponko_ecs::{Entity, World};
use serde_json::{Value, json};

use crate::hot_reload::Asset;
use crate::reflect::WorldReflect;
use crate::scene::{PREFAB_FORMAT, PREFAB_VERSION};

/// A place named assets can be read from, other than the `assets/` folder:
/// bytes embedded in the game, a pack file, a test fixture in memory.
///
/// Add one with [`Assets::add_source`].
/// Sources are asked in the order they were added, before the default
/// `assets/` search; a source that doesn't have a name says so with an
/// error of kind [`std::io::ErrorKind::NotFound`], and the next one is
/// asked. Any other error stops the search and is reported.
///
/// [`MemorySource`] is a ready-made one.
///
/// # Names, caching and hot reload
///
/// Names are the same logical names the default source uses
/// (`"sprites/crab.png"`, `/`-separated), and scenes and prefabs refer to
/// assets by them. A loaded asset is cached by its source's
/// [`id`](Self::id) and its name, so the same name in two sources is two
/// assets, and loading a name again returns the first handle without
/// reading the source again. This holds for every kind of asset: textures,
/// fonts, data, scenes, prefabs, sounds, streamed music and (with the `3d`
/// feature) meshes, skeletons, animation clips, shaders, screen effects and
/// compute passes. Images a glTF file refers to are shared between models
/// by the source they were found in, so two sources with the same image
/// name give two images. Texture arrays, reflection probe images and
/// [`load_asset_with`](crate::assets::Assets::load_with) read their source on
/// every call (they aren't cached).
///
/// # In a browser
///
/// Added sources work the same in web builds: they are asked first, and
/// what they have is never fetched. The default source there is the files
/// listed in `assets/index.json`, fetched before setup.
///
/// # Hot reload
///
/// Only files found in the default source on desktop are watched and
/// reloaded when they change (textures, fonts, sounds, data, shaders,
/// effects, compute). Assets that came from an added source are loaded once
/// and never reloaded, whatever the source does later, so a source can keep
/// its bytes in memory without being polled. The same name found in `assets/`
/// before a source was added keeps being watched.
///
/// `read` runs on the game thread, inside the load call that needs it.
pub trait AssetSource: 'static {
    /// A short name for this source, unique among the sources added to a
    /// game: part of every cache key and error message.
    fn id(&self) -> &str;

    /// The bytes of asset `name`, or an error of kind
    /// [`NotFound`](std::io::ErrorKind::NotFound) if this source doesn't
    /// have it.
    ///
    /// # Errors
    ///
    /// `NotFound` lets the next source try; any other error is reported to
    /// the caller of the load.
    fn read(&self, name: &str) -> std::io::Result<Vec<u8>>;

    /// Whether this source has asset `name`, without handing over its
    /// bytes. Used to tell which source a file a model refers to comes
    /// from, so it is shared with what was loaded from that source before
    /// instead of read again.
    ///
    /// The default reads it and checks the error is not
    /// [`NotFound`](std::io::ErrorKind::NotFound); override it when that is
    /// cheaper to answer (an index lookup).
    fn contains(&self, name: &str) -> bool {
        !matches!(self.read(name), Err(e) if e.kind() == std::io::ErrorKind::NotFound)
    }
}

/// An [`AssetSource`] holding its files in memory: assets embedded with
/// `include_bytes!`, generated at startup, or made up in a test.
///
/// ```
/// # use ponko::prelude::*;
/// Game::new("t", 1, 1).run_headless(0, |ctx| {
///     let mut pixel = Vec::new();
///     image::RgbaImage::from_pixel(1, 1, image::Rgba([255, 0, 0, 255]))
///         .write_to(&mut std::io::Cursor::new(&mut pixel), image::ImageFormat::Png)
///         .unwrap();
///     ctx.assets.add_source(MemorySource::new("embedded").with("red.png", pixel));
///     let red = ctx.gfx.load_texture("red.png");
///     assert_eq!((red.width(), red.height()), (1, 1));
/// });
/// ```
#[derive(Clone, Default)]
pub struct MemorySource {
    id: String,
    files: HashMap<String, Arc<[u8]>>,
}

impl MemorySource {
    /// An empty source called `id`.
    pub fn new(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            files: HashMap::new(),
        }
    }

    /// Adds (or replaces) file `name`, builder style.
    #[must_use]
    pub fn with(mut self, name: impl Into<String>, bytes: impl Into<Vec<u8>>) -> Self {
        self.insert(name, bytes);
        self
    }

    /// Adds (or replaces) file `name`.
    pub fn insert(&mut self, name: impl Into<String>, bytes: impl Into<Vec<u8>>) {
        self.files.insert(name.into(), bytes.into().into());
    }
}

impl AssetSource for MemorySource {
    fn id(&self) -> &str {
        &self.id
    }

    fn contains(&self, name: &str) -> bool {
        self.files.contains_key(name)
    }

    fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
        self.files.get(name).map(|b| b.to_vec()).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("\"{name}\" is not in memory source \"{}\"", self.id),
            )
        })
    }
}

/// Where a loaded asset came from; with its kind, the cache key.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub(crate) enum Origin {
    /// The default source: the file [`resolve`] found (on the web, the
    /// fetched copy's name). The only kind hot reload watches.
    File(PathBuf),
    /// An added [`AssetSource`], by its id, and the logical name.
    Source { source: String, name: String },
}

impl Origin {
    /// `name` for error messages, with where it came from.
    pub(crate) fn describe(&self, name: &str) -> String {
        match self {
            Origin::File(_) => format!("\"{name}\""),
            Origin::Source { source, .. } => format!("\"{name}\" (from asset source \"{source}\")"),
        }
    }

    /// Where the asset is, for listings: the file's path, or `source:name`.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn display(&self) -> String {
        match self {
            Origin::File(path) => path.display().to_string(),
            Origin::Source { source, name } => format!("{source}:{name}"),
        }
    }
}

/// What [`Sources::lookup`] found for a name.
pub(crate) enum Lookup<T> {
    /// The asset was loaded before: where it came from (3D files need it to
    /// find their skeleton and clips), and what it became.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    Cached(Origin, T),
    /// Not loaded yet.
    Found(Found),
}

/// A named asset that was found: where, and its bytes if reading it was
/// how it was found (added sources).
pub(crate) struct Found {
    pub origin: Origin,
    bytes: Option<Vec<u8>>,
}

impl Found {
    /// The asset's bytes, reading the file now if it came from the default source.
    /// A failed read is [`ErrorKind::Io`](crate::error::ErrorKind::Io).
    pub(crate) fn into_bytes(self) -> Result<Vec<u8>, crate::error::Error> {
        match (self.bytes, &self.origin) {
            (Some(bytes), _) => Ok(bytes),
            (None, Origin::File(path)) => read(path).map_err(|e| {
                crate::error::Error::new(
                    crate::error::ErrorKind::Io,
                    format!("could not read {}: {e}", path.display()),
                )
            }),
            (None, Origin::Source { .. }) => unreachable!("sources hand over their bytes when found"),
        }
    }

    /// The asset as UTF-8 text; `name` is for messages and the error's path.
    /// Bytes that aren't UTF-8 are [`ErrorKind::Decode`](crate::error::ErrorKind::Decode).
    pub(crate) fn into_text(self, name: &str) -> Result<String, crate::error::Error> {
        let origin = self.origin.clone();
        let bytes = self.into_bytes().map_err(|e| e.with_path(name))?;
        String::from_utf8(bytes).map_err(|e| {
            let message = match &origin {
                Origin::File(path) => format!("{} is not UTF-8 text: {e}", path.display()),
                Origin::Source { .. } => format!("{} is not UTF-8 text: {e}", origin.describe(name)),
            };
            crate::error::Error::decode(name, message)
        })
    }
}

/// The asset sources a game added, asked before the default one.
#[derive(Default)]
pub(crate) struct Sources {
    list: Vec<Box<dyn AssetSource>>,
}

impl Sources {
    /// Whether a source with this id was added.
    pub(crate) fn has(&self, id: &str) -> bool {
        self.list.iter().any(|s| s.id() == id)
    }

    /// Whether no source was added (only the default one is searched).
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Adds `source` after the others.
    ///
    /// # Panics
    /// If its id is empty or another source already has it.
    pub(crate) fn add(&mut self, source: Box<dyn AssetSource>) {
        let id = source.id();
        assert!(!id.is_empty(), "ponko: an asset source needs a non-empty id");
        assert!(
            self.list.iter().all(|s| s.id() != id),
            "ponko: an asset source called \"{id}\" was already added"
        );
        self.list.push(source);
    }

    /// The cache keys `name` would have in each added source, in order.
    pub(crate) fn origins<'a>(&'a self, name: &'a str) -> impl Iterator<Item = Origin> + 'a {
        self.list.iter().map(move |s| Origin::Source {
            source: s.id().to_string(),
            name: name.to_string(),
        })
    }

    /// Finds asset `name` unless `cached` already has it from where it
    /// would be found. Sources are only read for what isn't cached, so
    /// loading a name again never reads a source again.
    ///
    /// # Errors
    /// As [`find`](Self::find).
    pub(crate) fn lookup<T>(
        &self,
        name: &str,
        cached: impl Fn(&Origin) -> Option<T>,
    ) -> Result<Lookup<T>, crate::error::Error> {
        for origin in self.origins(name) {
            if let Some(value) = cached(&origin) {
                return Ok(Lookup::Cached(origin, value));
            }
        }
        let found = self.find(name)?;
        Ok(match cached(&found.origin) {
            Some(value) => Lookup::Cached(found.origin, value),
            None => Lookup::Found(found),
        })
    }

    /// The bytes of `name` from the first place that has it, for files an
    /// asset refers to (a glTF's images, say).
    ///
    /// # Errors
    /// As [`find`](Self::find), or if reading fails.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        Ok(self.find(name)?.into_bytes()?)
    }

    /// Where asset `name` would be found, without reading it: the first
    /// added source that [has](AssetSource::contains) it, else the default
    /// source's file. `None` if nothing has it.
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn locate(&self, name: &str) -> Option<Origin> {
        match self.list.iter().find(|s| s.contains(name)) {
            Some(source) => Some(Origin::Source {
                source: source.id().to_string(),
                name: name.to_string(),
            }),
            None => resolve(name).ok().map(Origin::File),
        }
    }

    /// Finds asset `name`: in the added sources in order, then the default one.
    ///
    /// # Errors
    /// If no source has it (naming every place searched), or a source
    /// failed to read it.
    /// Not finding it is [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound);
    /// a source that has it but fails to read it is
    /// [`Io`](crate::error::ErrorKind::Io). The error names `name` as its path.
    pub(crate) fn find(&self, name: &str) -> Result<Found, crate::error::Error> {
        for source in &self.list {
            match source.read(name) {
                Ok(bytes) => {
                    return Ok(Found {
                        origin: Origin::Source {
                            source: source.id().to_string(),
                            name: name.to_string(),
                        },
                        bytes: Some(bytes),
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    let message = format!("asset source \"{}\" could not read \"{name}\": {e}", source.id());
                    return Err(crate::error::Error::new(crate::error::ErrorKind::Io, message).with_path(name));
                }
            }
        }
        match resolve(name) {
            Ok(path) => Ok(Found {
                origin: Origin::File(path),
                bytes: None,
            }),
            Err(e) if self.list.is_empty() => Err(crate::error::Error::not_found(name, e)),
            Err(e) => {
                let ids: Vec<String> = self.list.iter().map(|s| format!("\"{}\"", s.id())).collect();
                let message = format!("{e}\nAsset sources searched first: {}", ids.join(", "));
                Err(crate::error::Error::not_found(name, message))
            }
        }
    }
}

/// The asset [`Sources`] of a game, shared by every service that loads
/// files (`ctx.assets` adds to them; `ctx.gfx` and `ctx.audio` read through
/// them). One game thread uses them, so a `RefCell` is enough.
#[derive(Clone, Default)]
pub(crate) struct SharedSources(std::rc::Rc<std::cell::RefCell<Sources>>);

impl SharedSources {
    /// As [`Sources::add`].
    pub(crate) fn add(&self, source: Box<dyn AssetSource>) {
        self.0.borrow_mut().add(source);
    }

    /// As [`Sources::lookup`].
    pub(crate) fn lookup<T>(
        &self,
        name: &str,
        cached: impl Fn(&Origin) -> Option<T>,
    ) -> Result<Lookup<T>, crate::error::Error> {
        self.0.borrow().lookup(name, cached)
    }

    /// As [`Sources::read`].
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn read(&self, name: &str) -> Result<Vec<u8>, String> {
        self.0.borrow().read(name)
    }

    /// As [`Sources::find`].
    pub(crate) fn find(&self, name: &str) -> Result<Found, crate::error::Error> {
        self.0.borrow().find(name)
    }

    /// As [`Sources::locate`].
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn locate(&self, name: &str) -> Option<Origin> {
        self.0.borrow().locate(name)
    }

    /// As [`Sources::has`].
    pub(crate) fn has(&self, id: &str) -> bool {
        self.0.borrow().has(id)
    }

    /// As [`Sources::is_empty`].
    #[cfg_attr(not(feature = "3d"), allow(dead_code))]
    pub(crate) fn is_empty(&self) -> bool {
        self.0.borrow().is_empty()
    }
}

/// Folders we look in for `assets/`, in order:
/// 1. next to `Cargo.toml` when started with `cargo run` (`CARGO_MANIFEST_DIR`)
/// 2. next to the executable (shipped games)
/// 3. the bundle's `Contents/Resources` when the executable is in a macOS
///    app bundle (`Name.app/Contents/MacOS/`), where `ponko package` puts them
/// 4. the current working directory
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn asset_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = std::env::var_os("CARGO_MANIFEST_DIR") {
        roots.push(PathBuf::from(dir).join("assets"));
    }
    if let Some(dir) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        roots.push(dir.join("assets"));
        if let Some(resources) = bundle_resources(&dir) {
            roots.push(resources.join("assets"));
        }
    }
    if let Ok(dir) = std::env::current_dir() {
        roots.push(dir.join("assets"));
    }
    roots.dedup();
    roots
}

/// The `Contents/Resources` folder of the macOS app bundle whose
/// `Contents/MacOS` folder is `exe_dir`, if it is one.
#[cfg(not(target_arch = "wasm32"))]
fn bundle_resources(exe_dir: &Path) -> Option<PathBuf> {
    let contents = exe_dir.parent()?;
    let bundle = contents.parent()?;
    let is_bundle =
        exe_dir.file_name()? == "MacOS" && contents.file_name()? == "Contents" && bundle.extension()? == "app";
    is_bundle.then(|| contents.join("Resources"))
}

/// Resolves `name` (relative to `assets/`) to an existing file.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn resolve(name: &str) -> Result<PathBuf, String> {
    let roots = asset_roots();
    for root in &roots {
        let candidate = root.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    let searched: Vec<String> = roots.iter().map(|r| format!("  {}", r.display())).collect();
    Err(format!(
        "\"{name}\" not found in assets/. Looked in:\n{}",
        searched.join("\n")
    ))
}

/// Reads an asset file found by [`resolve`].
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// Writes `name` (relative to `assets/`): over the file where [`resolve`]
/// finds it, else into the first `assets/` folder that exists (or the first
/// one, created). Returns where it went.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn write(name: &str, bytes: &[u8]) -> Result<PathBuf, String> {
    let path = match resolve(name) {
        Ok(existing) => existing,
        Err(_) => {
            let roots = asset_roots();
            let root = roots
                .iter()
                .find(|r| r.is_dir())
                .or(roots.first())
                .ok_or("no assets/ folder")?;
            root.join(name)
        }
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    // A file a screenshot or trace has claimed (it would remove it, and
    // what is written here, if it's given up).
    if crate::control::is_claimed_path(&path) {
        return Err(format!(
            "could not write {}: a screenshot or trace will be written there; name another file",
            path.display()
        ));
    }
    std::fs::write(&path, bytes).map_err(|e| format!("could not write {}: {e}", path.display()))?;
    Ok(path)
}

/// Files in folder `dir` of `assets/` ending in `.{extension}`, as asset
/// names (`"prefabs/palm.json"`), sorted, from every assets folder found.
#[cfg(all(not(target_arch = "wasm32"), feature = "inspector"))]
pub(crate) fn list(dir: &str, extension: &str) -> Vec<String> {
    let mut names = Vec::new();
    for root in asset_roots() {
        let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().is_some_and(|e| e == extension)
                && let Some(file) = path.file_name().and_then(|f| f.to_str())
            {
                names.push(format!("{dir}/{file}"));
            }
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Web builds have no folders to list.
#[cfg(all(target_arch = "wasm32", feature = "inspector"))]
pub(crate) fn list(_dir: &str, _extension: &str) -> Vec<String> {
    Vec::new()
}

/// Web builds can't write files.
#[cfg(target_arch = "wasm32")]
pub(crate) fn write(name: &str, _: &[u8]) -> Result<PathBuf, String> {
    Err(format!("can't save \"{name}\": web builds have no files"))
}

/// [`write`] for the public save calls: a failure is
/// [`ErrorKind::Io`](crate::error::ErrorKind::Io) naming `name`, or
/// [`Unsupported`](crate::error::ErrorKind::Unsupported) in web builds, which have
/// no files.
pub(crate) fn write_asset(name: &str, bytes: &[u8]) -> Result<(), crate::error::Error> {
    let kind = if cfg!(target_arch = "wasm32") {
        crate::error::ErrorKind::Unsupported
    } else {
        crate::error::ErrorKind::Io
    };
    write(name, bytes)
        .map(|_| ())
        .map_err(|e| crate::error::Error::new(kind, e).with_path(name))
}

/// Reads an asset file found by [`resolve`] as UTF-8 text.
pub(crate) fn read_to_string(path: &Path) -> Result<String, String> {
    let bytes = read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    String::from_utf8(bytes).map_err(|e| format!("{} is not UTF-8 text: {e}", path.display()))
}

// In a browser there are no files: the web build fetches everything listed
// in `assets/index.json` before setup runs (see `crate::web`), and asset
// loading reads those copies.
#[cfg(target_arch = "wasm32")]
thread_local! {
    static WEB_FILES: std::cell::RefCell<std::collections::HashMap<String, std::rc::Rc<[u8]>>> =
        Default::default();
}

/// Stores a fetched asset for later loads (web builds).
#[cfg(target_arch = "wasm32")]
pub(crate) fn insert_web_file(name: String, bytes: Vec<u8>) {
    WEB_FILES.with(|files| files.borrow_mut().insert(name, bytes.into()));
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn resolve(name: &str) -> Result<PathBuf, String> {
    if WEB_FILES.with(|files| files.borrow().contains_key(name)) {
        Ok(PathBuf::from(name))
    } else {
        Err(format!(
            "\"{name}\" not found in assets/. Web builds load the files listed in assets/index.json; \
             is it listed there?"
        ))
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn read(path: &Path) -> std::io::Result<Vec<u8>> {
    let name = path.to_string_lossy();
    WEB_FILES
        .with(|files| files.borrow().get(name.as_ref()).map(|b| b.to_vec()))
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, format!("{name} was not fetched")))
}

/// Decoded RGBA8 image.
pub(crate) struct Image {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// Reads and decodes the image at `path`; `name` is for messages.
pub(crate) fn load_image_at(path: &Path, name: &str) -> Result<Image, String> {
    let bytes = read(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    decode_image(&bytes, name, &format!("\"{name}\"")).map_err(|e| e.message().to_string())
}

/// The ponko cargo feature that turns on reading `format`, and its name for
/// people; `None` for a format the engine never reads (PNG is always built in).
fn image_feature(format: image::ImageFormat) -> Option<(&'static str, &'static str)> {
    use image::ImageFormat as F;
    Some(match format {
        F::Jpeg => ("image-jpeg", "JPEG"),
        F::WebP => ("image-webp", "WebP"),
        F::Bmp => ("image-bmp", "BMP"),
        F::Tga => ("image-tga", "TGA"),
        F::Gif => ("image-gif", "GIF"),
        F::Tiff => ("image-tiff", "TIFF"),
        F::Ico => ("image-ico", "ICO"),
        F::Qoi => ("image-qoi", "QOI"),
        F::Pnm => ("image-pnm", "PNM"),
        F::Dds => ("image-dds", "DDS"),
        F::Hdr => ("image-hdr", "Radiance HDR"),
        F::OpenExr => ("image-exr", "OpenEXR"),
        _ => return None,
    })
}

/// The error for a file in `format` when the build can't read it. `by_magic`
/// says how the format was found (the file's first bytes, or its extension).
fn format_unavailable(format: image::ImageFormat, what: &str, path: &str, by_magic: bool) -> crate::error::Error {
    let found = if by_magic { "is" } else { "has the extension of" };
    let message = match image_feature(format) {
        Some((feature, name)) => format!(
            "{what} {found} a {name} image, but this build of ponko doesn't read {name}: \
             turn on its `{feature}` cargo feature (or `image-formats` for all of them)"
        ),
        None => format!(
            "{what} {found} a {format:?} image, which ponko can't read: \
             use PNG or JPEG, or any other format listed under the `image-formats` cargo feature"
        ),
    };
    crate::error::Error::new(crate::error::ErrorKind::Unsupported, message).with_path(path)
}

/// Decodes an image file's bytes into RGBA8. The format comes from the first
/// bytes, so a mislabelled extension doesn't matter; the extension of `path`
/// is only used for formats with no recognizable start (TGA). `what` names the
/// file for messages. A format the build leaves out fails with
/// [`ErrorKind::Unsupported`](crate::error::ErrorKind::Unsupported), naming the
/// cargo feature to enable; bytes that aren't a readable image with
/// [`ErrorKind::Decode`](crate::error::ErrorKind::Decode).
pub(crate) fn decode_image(bytes: &[u8], path: &str, what: &str) -> Result<Image, crate::error::Error> {
    // KTX2 and DDS of GPU blocks, decoded on the CPU for loaders that need
    // pixels (textures themselves upload as blocks, see `Graphics::texture_from_file`).
    #[cfg(feature = "compressed-textures")]
    if let Some(image) = crate::compressed::decode_rgba(bytes, path, what) {
        return image;
    }
    #[cfg(not(feature = "compressed-textures"))]
    if bytes.starts_with(&[0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB]) {
        return Err(crate::error::Error::new(
            crate::error::ErrorKind::Unsupported,
            format!(
                "{what} is a KTX2 texture, but this build of ponko doesn't read KTX2: \
                 turn on its `compressed-textures` cargo feature"
            ),
        )
        .with_path(path));
    }
    let by_magic = image::guess_format(bytes).ok();
    let format = by_magic.or_else(|| image::ImageFormat::from_path(path).ok());
    if let Some(format) = format.filter(|f| !f.reading_enabled()) {
        return Err(format_unavailable(format, what, path, by_magic.is_some()));
    }
    let decoded = match format {
        Some(format) => image::load_from_memory_with_format(bytes, format),
        None => image::load_from_memory(bytes),
    };
    let rgba = decoded
        .map_err(|e| crate::error::Error::decode(path, format!("{what} is not a valid image: {e}")))?
        .into_rgba8();
    Ok(Image {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// How to apply a loaded data file's text to the world (kept for reloads).
pub(crate) type DataLoader = Box<dyn Fn(&mut World, &str) -> Result<(), crate::error::Error>>;

/// Asset files: `ctx.assets`.
///
/// - **What lives here:** where named assets are read from
///   ([`add_source`](Self::add_source)), your own asset types
///   ([`load_with`](Self::load_with)), data files that fill resources and
///   components and follow their files, saving scenes and prefabs, and
///   what hot reload changed this frame. Textures, fonts, meshes and
///   shaders load through `ctx.gfx` and sounds through `ctx.audio`, from
///   the same sources. [`Ctx::load_scene`](crate::ctx::Ctx::load_scene) and
///   [`Ctx::spawn_prefab`](crate::ctx::Ctx::spawn_prefab) stay on `Ctx`: they
///   load through every service.
/// - **When to call it:** add sources first, in setup; load data files in
///   setup too (with hot reload on they are applied again when their files
///   change, at the start of a frame). Read what was reloaded from any
///   system, and save from any system.
/// - **What it needs:** no window, GPU or audio device. Saving writes to
///   `assets/` and isn't available in web builds. The calls that read or
///   write entities and resources take the world (`&mut ctx.world`), so
///   `ctx.assets` borrows apart from it.
#[derive(Default)]
pub struct Assets {
    /// Asset sources added with `add_source`, asked before `assets/`.
    /// Shared with the services that load files.
    pub(crate) asset_sources: SharedSources,
    /// Asset hot reload: which files loaded assets came from. Shared too.
    pub(crate) watcher: crate::hot_reload::SharedWatcher,
    /// Textures reloaded at the start of this frame.
    pub(crate) reloaded_textures: Vec<crate::texture::Texture>,
    /// Names of every file reloaded at the start of this frame.
    pub(crate) reloaded_files: Vec<String>,
    /// Scratch for the watcher's changes, kept so checks don't allocate.
    pub(crate) reload_pending: Vec<(Asset, PathBuf, String)>,
    /// How to apply each loaded data file's text (`load_resource`).
    pub(crate) data_loaders: Vec<DataLoader>,
    /// Parsed prefab files, by name (`Ctx::spawn_prefab`).
    pub(crate) prefabs: HashMap<String, std::rc::Rc<crate::scene::PrefabFile>>,
    /// Fields of reflected components that name assets, and how to load
    /// them before a scene or prefab is spawned (`scene_asset_field`).
    pub(crate) scene_asset_fields: Vec<crate::scene::SceneAssetField>,
}

impl Assets {
    /// Whether files loaded from `assets/` are watched and reloaded when
    /// they change (see [`Game::hot_reload`](crate::game::Game::hot_reload)).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).hot_reload(false).run_headless(0, |ctx| {
    ///     assert!(!ctx.assets.hot_reload());
    /// });
    /// ```
    #[must_use]
    pub fn hot_reload(&self) -> bool {
        self.watcher.on()
    }

    /// Adds a place to load named assets from. Added sources are asked in
    /// the order they were added, all before the `assets/` folder.
    ///
    /// Every load by name goes through it: [`load_texture`](crate::gfx::Graphics::load_texture),
    /// [`load_font`](crate::gfx::Graphics::load_font), [`load_resource`](Self::load_resource),
    /// [`load_scene`](crate::ctx::Ctx::load_scene), [`spawn_prefab`](crate::ctx::Ctx::spawn_prefab)
    /// (and the textures their scenes name), [`load_sound`](crate::audio::Audio::load),
    /// [`load_with`](Self::load_with) and, with the `3d` feature,
    /// meshes, skeletons and animation clips (and the files a glTF refers to),
    /// shaders, screen effects and compute passes.
    ///
    /// Not covered yet: [`load_stream`](crate::audio::Audio::load_stream) (music streamed
    /// from a file), texture arrays and reflection probe images still come
    /// only from `assets/`.
    ///
    /// Assets from sources are never hot reloaded: only files found in
    /// `assets/` are watched. See [`AssetSource`] for
    /// names, caching and hot reload.
    ///
    /// Add sources in setup, before loading what they hold: assets loaded
    /// earlier stay cached where they were found.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// #[derive(serde::Deserialize)]
    /// struct Rules {
    ///     speed: f32,
    /// }
    ///
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     ctx.assets.add_source(MemorySource::new("built-in").with("rules.json", r#"{"speed": 30}"#));
    ///     ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json");
    ///     assert_eq!(ctx.world.resource::<Rules>().speed, 30.0);
    /// });
    /// ```
    ///
    /// # Panics
    /// If the source's id is empty or a source with that id was already added.
    pub fn add_source(&mut self, source: impl crate::assets::AssetSource) {
        self.asset_sources.add(Box::new(source));
    }

    /// Whether a source with id `id` was added with
    /// [`add_source`](Self::add_source), for code that adds a source only
    /// once (`add_source` panics on a duplicate id).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     assert!(!ctx.assets.has_source("built-in"));
    ///     ctx.assets.add_source(MemorySource::new("built-in"));
    ///     assert!(ctx.assets.has_source("built-in"));
    /// });
    /// ```
    pub fn has_source(&self, id: &str) -> bool {
        self.asset_sources.has(id)
    }

    /// Reads asset `name` (from the added sources, then `assets/`) and
    /// decodes it with `decode` on the CPU: your own asset type, with no
    /// registration. Nothing is cached and nothing is watched: it reads and
    /// decodes each call, so keep the result.
    ///
    /// `decode` gets the bytes and returns your value, or a [`crate::error::Error`]
    /// (usually [`ErrorKind::Decode`](crate::error::ErrorKind::Decode)), which is
    /// reported with the asset's name and source in front of its message.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// /// A level in a tiny made-up format: one row of tiles per line.
    /// struct Tiles(Vec<Vec<u8>>);
    ///
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     ctx.assets.add_source(MemorySource::new("built-in").with("levels/one.tiles", "##.\n#..\n"));
    ///     let level = ctx
    ///         .assets
    ///         .load_with("levels/one.tiles", |bytes| {
    ///             let text = std::str::from_utf8(bytes).map_err(|e| Error::new(ErrorKind::Decode, e.to_string()))?;
    ///             Ok(Tiles(text.lines().map(|row| row.bytes().collect()).collect()))
    ///         })
    ///         .unwrap();
    ///     assert_eq!(level.0.len(), 2);
    ///     assert_eq!(level.0[0], b"##.");
    /// });
    /// ```
    ///
    /// # Errors
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if no source has
    /// the asset (the message says where it looked), [`Io`](crate::error::ErrorKind::Io)
    /// if reading it fails, and `decode`'s error, with its kind, if it fails.
    /// The error's path is `name`.
    pub fn load_with<T>(
        &self,
        name: &str,
        decode: impl FnOnce(&[u8]) -> Result<T, crate::error::Error>,
    ) -> Result<T, crate::error::Error> {
        let found = self.asset_sources.find(name)?;
        let origin = found.origin.clone();
        let bytes = found.into_bytes().map_err(|e| e.with_path(name))?;
        decode(&bytes).map_err(|e| {
            e.context(format!("{} could not be decoded", origin.describe(name)))
                .with_path(name)
        })
    }

    /// Reads asset `name` as UTF-8 text from the first source that has it.
    pub(crate) fn read_text(&self, name: &str) -> Result<(Origin, String), crate::error::Error> {
        let found = self.asset_sources.find(name)?;
        let origin = found.origin.clone();
        Ok((origin, found.into_text(name)?))
    }

    /// Loads resource `R` from a JSON file in `assets/`. With [hot
    /// reload](crate::game::Game::hot_reload) on, editing the file replaces the
    /// resource while the game runs: tune numbers without restarting. A file
    /// that doesn't parse keeps the old values and prints why.
    ///
    /// # Panics
    /// If the file is missing or isn't valid JSON for `R` (with the line and
    /// column).
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// #[derive(serde::Deserialize)]
    /// struct Rules {
    ///     speed: f32,
    ///     damage: f32,
    /// }
    ///
    /// Game::new("Tuning", 800, 600).run(|ctx| {
    ///     ctx.assets.load_resource::<Rules>(&mut ctx.world, "rules.json"); // {"speed": 30, "damage": 2}
    ///     |ctx: &mut Ctx| {
    ///         let speed = ctx.world.resource::<Rules>().speed;
    ///         // ...
    ///     }
    /// });
    /// ```
    pub fn load_resource<R: Send + Sync + 'static + serde::de::DeserializeOwned>(
        &mut self,
        world: &mut World,
        path: &str,
    ) {
        let loaded = self.load_data(
            world,
            path,
            Box::new(|world: &mut World, text: &str| {
                let value: R = serde_json::from_str(text)?;
                world.insert_resource(value);
                Ok(())
            }),
        );
        loaded.unwrap_or_else(|e| panic!("{e}"));
    }

    /// Loads a JSON file into the reflected component `T` of `entity` (object
    /// fields merge), reloading it when the file changes. `T` must be
    /// registered with `World::reflect`; the name the file is applied under
    /// comes from the type, so it can't drift from it.
    /// ```
    /// # use ponko::prelude::*;
    /// # #[derive(Clone, Copy, Default, serde::Serialize, serde::Deserialize)]
    /// # struct Stats { hp: f32 }
    /// # fn f(ctx: &mut Ctx, e: Entity) -> Result<(), ponko::prelude::Error> {
    /// ctx.assets.load_component_json::<Stats>(&mut ctx.world, "stats.json", e)?;
    /// # Ok(()) }
    /// ```
    ///
    /// # Errors
    ///
    /// As [`load_resource_json`](Self::load_resource_json), and
    /// [`ErrorKind::BadHandle`](crate::error::ErrorKind::BadHandle) if `entity` is dead.
    pub fn load_component_json<T: 'static>(
        &mut self,
        world: &mut World,
        path: &str,
        entity: Entity,
    ) -> Result<(), crate::error::Error> {
        self.load_component_json_named(
            world,
            path,
            entity,
            &crate::reflect::short_name(std::any::type_name::<T>()),
        )
    }

    /// Loads a JSON file into the reflected component `component` of `entity`
    /// by name: the C and C++ way, and for components registered by name.
    /// Rust code that has the type uses
    /// [`load_component_json`](Self::load_component_json).
    #[doc(hidden)]
    pub fn load_component_json_named(
        &mut self,
        world: &mut World,
        path: &str,
        entity: Entity,
        component: &str,
    ) -> Result<(), crate::error::Error> {
        let component = component.to_string();
        self.load_data(
            world,
            path,
            Box::new(move |world: &mut World, text: &str| {
                let value: serde_json::Value = serde_json::from_str(text)?;
                Ok(world.set_json(entity, &component, value)?)
            }),
        )
    }

    /// Loads a JSON file into the reflected resource `T` (object fields
    /// merge; it's added if missing), reloading it when the file changes.
    /// `T` must be registered with `World::reflect_resource`; the name comes
    /// from the type. Unlike [`load_resource`](Self::load_resource) this
    /// merges into the existing value instead of replacing it.
    /// ```
    /// # use ponko::prelude::*;
    /// # #[derive(Clone, Default, serde::Serialize, serde::Deserialize)]
    /// # struct Rules { speed: f32 }
    /// # fn f(ctx: &mut Ctx) -> Result<(), ponko::prelude::Error> {
    /// ctx.assets.load_resource_json::<Rules>(&mut ctx.world, "rules.json")?;
    /// # Ok(()) }
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if the file is
    /// missing or `T` isn't registered, [`Io`](crate::error::ErrorKind::Io) if it
    /// can't be read, [`Decode`](crate::error::ErrorKind::Decode) if it isn't UTF-8
    /// and [`Json`](crate::error::ErrorKind::Json) if it isn't valid JSON for `T`.
    /// The error's path is `path`.
    pub fn load_resource_json<T: 'static>(&mut self, world: &mut World, path: &str) -> Result<(), crate::error::Error> {
        self.load_resource_json_named(world, path, &crate::reflect::short_name(std::any::type_name::<T>()))
    }

    /// Loads a JSON file into the reflected resource `name` by name: the C
    /// and C++ way. Rust code that has the type uses
    /// [`load_resource_json`](Self::load_resource_json).
    #[doc(hidden)]
    pub fn load_resource_json_named(
        &mut self,
        world: &mut World,
        path: &str,
        name: &str,
    ) -> Result<(), crate::error::Error> {
        let name = name.to_string();
        self.load_data(
            world,
            path,
            Box::new(move |world: &mut World, text: &str| {
                let value: serde_json::Value = serde_json::from_str(text)?;
                Ok(world.set_resource_json(&name, value)?)
            }),
        )
    }

    fn load_data(&mut self, world: &mut World, path: &str, apply: DataLoader) -> Result<(), crate::error::Error> {
        let (origin, text) = self.read_text(path)?;
        apply(world, &text).map_err(|e| e.context(path).with_path(path))?;
        self.data_loaders.push(apply);
        if let Origin::File(file) = origin {
            self.watcher.watch(path, file, Asset::Data(self.data_loaders.len() - 1));
        }
        Ok(())
    }

    /// Saves the world (its reflected components and resources) as a scene
    /// in `assets/<path>`, over the file if it exists. Not in web builds.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// ctx.assets.save_scene(&ctx.world, "levels/beach.json").expect("saved");
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::Io`](crate::error::ErrorKind::Io) if the file can't be written
    /// ([`Unsupported`](crate::error::ErrorKind::Unsupported) in web builds).
    pub fn save_scene(&self, world: &World, path: &str) -> Result<(), crate::error::Error> {
        let text = serde_json::to_string_pretty(&world.save())?;
        write_asset(path, text.as_bytes())
    }

    /// Saves entity `e` (its reflected components) as a prefab in
    /// `assets/<path>`, for [`spawn_prefab`](crate::ctx::Ctx::spawn_prefab). Its
    /// position isn't kept: each spawn says where. Not in web builds.
    ///
    /// The prefab holds `e`'s children too: its [`PrefabParts`](crate::prelude::PrefabParts)
    /// (if a prefab spawned it) and every entity attached to one of them,
    /// by [`Attached`](crate::attach::Attached) or its 3D form, a skin or joint attachment, or a 3D
    /// joint between two of them. Their transforms are saved relative to
    /// `e`'s, and their references to each other are kept. Included prefabs
    /// are saved as their entities, not as a link to their file. An entity
    /// with no children is saved as a version-1 file, as before.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, palm: Entity) {
    /// ctx.assets.save_prefab(&ctx.world, palm, "prefabs/palm.json").expect("saved");
    /// # }
    /// ```
    ///
    /// # Errors
    ///
    /// [`ErrorKind::BadHandle`](crate::error::ErrorKind::BadHandle) if `e` is dead,
    /// [`Io`](crate::error::ErrorKind::Io) if the file can't be written
    /// ([`Unsupported`](crate::error::ErrorKind::Unsupported) in web builds).
    pub fn save_prefab(&mut self, world: &World, e: Entity, path: &str) -> Result<(), crate::error::Error> {
        let mut components = world.entity_json(e)?;
        if let Some(t) = components.get_mut("Transform2D") {
            t["position"] = json!([0.0, 0.0]);
        }
        // Spawning gives the root its parts again.
        if let Value::Object(map) = &mut components {
            map.remove("PrefabParts");
        }
        let members = crate::scene::prefab_members(world, e);
        if members.len() == 1 {
            let prefab = json!({ "format": PREFAB_FORMAT, "version": PREFAB_VERSION, "components": components });
            let text = serde_json::to_string_pretty(&prefab)?;
            write_asset(path, text.as_bytes())?;
            if let Value::Object(components) = components {
                let file = crate::scene::PrefabFile::single(components);
                self.prefabs.insert(path.to_string(), std::rc::Rc::new(file));
            }
            return Ok(());
        }
        let mut entities = Vec::with_capacity(members.len());
        for &m in &members {
            let mut json = if m == e {
                components.clone()
            } else {
                world.entity_json(m)?
            };
            if m != e {
                crate::scene::relative_to(world, m, e, &mut json);
            }
            // Spawning gives the root its parts again.
            if let Value::Object(map) = &mut json {
                map.remove("PrefabParts");
            }
            entities.push(json!({ "entity": m.to_bits(), "components": json }));
        }
        let prefab =
            json!({ "format": PREFAB_FORMAT, "version": crate::scene::PREFAB_TREE_VERSION, "entities": entities });
        let text = serde_json::to_string_pretty(&prefab)?;
        write_asset(path, text.as_bytes())?;
        // Read back from the file's JSON next time, like any prefab.
        self.prefabs.remove(path);
        Ok(())
    }

    /// Names of the files (as you loaded them) that changed and were
    /// reloaded at the start of this frame, textures and data alike. Use it
    /// to redo work that depends on them.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// if ctx.assets.reloaded_files().iter().any(|f| f == "map.json") {
    ///     println!("map changed: rebuilding the flow fields");
    /// }
    /// # }
    /// ```
    pub fn reloaded_files(&self) -> &[String] {
        &self.reloaded_files
    }

    /// Textures whose files changed and were reloaded at the start of this
    /// frame (with [hot reload](crate::game::Game::hot_reload) on). They keep their
    /// handles, so sprites show the new image by themselves; use this to redo
    /// anything you computed from an image, such as a level read from its pixels.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx, level: Texture) {
    /// if ctx.assets.reloaded_textures().contains(&level) {
    ///     println!("level image changed: rebuilding the map");
    /// }
    /// # }
    /// ```
    pub fn reloaded_textures(&self) -> &[crate::texture::Texture] {
        &self.reloaded_textures
    }

    pub(crate) fn report_reload(&mut self, name: String, applied: Result<(), String>) {
        match applied {
            Ok(()) => {
                eprintln!("ponko: reloaded {name}");
                self.reloaded_files.push(name);
            }
            Err(e) => eprintln!("ponko: could not reload {name} (keeping the old one): {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(not(feature = "compressed-textures"))]
    #[test]
    fn ktx2_files_without_the_feature_name_it() {
        let header = [
            0xAB, b'K', b'T', b'X', b' ', b'2', b'0', 0xBB, b'\r', b'\n', 0x1A, b'\n',
        ];
        let Err(err) = decode_image(&header, "rock.ktx2", "rock.ktx2") else {
            panic!("decoded");
        };
        assert_eq!(err.kind(), crate::error::ErrorKind::Unsupported);
        assert!(err.to_string().contains("rock.ktx2"), "{err}");
        assert!(err.message().contains("compressed-textures"), "{err}");
    }

    #[test]
    fn missing_asset_error_names_file_and_folders() {
        let err = resolve("definitely-not-here.png").unwrap_err();
        assert!(err.contains("\"definitely-not-here.png\" not found in assets/"));
        assert!(err.contains("assets"));
    }

    #[test]
    fn a_macos_app_bundle_finds_assets_in_its_resources() {
        let exe_dir = Path::new("/Games/Coin Dash.app/Contents/MacOS");
        assert_eq!(
            bundle_resources(exe_dir),
            Some(PathBuf::from("/Games/Coin Dash.app/Contents/Resources"))
        );
        // A plain folder, or one that only looks partly like a bundle, is not one.
        for dir in [
            "/Games/coin-dash",
            "/Games/Coin Dash/Contents/MacOS",
            "/x.app/Other/MacOS",
            "/MacOS",
        ] {
            assert_eq!(bundle_resources(Path::new(dir)), None, "{dir}");
        }
    }

    #[test]
    fn sources_are_asked_in_order_and_say_not_found() {
        let mut sources = Sources::default();
        sources.add(Box::new(MemorySource::new("a").with("x", *b"from a")));
        sources.add(Box::new(
            MemorySource::new("b").with("x", *b"from b").with("y", *b"only b"),
        ));
        let x = sources.find("x").unwrap();
        assert_eq!(
            x.origin,
            Origin::Source {
                source: "a".into(),
                name: "x".into()
            }
        );
        assert_eq!(x.into_bytes().unwrap(), b"from a");
        assert_eq!(sources.find("y").unwrap().into_text("y").unwrap(), "only b");
        let missing = MemorySource::new("a").read("z").unwrap_err();
        assert_eq!(missing.kind(), std::io::ErrorKind::NotFound);
        let err = sources.find("definitely-not-here.png").err().unwrap();
        assert_eq!(err.kind(), crate::error::ErrorKind::NotFound);
        assert!(
            err.message().ends_with("Asset sources searched first: \"a\", \"b\""),
            "{err}"
        );
    }

    /// A source that only implements `read`: `contains` falls back to it.
    struct ReadOnly;
    impl AssetSource for ReadOnly {
        fn id(&self) -> &str {
            "read-only"
        }
        fn read(&self, name: &str) -> std::io::Result<Vec<u8>> {
            match name {
                "here" => Ok(b"x".to_vec()),
                "broken" => Err(std::io::Error::other("disk on fire")),
                _ => Err(std::io::ErrorKind::NotFound.into()),
            }
        }
    }

    #[test]
    fn locate_names_the_first_source_that_has_a_name_without_reading_memory() {
        let mut sources = Sources::default();
        sources.add(Box::new(ReadOnly));
        sources.add(Box::new(
            MemorySource::new("mem").with("here", *b"y").with("only-mem", *b"z"),
        ));
        let in_source = |source: &str, name: &str| Origin::Source {
            source: source.into(),
            name: name.into(),
        };
        assert_eq!(sources.locate("here"), Some(in_source("read-only", "here")));
        assert_eq!(sources.locate("only-mem"), Some(in_source("mem", "only-mem")));
        // A source that has it but fails to read it still has it: the
        // load reports the error.
        assert_eq!(sources.locate("broken"), Some(in_source("read-only", "broken")));
        assert_eq!(sources.locate("definitely-not-here.png"), None);
    }

    #[test]
    fn garbage_bytes_are_rejected() {
        assert!(decode_image(b"not a png", "a.png", "\"a.png\"").is_err());
    }

    #[test]
    fn decodes_a_jpeg_to_rgba() {
        if !image::ImageFormat::Jpeg.reading_enabled() {
            return;
        }
        let jpeg = include_bytes!("../tests/fixtures/images/red-blue.jpg");
        let img = decode_image(jpeg, "bg.jpg", "\"bg.jpg\"").unwrap();
        assert_eq!((img.width, img.height), (16, 8));
        let px = |x: usize| &img.rgba[(3 * 16 + x) * 4..][..4];
        let (left, right) = (px(2), px(13));
        assert!(left[0] > 200 && left[1] < 60 && left[2] < 60, "left is red: {left:?}");
        assert!(
            right[2] > 200 && right[0] < 60 && right[1] < 60,
            "right is blue: {right:?}"
        );
        assert_eq!(left[3], 255);
    }

    #[test]
    fn a_compiled_out_format_is_found_from_its_magic_bytes() {
        // Never decoded: the check comes first. Skip when the build has WebP.
        if image::ImageFormat::WebP.reading_enabled() {
            return;
        }
        let webp = b"RIFF\x0c\0\0\0WEBPVP8 \0\0\0\0";
        let err = decode_image(webp, "x.png", "\"x.png\"").err().unwrap();
        assert_eq!(err.kind(), crate::error::ErrorKind::Unsupported);
        assert!(err.message().contains("`image-webp`"), "{}", err.message());
        assert!(err.message().contains("is a WebP image"), "{}", err.message());
    }

    #[test]
    fn a_format_the_engine_never_reads_says_so() {
        let err = format_unavailable(image::ImageFormat::Avif, "\"a.avif\"", "a.avif", false);
        assert_eq!(err.kind(), crate::error::ErrorKind::Unsupported);
        assert!(err.message().contains("can't read"), "{}", err.message());
    }

    #[test]
    fn decodes_png() {
        // 1x1 red pixel PNG, encoded in the test so no asset file is needed.
        let mut png = Vec::new();
        image::RgbaImage::from_raw(1, 1, vec![255, 0, 0, 255])
            .unwrap()
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let img = decode_image(&png, "red.png", "\"red.png\"").unwrap();
        assert_eq!((img.width, img.height), (1, 1));
        assert_eq!(img.rgba, [255, 0, 0, 255]);
    }
}
