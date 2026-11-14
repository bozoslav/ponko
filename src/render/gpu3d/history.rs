//! Temporal history: what the renderer keeps from one frame for the next.
//!
//! Most of a frame's state is rewritten by every `prepare`. The exceptions
//! are the images and cameras the temporal effects reuse: TAA's history,
//! temporal AO's, the reflections' last image, and the camera that motion
//! vectors and camera cuts are measured from. [`History`] owns the cameras
//! and TAA's frame counters, and makes the one decision per frame that all
//! of them follow ([`HistoryReset`]): whether the last frame is still a
//! past of this one. The images themselves live in the targets
//! (`targets.rs`); the effects forget theirs when told to
//! (`Renderer3D::forget_history`).

use glam::{Mat4, Vec3};

/// The cameras and counters carried from frame to frame.
#[derive(Default)]
pub(super) struct History {
    /// A successful reload (or a late rollback) changed the scene's shading.
    pub(super) content_changed: bool,
    /// Frames drawn with temporal antialiasing in a row (the jitter's position).
    pub(super) taa_frame: u32,
    /// Which history image the next frame reads, and the one this frame does.
    pub(super) taa_read: usize,
    pub(super) taa_frame_read: usize,
    /// The last frame's unjittered camera, when its image is in the
    /// history: camera motion is measured from it.
    pub(super) previous: Option<Mat4>,
    /// The unjittered camera of the last frame drawn: camera cuts are
    /// measured from it.
    pub(super) camera: Option<Mat4>,
    /// The time (`frame.time.xy`) of the last frame drawn: a material's
    /// `vertex` runs as of it for the motion pass (`frame.time.zw`).
    pub(super) time: Option<(f32, f32)>,
    /// Why this frame dropped the temporal history, if it did.
    #[cfg(test)]
    pub(super) reset: HistoryReset,
}

impl History {
    /// This frame's decision: why the history is dropped, if it is.
    /// `requested`: the game asked; `targets`: the targets are made again;
    /// `now`: this frame's unjittered camera, checked for a cut.
    pub(super) fn decide(&mut self, requested: bool, targets: bool, now: Mat4) -> HistoryReset {
        let reset = HistoryReset {
            content: std::mem::take(&mut self.content_changed),
            requested,
            targets,
            camera_cut: self.camera.is_some_and(|before| camera_cut(before, now)),
        };
        #[cfg(test)]
        {
            self.reset = reset;
        }
        reset
    }

    /// A frame was resolved by temporal antialiasing with camera `now`: the
    /// next one continues the jitter sequence and reads the image this one
    /// wrote.
    pub(super) fn resolved(&mut self, now: Mat4) {
        self.previous = Some(now);
        self.taa_frame = self.taa_frame.wrapping_add(1);
        self.taa_frame_read = self.taa_read;
        self.taa_read ^= 1;
    }
}

/// Why a frame drops the temporal history: everything an effect kept from
/// earlier frames (TAA's history, temporal AO's, SSR's last image, the
/// camera and models motion vectors are measured from). Decided once per
/// frame in `Renderer3D::prepare`, before any effect prepares, so they all
/// agree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct HistoryReset {
    /// Shader reload or rollback: the old image has different shading.
    pub content: bool,
    /// The game asked (`Graphics::reset_temporal_history`): a teleport or a new
    /// scene the camera can't tell apart from motion.
    pub requested: bool,
    /// The targets are made again (a resize, a render scale change or an
    /// effect/sample count switched, or presentation interrupted/recreated):
    /// old history images have incompatible dimensions/samples or describe a
    /// scene from before presentation stopped. Render-scale controllers write
    /// `Post::render_scale`; prepare_view computes the size before this check.
    pub targets: bool,
    /// The camera jumped far or turned sharply (see `camera_cut`): the
    /// last frame shows somewhere else.
    pub camera_cut: bool,
}

impl HistoryReset {
    /// Whether the frame starts over.
    pub fn any(self) -> bool {
        self.requested || self.targets || self.camera_cut || self.content
    }
}

/// Whether the camera `now` is a cut from the one `before`: it turned a
/// lot or jumped far in one frame, so what was drawn then is no history.
fn camera_cut(before: Mat4, now: Mat4) -> bool {
    // Where the view starts and which way it looks.
    let eye = |m: Mat4| {
        let inverse = m.inverse();
        let near = inverse.project_point3(Vec3::ZERO);
        let far = inverse.project_point3(Vec3::Z);
        (near, (far - near).normalize_or_zero())
    };
    let (a, b) = (eye(before), eye(now));
    !(a.0.distance(b.0) < TAA_CUT_DISTANCE && a.1.dot(b.1) > TAA_CUT_ANGLE.cos())
}

/// A camera cut: this far apart (world units), or turned this much (radians).
const TAA_CUT_DISTANCE: f32 = 50.0;

const TAA_CUT_ANGLE: f32 = std::f32::consts::FRAC_PI_4;
