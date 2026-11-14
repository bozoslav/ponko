//! [`TextureArray`]: several images of the same size as one texture, for
//! 3D materials that pick between many (terrain layers, foliage variants).

use crate::assets;
use crate::gfx::Graphics;

/// What a [`TextureArray`]'s pixels hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
#[non_exhaustive]
pub enum TextureKind {
    /// Colors, stored as sRGB (as images are painted); shaders read them as
    /// linear light.
    #[default]
    Color = 0,
    /// Numbers, read as stored: normal maps, masks, blend weights.
    Data = 1,
}

/// Several images of the same size in one texture, for materials that use
/// many: terrain layers, foliage variants. Made with
/// [`Graphics::create_texture_array`](crate::gfx::Graphics::create_texture_array) or [`Graphics::load_texture_array`](crate::gfx::Graphics::load_texture_array); a cheap
/// handle. Custom shaders read one through
/// [`MaterialDesc::custom_arrays`](crate::render::MaterialDesc::custom_arrays), and
/// it gets mipmaps.
///
/// ```no_run
/// # use ponko::prelude::*;
/// # fn f(ctx: &mut Ctx) {
/// let ground = ctx.gfx.load_texture_array(&["grass.png", "dirt.png", "rock.png"], ponko::terrain::TextureKind::Color);
/// assert_eq!(ground.layers(), 3);
/// # }
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(C)] // shared with C/C++ (`PonkoTextureArray`)
pub struct TextureArray {
    pub(crate) id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) layers: u32,
}

impl TextureArray {
    /// Width of each image, in pixels.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(a: ponko::terrain::TextureArray) { let _ = a.width(); }
    /// ```
    #[must_use]
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Height of each image, in pixels.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(a: ponko::terrain::TextureArray) { let _ = a.height(); }
    /// ```
    #[must_use]
    pub fn height(&self) -> u32 {
        self.height
    }

    /// How many images it holds.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(a: ponko::terrain::TextureArray) { let _ = a.layers(); }
    /// ```
    #[must_use]
    pub fn layers(&self) -> u32 {
        self.layers
    }
}

impl Graphics {
    /// Makes a [`TextureArray`] from images of `width` × `height` RGBA
    /// pixels (4 bytes each), one per layer.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let red = [255, 0, 0, 255].repeat(4);
    /// let blue = [0, 0, 255, 255].repeat(4);
    /// let tints = ctx.gfx.create_texture_array(2, 2, &[&red, &blue], ponko::terrain::TextureKind::Color);
    /// assert_eq!(tints.layers(), 2);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If there are no layers, the size is zero or larger than the GPU
    /// takes, or a layer isn't `width * height * 4` bytes.
    pub fn create_texture_array(
        &mut self,
        width: u32,
        height: u32,
        layers: &[&[u8]],
        kind: TextureKind,
    ) -> TextureArray {
        self.try_create_texture_array(width, height, layers, kind)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`create_texture_array`](Self::create_texture_array) with errors as
    /// values: [`ErrorKind::Invalid`](crate::error::ErrorKind::Invalid) for no
    /// layers, a zero size or a layer of the wrong length,
    /// [`Limit`](crate::error::ErrorKind::Limit) for one larger than the GPU takes.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_create_texture_array(2, 2, &[&[0; 3]], ponko::terrain::TextureKind::Color).unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::Invalid);
    /// });
    /// ```
    pub fn try_create_texture_array(
        &mut self,
        width: u32,
        height: u32,
        layers: &[&[u8]],
        kind: TextureKind,
    ) -> Result<TextureArray, crate::error::Error> {
        let invalid = |message: String| crate::error::Error::new(crate::error::ErrorKind::Invalid, message);
        if layers.is_empty() {
            return Err(invalid("a texture array needs at least one layer".into()));
        }
        if width == 0 || height == 0 {
            return Err(invalid(format!(
                "texture size must not be zero (got {width} x {height})"
            )));
        }
        let expected = width as usize * height as usize * 4;
        if let Some((i, layer)) = layers.iter().enumerate().find(|(_, l)| l.len() != expected) {
            return Err(invalid(format!(
                "layer {i} of a {width} x {height} texture array needs {expected} bytes of RGBA, got {}",
                layer.len()
            )));
        }
        let id = self.assets3d.arrays.len() as u32;
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            let (max, max_layers) = renderer.max_texture_array_size();
            if width > max || height > max || layers.len() as u32 > max_layers {
                return Err(crate::error::Error::new(
                    crate::error::ErrorKind::Limit,
                    format!(
                        "a {width} x {height} texture array of {} layers is larger than this GPU takes \
                         ({max} x {max}, {max_layers} layers)",
                        layers.len()
                    ),
                ));
            }
            let gpu_id = renderer.create_texture_array(width, height, layers, kind == TextureKind::Data);
            debug_assert_eq!(gpu_id, id);
        }
        let array = TextureArray {
            id,
            width,
            height,
            layers: layers.len() as u32,
        };
        self.assets3d.arrays.push(array);
        self.assets3d.array_data.push(kind == TextureKind::Data);
        self.assets3d.arrays_live.insert(id);
        self.asset_ledger.made(crate::inventory::AssetClass::TextureArray, id);
        Ok(array)
    }

    /// Loads images (PNG, JPEG, ...) from the [added asset
    /// sources](crate::assets::Assets::add_source) or `assets/` into a
    /// [`TextureArray`], one layer each, in order. They must all be the
    /// same size. Each call makes a new array.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let normals = ctx.gfx.load_texture_array(&["grass_n.png", "rock_n.png"], ponko::terrain::TextureKind::Data);
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// With a clear message if a file is missing or not an image, or the
    /// sizes differ.
    pub fn load_texture_array(&mut self, paths: &[&str], kind: TextureKind) -> TextureArray {
        self.try_load_texture_array(paths, kind)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`load_texture_array`](Self::load_texture_array) with errors as
    /// values: [`ErrorKind::NotFound`](crate::error::ErrorKind::NotFound) if a file
    /// is missing, [`Io`](crate::error::ErrorKind::Io) if one can't be read,
    /// [`Decode`](crate::error::ErrorKind::Decode) if one isn't an image (each with
    /// that file as its path), and [`Invalid`](crate::error::ErrorKind::Invalid) for
    /// no files or images of different sizes.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// Game::new("t", 1, 1).run_headless(0, |ctx| {
    ///     let err = ctx.gfx.try_load_texture_array(&["no/such.png"], ponko::terrain::TextureKind::Color).unwrap_err();
    ///     assert_eq!(err.kind(), ErrorKind::NotFound);
    ///     assert_eq!(err.path(), Some("no/such.png"));
    /// });
    /// ```
    pub fn try_load_texture_array(
        &mut self,
        paths: &[&str],
        kind: TextureKind,
    ) -> Result<TextureArray, crate::error::Error> {
        let mut images = Vec::with_capacity(paths.len());
        #[cfg(feature = "compressed-textures")]
        let mut blocks = Vec::new();
        for path in paths {
            let found = self.asset_sources.find(path)?;
            let origin = found.origin.clone();
            let bytes = found.into_bytes().map_err(|e| e.with_path(*path))?;
            let what = origin.describe(path);
            #[cfg(feature = "compressed-textures")]
            if let Some(decoded) = crate::compressed::decode(&bytes, path, &what, self.compressed_formats()) {
                match decoded? {
                    crate::compressed::Decoded::Rgba(image) => images.push(image),
                    crate::compressed::Decoded::Blocks(image) => blocks.push((images.len(), image)),
                }
                continue;
            }
            images.push(assets::decode_image(&bytes, path, &what)?);
        }
        #[cfg(feature = "compressed-textures")]
        if !blocks.is_empty() {
            if images.is_empty()
                && let Some(array) = self.compressed_array(paths, &mut blocks, kind)?
            {
                return Ok(array);
            }
            // Mixed with image files (or of different formats): every
            // layer as RGBA8.
            for (at, image) in blocks {
                let level = crate::compressed::decode_blocks(image.format, image.width, image.height, &image.levels[0])
                    .map_err(|e| crate::error::Error::decode(paths[at], e))?;
                images.insert(at, level);
            }
        }
        let Some(first) = images.first() else {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::Invalid,
                "load_texture_array: no files given",
            ));
        };
        let (width, height) = (first.width, first.height);
        if let Some((i, other)) = images
            .iter()
            .enumerate()
            .find(|(_, im)| (im.width, im.height) != (width, height))
        {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::Invalid,
                format!(
                    "load_texture_array: {} is {} x {}, but {} is {width} x {height}; every layer must be the same size",
                    paths[i], other.width, other.height, paths[0]
                ),
            )
            .with_path(paths[i]));
        }
        let layers: Vec<&[u8]> = images.iter().map(|im| im.rgba.as_slice()).collect();
        self.try_create_texture_array(width, height, &layers, kind)
    }

    /// Whether this game made `array`, as it is, and hasn't freed it (for
    /// the C API, which takes ids and sizes): [`Ctx::is_live`](crate::ctx::Ctx::is_live), with the
    /// size checked too.
    /// A texture array of block-compressed layers, if they share one
    /// size, format and color space; `None` to load them as RGBA8 instead.
    /// The array's `kind` decides sRGB decoding, as for RGBA8 arrays.
    #[cfg(feature = "compressed-textures")]
    fn compressed_array(
        &mut self,
        paths: &[&str],
        layers: &mut [(usize, crate::compressed::CompressedImage)],
        kind: TextureKind,
    ) -> Result<Option<TextureArray>, crate::error::Error> {
        let (_, first) = &layers[0];
        let (width, height, format) = (first.width, first.height, first.format);
        if let Some((at, other)) = layers.iter().find(|(_, l)| (l.width, l.height) != (width, height)) {
            return Err(crate::error::Error::new(
                crate::error::ErrorKind::Invalid,
                format!(
                    "load_texture_array: {} is {} x {}, but {} is {width} x {height}; every layer must be the same size",
                    paths[*at], other.width, other.height, paths[0]
                ),
            )
            .with_path(paths[*at]));
        }
        if layers.iter().any(|(_, l)| l.format != format) {
            return Ok(None);
        }
        let srgb = kind == TextureKind::Color && format.has_srgb();
        // Every layer keeps the mip levels they all have.
        let levels = layers.iter().map(|(_, l)| l.levels.len()).min().unwrap_or(1);
        for (_, layer) in layers.iter_mut() {
            layer.srgb = srgb;
            layer.levels.truncate(levels);
        }
        let id = self.assets3d.arrays.len() as u32;
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            let (max, max_layers) = renderer.max_texture_array_size();
            if width > max || height > max || layers.len() as u32 > max_layers {
                return Err(crate::error::Error::new(
                    crate::error::ErrorKind::Limit,
                    format!(
                        "a {width} x {height} texture array of {} layers is larger than this GPU takes \
                         ({max} x {max}, {max_layers} layers)",
                        layers.len()
                    ),
                ));
            }
            let refs: Vec<&crate::compressed::CompressedImage> = layers.iter().map(|(_, l)| l).collect();
            let gpu_id = renderer.create_compressed_texture_array(&refs);
            debug_assert_eq!(gpu_id, id);
        }
        let array = TextureArray {
            id,
            width,
            height,
            layers: layers.len() as u32,
        };
        self.assets3d.arrays.push(array);
        self.assets3d.array_data.push(kind == TextureKind::Data);
        self.assets3d.arrays_live.insert(id);
        self.asset_ledger.made(crate::inventory::AssetClass::TextureArray, id);
        Ok(Some(array))
    }

    #[doc(hidden)]
    #[must_use]
    pub fn has_texture_array(&self, array: TextureArray) -> bool {
        self.assets3d.arrays_live.contains(array.id) && self.assets3d.arrays.get(array.id as usize) == Some(&array)
    }

    /// Frees texture array `id` (see [`Ctx::free`](crate::ctx::Ctx::free)): materials reading it
    /// see a white one, and decals lose their images if it was theirs.
    pub(crate) fn free_texture_array_id(&mut self, id: u32) {
        if !self.assets3d.arrays_live.remove(id) {
            return;
        }
        self.asset_ledger.freed(crate::inventory::AssetClass::TextureArray, id);
        if self.assets3d.decal_array.is_some_and(|a| a.id == id) {
            self.assets3d.decal_array = None;
        }
        if let Some(renderer) = self.renderer.as_mut()
            && !self.headless
        {
            renderer.free_texture_array(id);
        }
    }

    /// Sets the texture array [`Decal3D`](crate::d3::Decal)s take their images
    /// from (`None`: flat-colored decals). Every decal shares it, so a
    /// whole game's decals are drawn with fixed bindings: put the images in
    /// its layers (or in parts of layers, with `uv_min`/`uv_max`), color
    /// images and normal maps together.
    ///
    /// Make the array with [`TextureKind::Data`]: color images are decoded
    /// from sRGB when a decal draws them, and normal maps are read as
    /// stored, which one array can only do with its pixels left as they are.
    /// Leave a few pixels of padding around atlas entries, as mipmaps blend
    /// them.
    ///
    /// ```no_run
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &mut Ctx) {
    /// let images = ctx.gfx.load_texture_array(&["blood.png", "tyre.png", "tyre_n.png"], ponko::terrain::TextureKind::Data);
    /// ctx.gfx.set_decal_textures(Some(images));
    /// # }
    /// ```
    ///
    /// # Panics
    ///
    /// If the array wasn't made by this context or isn't `TextureKind::Data`.
    pub fn set_decal_textures(&mut self, array: Option<TextureArray>) {
        self.try_set_decal_textures(array).unwrap_or_else(|e| panic!("{e}"))
    }

    /// [`set_decal_textures`](Self::set_decal_textures) with errors as
    /// values: [`ErrorKind::BadHandle`](crate::error::ErrorKind::BadHandle) for an
    /// array this game didn't make (or freed),
    /// [`Invalid`](crate::error::ErrorKind::Invalid) for one not made with
    /// [`TextureKind::Data`].
    pub fn try_set_decal_textures(&mut self, array: Option<TextureArray>) -> Result<(), crate::error::Error> {
        if let Some(a) = array {
            if !self.has_texture_array(a) {
                return Err(crate::error::Error::new(
                    crate::error::ErrorKind::BadHandle,
                    "decal textures: that texture array wasn't made by this context",
                ));
            }
            if !self.assets3d.array_data.get(a.id as usize).copied().unwrap_or(false) {
                return Err(crate::error::Error::new(
                    crate::error::ErrorKind::Invalid,
                    "decal textures: make the array with TextureKind::Data (decals decode colors themselves)",
                ));
            }
        }
        self.assets3d.decal_array = array;
        Ok(())
    }

    /// The texture array decals draw their images from, if one is set.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &Ctx) { assert!(ctx.gfx.decal_textures().is_none()); }
    /// ```
    #[must_use]
    pub fn decal_textures(&self) -> Option<TextureArray> {
        self.assets3d.decal_array
    }

    /// Whether this GPU draws projected decals: it needs one more sampled
    /// texture in the mesh shader than WebGL2 (and GPUs at the default
    /// limit) have. Where it's false, [`Decal3D`](crate::d3::Decal)s are
    /// skipped and everything else draws as usual. False in headless runs.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// # fn f(ctx: &Ctx) { let _ = ctx.gfx.supports_decals(); }
    /// ```
    #[must_use]
    pub fn supports_decals(&self) -> bool {
        self.renderer.as_ref().is_some_and(|r| r.supports_decals())
    }
}
