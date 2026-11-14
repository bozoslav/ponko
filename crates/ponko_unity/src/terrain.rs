//! Terrain saved as `TerrainData` (class 156): a square grid of 16-bit
//! heights, plus the ground layers painted on it.

use crate::{Error, ObjRef, UnityObject};

/// The largest height Unity stores in a heightmap: 1 of the terrain's
/// height.
const MAX_HEIGHT: f64 = 32766.0;

/// A `TerrainData`'s heights in world units, ready for
/// `Graphics::create_terrain_tile`, and the ground layers painted on it.
///
/// Unity's +Z is Ponko's -Z, so the rows are flipped: the first row is
/// Unity's far (+Z) edge, and the tile's corner is where
/// [`corner`](Self::corner) puts it.
///
/// ```
/// let text = "%YAML 1.1\n--- !u!156 &1\nTerrainData:\n  m_Name: Hill\n  m_Heightmap:\n    m_Resolution: 2\n    m_Scale: {x: 10, y: 50, z: 10}\n    m_Heights: [0, 0, 32766, 16383]\n";
/// let file = ponko_unity::UnityFile::parse(text).unwrap();
/// let t = ponko_unity::UnityTerrain::from_object(&file.objects[0]).unwrap();
/// assert_eq!((t.resolution, t.size, t.height), (2, 10.0, 50.0));
/// assert_eq!(t.heights, [50.0, 25.0, 0.0, 0.0], "Unity's far row comes first");
/// assert_eq!(t.corner([100.0, 0.0, 20.0]), [100.0, 0.0, -30.0]);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct UnityTerrain {
    /// `m_Name`.
    pub name: String,
    /// Heights on a side.
    pub resolution: usize,
    /// How wide (and long) the tile is, in world units.
    pub size: f32,
    /// The height of a full-scale sample: the tallest the ground can be.
    pub height: f32,
    /// `resolution²` heights in world units, row after row along Ponko's +Z.
    pub heights: Vec<f32>,
    /// The terrain layers painted on it (`.terrainlayer` assets), in order:
    /// layer `i`'s weight is channel `i % 4` of alphamap `i / 4`.
    pub layers: Vec<ObjRef>,
    /// The `Texture2D`s holding the layers' weights (RGBA, in the same
    /// file). Read with [`UnityTexture`](crate::UnityTexture): its rows
    /// from the top already run along Ponko's +Z from [`corner`](Self::corner).
    pub alphamaps: Vec<ObjRef>,
}

impl UnityTerrain {
    /// Reads a `TerrainData`, text or binary.
    ///
    /// # Errors
    ///
    /// If it isn't one, its heights aren't `m_Resolution²`, or it isn't
    /// square (Ponko's tiles are).
    pub fn from_object(object: &UnityObject) -> Result<Self, Error> {
        if object.class_id != 156 {
            return Err(Error::new(format!(
                "object {} is a {}, not TerrainData",
                object.file_id, object.type_name
            )));
        }
        let b = &object.body;
        let name = b["m_Name"].as_str().unwrap_or("").to_string();
        let map = &b["m_Heightmap"];
        let n = map["m_Resolution"].as_i64().unwrap_or(0).max(0) as usize;
        let raw: Vec<f64> = match map["m_Heights"].as_numbers() {
            Some(h) => h.to_vec(),
            // Text assets list them as a YAML sequence.
            None => map["m_Heights"]
                .as_seq()
                .unwrap_or(&[])
                .iter()
                .map(|v| v.as_f32().map_or(f64::NAN, f64::from))
                .collect(),
        };
        if n < 2 || n.checked_mul(n) != Some(raw.len()) {
            return Err(Error::new(format!(
                "{name}: {} heights for a resolution of {n}",
                raw.len()
            )));
        }
        let [sx, sy, sz] = map["m_Scale"]
            .vec3()
            .ok_or_else(|| Error::new(format!("{name}: no m_Scale")))?;
        if sx != sz || sx <= 0.0 {
            return Err(Error::new(format!(
                "{name}: heights {sx} apart along X but {sz} along Z; Ponko's tiles are square"
            )));
        }
        let mut heights = Vec::with_capacity(n * n);
        for row in raw.chunks_exact(n).rev() {
            heights.extend(row.iter().map(|&h| (h / MAX_HEIGHT * f64::from(sy)) as f32));
        }
        let refs = |key: &str| -> Vec<ObjRef> {
            b["m_SplatDatabase"][key]
                .as_seq()
                .unwrap_or(&[])
                .iter()
                .filter_map(crate::Value::obj_ref)
                .collect()
        };
        Ok(Self {
            name,
            resolution: n,
            size: sx * (n - 1) as f32,
            height: sy,
            heights,
            layers: refs("m_TerrainLayers"),
            alphamaps: refs("m_AlphaTextures"),
        })
    }

    /// Where a Ponko tile of this terrain goes (its corner, for its
    /// `d3::Transform`) when Unity's `Terrain` is at `position`.
    #[must_use]
    pub fn corner(&self, position: [f32; 3]) -> [f32; 3] {
        let [x, y, z] = position;
        [x, y, -z - self.size]
    }
}
