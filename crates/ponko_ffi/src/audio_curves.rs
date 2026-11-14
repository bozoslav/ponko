//! Positional sound with a keyed distance curve (Rust
//! `AudioRolloff::Keyed`), and whether the audio device may play yet
//! (`Audio::unlocked`).
use crate::audio3d::{PonkoAudioSettings3D, settings_from_c};
use crate::ctx::{ctx_access, out};
use crate::media::{PonkoPlaying, PonkoSound, playing_to_c, sound_from_c};
use crate::three::PonkoVec3;
use crate::types::to_c;
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, invalid};
use ponko::audio::AudioRolloff;
use ponko::d3::AudioSettings as AudioSettings3D;
use ponko::d3::AudioSource as AudioSource3D;
use std::sync::Mutex;

/// One key of a distance curve: at `distance` (0 at the minimum distance, 1
/// at the maximum) the sound plays at `gain` (linear, 1 full volume).
/// ```
/// let k = ponko_ffi::PonkoAudioKey { distance: 0.5, gain: 0.3 };
/// assert_eq!(k.gain, 0.3);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PonkoAudioKey {
    /// Normalized distance, 0 to 1.
    pub distance: f32,
    /// Linear gain there.
    pub gain: f32,
}

/// Every distinct curve C gave, kept for the rest of the program (Rust
/// curves are `&'static`): the same keys again reuse the copy.
static CURVES: Mutex<Vec<&'static [(f32, f32)]>> = Mutex::new(Vec::new());

/// The curve `count` keys at `keys` describe, as a Rust keyed rolloff.
/// # Safety
/// `keys` readable for `count` values (or null with count 0).
unsafe fn keyed(keys: *const PonkoAudioKey, count: usize) -> Result<AudioRolloff, Failure> {
    if keys.is_null() && count > 0 {
        return Err(invalid("keys is null"));
    }
    let given = if count == 0 {
        &[][..]
    } else {
        // SAFETY: caller contract.
        unsafe { std::slice::from_raw_parts(keys, count) }
    };
    if given.iter().any(|k| !k.distance.is_finite() || !k.gain.is_finite()) {
        return Err(invalid("curve keys must be finite"));
    }
    if given.windows(2).any(|w| w[1].distance < w[0].distance) {
        return Err(invalid("curve keys must be sorted by distance"));
    }
    let wanted: Vec<(f32, f32)> = given.iter().map(|k| (k.distance, k.gain)).collect();
    let same = |kept: &[(f32, f32)]| {
        kept.len() == wanted.len()
            && kept
                .iter()
                .zip(&wanted)
                .all(|(a, b)| a.0.to_bits() == b.0.to_bits() && a.1.to_bits() == b.1.to_bits())
    };
    let mut curves = CURVES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let curve = match curves.iter().find(|c| same(c)) {
        Some(c) => *c,
        None => {
            let c: &'static [(f32, f32)] = Box::leak(wanted.into_boxed_slice());
            curves.push(c);
            c
        }
    };
    Ok(AudioRolloff::Keyed(curve))
}

/// `settings` (null for the defaults) with the keyed curve instead of its rolloff.
/// # Safety
/// `settings` null or readable; `keys` as for [`keyed`].
unsafe fn keyed_settings(
    settings: *const PonkoAudioSettings3D,
    keys: *const PonkoAudioKey,
    count: usize,
) -> Result<AudioSettings3D, Failure> {
    let mut s = settings_from_c(settings);
    // SAFETY: forwarded caller contract.
    s.rolloff = unsafe { keyed(keys, count)? };
    Ok(s)
}

/// `ponko_ctx_play_sound_at` with a keyed distance curve instead of
/// `settings`' rolloff (Rust `AudioRolloff::Keyed`): `count` keys sorted by
/// distance, from 0 (at `min_distance`) to 1 (at `max_distance`); between
/// keys the gain is interpolated, outside them it is the nearest key's. No
/// keys: silent. Each distinct curve is kept for the rest of the program,
/// so make curves once, not per frame. Unsorted or non-finite keys are an
/// error.
/// ```
/// let s = ponko_ffi::PonkoSound { id: 0, seconds: 0.0 };
/// let at = ponko_ffi::PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_play_sound_at_keyed(std::ptr::null_mut(), s, at, std::ptr::null(), std::ptr::null(), 0, std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `settings` null or readable; `keys` readable for
/// `count` values (or null with count 0); `out_playing` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_play_sound_at_keyed(
    ctx: *mut PonkoCtx,
    sound: PonkoSound,
    position: PonkoVec3,
    settings: *const PonkoAudioSettings3D,
    keys: *const PonkoAudioKey,
    count: usize,
    out_playing: *mut PonkoPlaying,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let s = keyed_settings(settings, keys, count)?;
        ctx_access(ctx, |c| {
            let playing = c.audio.play_at(sound_from_c(sound), to_c(position), s);
            if !out_playing.is_null() {
                out_playing.write(playing_to_c(playing));
            }
            Ok(())
        })
    })
}

/// `ponko_ctx_set_audio_source3d` with a keyed distance curve instead of
/// `settings`' rolloff, as for `ponko_ctx_play_sound_at_keyed`.
/// ```
/// let s = ponko_ffi::PonkoSound { id: 0, seconds: 0.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_audio_source3d_keyed(std::ptr::null_mut(), 0, s, std::ptr::null(), std::ptr::null(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `settings` null or readable; `keys` readable for
/// `count` values (or null with count 0).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_audio_source3d_keyed(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    sound: PonkoSound,
    settings: *const PonkoAudioSettings3D,
    keys: *const PonkoAudioKey,
    count: usize,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        let s = keyed_settings(settings, keys, count)?;
        ctx_access(ctx, |c| {
            let e = ponko::prelude::Entity::from_bits(entity_bits);
            if !c.world.is_alive(e) {
                return Err(invalid("no such entity"));
            }
            c.world.insert(e, AudioSource3D::new(sound_from_c(sound), s));
            Ok(())
        })
    })
}

/// Whether sound can play yet (Rust `Audio::unlocked`): on the web, `false`
/// until the player's first click or key press, while music and loops
/// wait and one-shots are dropped; always `true` on the desktop. Show a
/// "click to start" screen while it is `false` if the first sounds matter.
/// ```
/// let mut on = false;
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_audio_unlocked(std::ptr::null_mut(), &mut on) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_unlocked` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_audio_unlocked(ctx: *mut PonkoCtx, out_unlocked: *mut bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe { ctx_access(ctx, |c| out(out_unlocked, c.audio.unlocked())) })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(list: &[(f32, f32)]) -> Vec<PonkoAudioKey> {
        list.iter()
            .map(|&(distance, gain)| PonkoAudioKey { distance, gain })
            .collect()
    }

    #[test]
    fn keyed_curves_carry_the_keys_and_equal_curves_share_one_copy() {
        let given = keys(&[(0.0, 1.0), (0.5, 0.3), (1.0, 0.0)]);
        // SAFETY: a live vector of `len` keys.
        let (a, b) = unsafe {
            (
                keyed(given.as_ptr(), 3).ok().unwrap(),
                keyed(given.clone().as_ptr(), 3).ok().unwrap(),
            )
        };
        let (AudioRolloff::Keyed(a), AudioRolloff::Keyed(b)) = (a, b) else {
            panic!("not keyed");
        };
        assert_eq!(a, [(0.0, 1.0), (0.5, 0.3), (1.0, 0.0)]);
        assert!(std::ptr::eq(a, b), "the same keys again reuse the kept copy");
        let other = keys(&[(0.0, 1.0), (1.0, 0.5)]);
        // SAFETY: as above.
        let AudioRolloff::Keyed(c) = (unsafe { keyed(other.as_ptr(), 2).ok().unwrap() }) else {
            panic!("not keyed");
        };
        assert!(!std::ptr::eq(a, c));
        // SAFETY: null with no keys.
        assert!(matches!(
            unsafe { keyed(std::ptr::null(), 0) },
            Ok(AudioRolloff::Keyed([]))
        ));
    }
}
