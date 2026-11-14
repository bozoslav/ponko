//! The screen-space passes that read the scene's depth and motion before
//! or after it is lit: temporal antialiasing (`../taa.rs`), ambient
//! occlusion with contact shadows (`../screen.rs`) and reflections
//! (`../ssr.rs`).
//!
//! [`ScreenSpace`] owns their pipelines (made the first frame each is on)
//! and which of them run this frame. `Renderer3D::prepare` decides the
//! flags from the game's `Post` settings and the device (step 1), the
//! scene's images are made for the ones that are on (`targets.rs`), and
//! recording reads them. Their histories are their own, dropped with the
//! frame's (`History::decide`).

use crate::render::screen::Ao;
use crate::render::ssr::Ssr;
use crate::render::taa::Taa;

pub(super) struct ScreenSpace {
    /// Temporal antialiasing: the resolve pass (made when first used).
    pub(super) taa: Option<Taa>,
    /// Whether this device can run it (WebGL2 can't: FXAA stands in).
    pub(super) taa_supported: bool,
    /// Whether this frame is resolved by it.
    pub(super) taa_on: bool,
    /// Whether moved models are drawn again for their motion vectors
    /// (temporal antialiasing or temporal ambient occlusion).
    pub(super) velocity_on: bool,
    /// Whether the motion pass also draws the models a shader's `vertex`
    /// moves while they hold still (see `vertex_motion.rs`).
    pub(super) still_motion: bool,
    /// Ambient occlusion and contact shadows (made when first used), and
    /// whether this frame has them.
    pub(super) ao: Option<Ao>,
    pub(super) ao_enabled: bool,
    /// Screen-space reflections: the passes (made when first used), and
    /// whether this frame has them.
    pub(super) ssr: Option<Ssr>,
    pub(super) ssr_on: bool,
}

impl ScreenSpace {
    /// `taa_supported`: whether the device has compute-class limits.
    pub(super) fn new(taa_supported: bool) -> Self {
        Self {
            taa: None,
            taa_supported,
            taa_on: false,
            velocity_on: false,
            still_motion: false,
            ao: None,
            ao_enabled: false,
            ssr: None,
            ssr_on: false,
        }
    }
}
