//! Mixer buses, effects, streams and positional sound from C (Rust
//! `AudioBus`, `AudioEffects`, `AudioSettings3D`, `AudioSource3D`,
//! `AudioListener3D`, `Audio::play_at`, `Audio::set_listener`).

use std::ffi::c_char;

use ponko::audio::AudioEffects;
use ponko::audio::AudioRolloff;
use ponko::d3::AudioListener as AudioListener3D;
use ponko::d3::AudioSettings as AudioSettings3D;
use ponko::d3::AudioSource as AudioSource3D;
use ponko::prelude::{AudioBus, Ctx, Entity, SoundParams};

use crate::ctx::{ctx_access, out, text};
use crate::media::{
    PonkoPlaying, PonkoSound, PonkoSoundParams, playing_from_c, playing_to_c, ponko_sound_params_default, sound_from_c,
    sound_to_c,
};
use crate::three::{PonkoQuat, PonkoVec3, quat_from_c, quat_to_c};
use crate::types::to_c;
use crate::{Failure, PonkoCtx, PonkoStatus, boundary, invalid};

/// A mixer bus from `ponko_ctx_audio_bus` (Rust `AudioBus`). Zero is `sfx`,
/// where ordinary sounds play.
/// ```
/// assert_eq!(ponko_ffi::PonkoAudioBus::default().index, 0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PonkoAudioBus {
    /// Engine-internal index. Never make one up.
    pub index: usize,
}

/// A voice's low-pass filter and reverb send (Rust `AudioEffects`). Start
/// from `ponko_audio_effects_default()`.
/// ```
/// assert_eq!(ponko_ffi::ponko_audio_effects_default().reverb, 0.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoAudioEffects {
    /// Low-pass cutoff in Hz, 20 to 20000. Default 20000 (open).
    pub low_pass_hz: f32,
    /// How much goes to the shared reverb, 0 to 1. Default 0.
    pub reverb: f32,
}

/// No filtering, no reverb.
/// ```
/// assert_eq!(ponko_ffi::ponko_audio_effects_default().low_pass_hz, 20000.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_audio_effects_default() -> PonkoAudioEffects {
    effects_to_c(AudioEffects::default())
}

/// How a positional sound fades with distance (Rust `AudioRolloff`; a
/// keyed curve goes through `ponko_ctx_play_sound_at_keyed` and
/// `ponko_ctx_set_audio_source3d_keyed`).
/// ```
/// let _ = ponko_ffi::PonkoAudioRolloff::PONKO_ROLLOFF_LINEAR;
/// ```
#[repr(C)]
#[allow(non_camel_case_types)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PonkoAudioRolloff {
    /// Inverse distance, like real sound; silent at `max_distance`.
    PONKO_ROLLOFF_LOGARITHMIC = 0,
    /// A straight fade from `min_distance` to `max_distance`.
    PONKO_ROLLOFF_LINEAR = 1,
}

/// Settings for a positional sound (Rust `AudioSettings3D`). Start from
/// `ponko_audio_settings3d_default()`.
/// ```
/// assert_eq!(ponko_ffi::ponko_audio_settings3d_default().doppler, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoAudioSettings3D {
    /// Volume, speed (pitch) and looping.
    pub sound: PonkoSoundParams,
    /// The bus it plays on, chosen when it starts.
    pub bus: PonkoAudioBus,
    /// Its starting low-pass and reverb; change them with `ponko_ctx_set_sound_effects`.
    pub effects: PonkoAudioEffects,
    /// Full volume inside this radius. Default 1.
    pub min_distance: f32,
    /// Silent beyond this radius. Default 100.
    pub max_distance: f32,
    /// The fade between them.
    pub rolloff: PonkoAudioRolloff,
    /// Doppler strength; 0 turns it off. Default 1.
    pub doppler: f32,
}

/// The defaults: full volume within 1 unit, silent at 100, natural fade.
/// ```
/// assert_eq!(ponko_ffi::ponko_audio_settings3d_default().max_distance, 100.0);
/// ```
#[unsafe(no_mangle)]
pub extern "C" fn ponko_audio_settings3d_default() -> PonkoAudioSettings3D {
    let s = AudioSettings3D::default();
    PonkoAudioSettings3D {
        sound: ponko_sound_params_default(),
        bus: bus_to_c(s.bus),
        effects: effects_to_c(s.effects),
        min_distance: s.min_distance,
        max_distance: s.max_distance,
        rolloff: PonkoAudioRolloff::PONKO_ROLLOFF_LOGARITHMIC,
        doppler: s.doppler,
    }
}

/// Where the ears are (Rust `AudioListener3D`): +X is right, -Z forward.
/// ```
/// let l = ponko_ffi::PonkoAudioListener3D {
///     position: ponko_ffi::PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 },
///     rotation: ponko_ffi::PonkoQuat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 },
/// };
/// assert_eq!(l.rotation.w, 1.0);
/// ```
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PonkoAudioListener3D {
    /// Ear position in the world.
    pub position: PonkoVec3,
    /// Which way the head faces.
    pub rotation: PonkoQuat,
}

const _: () = assert!(size_of::<PonkoAudioBus>() == size_of::<AudioBus>());
fn bus_to_c(b: AudioBus) -> PonkoAudioBus {
    // SAFETY: AudioBus is repr(transparent) over usize (size checked above).
    unsafe { std::mem::transmute(b) }
}
fn bus_from_c(b: PonkoAudioBus) -> AudioBus {
    // SAFETY: as above. An unknown index plays on no bus; it is not UB.
    unsafe { std::mem::transmute(b) }
}
fn effects_to_c(e: AudioEffects) -> PonkoAudioEffects {
    PonkoAudioEffects {
        low_pass_hz: e.low_pass_hz,
        reverb: e.reverb,
    }
}
fn effects_from_c(e: *const PonkoAudioEffects) -> AudioEffects {
    // SAFETY: callers pass null or a readable pointer.
    unsafe { e.as_ref() }.map_or_else(AudioEffects::default, |e| AudioEffects {
        low_pass_hz: e.low_pass_hz,
        reverb: e.reverb,
    })
}
fn params_from_c(p: PonkoSoundParams) -> SoundParams {
    SoundParams {
        volume: p.volume,
        speed: p.speed,
        looping: p.looping,
    }
}
pub(crate) fn settings_from_c(s: *const PonkoAudioSettings3D) -> AudioSettings3D {
    // SAFETY: callers pass null or a readable pointer.
    let Some(s) = (unsafe { s.as_ref() }) else {
        return AudioSettings3D::default();
    };
    AudioSettings3D {
        sound: params_from_c(s.sound),
        bus: bus_from_c(s.bus),
        effects: effects_from_c(&s.effects),
        min_distance: s.min_distance,
        max_distance: s.max_distance,
        rolloff: match s.rolloff {
            PonkoAudioRolloff::PONKO_ROLLOFF_LINEAR => AudioRolloff::Linear,
            PonkoAudioRolloff::PONKO_ROLLOFF_LOGARITHMIC => AudioRolloff::Logarithmic,
        },
        doppler: s.doppler,
    }
}
fn entity(c: &Ctx, bits: u64) -> Result<Entity, Failure> {
    let e = Entity::from_bits(bits);
    if c.world.is_alive(e) {
        Ok(e)
    } else {
        Err(invalid("no such entity"))
    }
}

/// The bus named `name`, made the first time it's asked for (Rust
/// `Audio::bus`). Keep the handle: names are looked up only here.
/// ```
/// let mut bus = ponko_ffi::PonkoAudioBus::default();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_audio_bus(std::ptr::null_mut(), c"music".as_ptr(), &mut bus) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `name` NUL-terminated; `out_bus` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_audio_bus(
    ctx: *mut PonkoCtx,
    name: *const c_char,
    out_bus: *mut PonkoAudioBus,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let name = text(name)?;
            out(out_bus, bus_to_c(c.audio.bus(name)))
        })
    })
}

/// Sets a bus's volume, 0 (silent) to 1 (as recorded) or more.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_audio_bus_volume(std::ptr::null_mut(), Default::default(), 0.5) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_audio_bus_volume(
    ctx: *mut PonkoCtx,
    bus: PonkoAudioBus,
    volume: f32,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.audio.set_bus_volume(bus_from_c(bus), volume);
            Ok(())
        })
    })
}

/// Mutes or unmutes a bus, keeping its volume.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_mute_audio_bus(std::ptr::null_mut(), Default::default(), true) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_mute_audio_bus(ctx: *mut PonkoCtx, bus: PonkoAudioBus, muted: bool) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.audio.mute_bus(bus_from_c(bus), muted);
            Ok(())
        })
    })
}

/// Plays a sound on a bus with effects; `params` and `effects` may be null
/// for the defaults. Writes the playback to `out_playing` unless that is null.
/// ```
/// let s = ponko_ffi::PonkoSound { id: 0, seconds: 0.0 };
/// let null = std::ptr::null_mut();
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_play_sound_on(null, s, std::ptr::null(), Default::default(), std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `params` and `effects` null or readable; `out_playing` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_play_sound_on(
    ctx: *mut PonkoCtx,
    sound: PonkoSound,
    params: *const PonkoSoundParams,
    bus: PonkoAudioBus,
    effects: *const PonkoAudioEffects,
    out_playing: *mut PonkoPlaying,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let p = params.as_ref().copied().unwrap_or_else(|| ponko_sound_params_default());
            let playing = c.audio.play_on(
                sound_from_c(sound),
                params_from_c(p),
                bus_from_c(bus),
                effects_from_c(effects),
            );
            if !out_playing.is_null() {
                out_playing.write(playing_to_c(playing));
            }
            Ok(())
        })
    })
}

/// Changes a playing voice's low-pass and reverb, smoothly. Does nothing
/// for one that has stopped.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_sound_effects(std::ptr::null_mut(), Default::default(), std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `effects` null (the defaults) or readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_sound_effects(
    ctx: *mut PonkoCtx,
    playing: PonkoPlaying,
    effects: *const PonkoAudioEffects,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.audio.set_effects(playing_from_c(playing), effects_from_c(effects));
            Ok(())
        })
    })
}

/// Opens a long sound (music) from the assets folder to stream as it plays,
/// instead of decoding it all up front. Play it like any sound.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_load_stream(std::ptr::null_mut(), c"a.ogg".as_ptr(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `path` NUL-terminated; `out_sound` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_load_stream(
    ctx: *mut PonkoCtx,
    path: *const c_char,
    out_sound: *mut PonkoSound,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let path = text(path)?;
            if out_sound.is_null() {
                return Err(invalid("output is null"));
            }
            out(out_sound, sound_to_c(c.audio.load_stream(path)))
        })
    })
}

/// Plays a sound from a point in the world (Rust `Audio::play_at`);
/// `settings` may be null for the defaults. Its volume, panning and doppler
/// follow the listener every frame.
/// ```
/// let s = ponko_ffi::PonkoSound { id: 0, seconds: 0.0 };
/// let at = ponko_ffi::PonkoVec3 { x: 0.0, y: 0.0, z: 0.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_play_sound_at(std::ptr::null_mut(), s, at, std::ptr::null(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `settings` null or readable; `out_playing` null or writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_play_sound_at(
    ctx: *mut PonkoCtx,
    sound: PonkoSound,
    position: PonkoVec3,
    settings: *const PonkoAudioSettings3D,
    out_playing: *mut PonkoPlaying,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let playing = c
                .audio
                .play_at(sound_from_c(sound), to_c(position), settings_from_c(settings));
            if !out_playing.is_null() {
                out_playing.write(playing_to_c(playing));
            }
            Ok(())
        })
    })
}

/// Makes `entity` (which needs a 3D transform) play `sound` from where it is
/// (Rust `AudioSource3D`); `settings` may be null for the defaults. It starts
/// on the next frame and follows the entity. Setting it again replaces it.
/// ```
/// let s = ponko_ffi::PonkoSound { id: 0, seconds: 0.0 };
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_audio_source3d(std::ptr::null_mut(), 0, s, std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `settings` null or readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_audio_source3d(
    ctx: *mut PonkoCtx,
    entity_bits: u64,
    sound: PonkoSound,
    settings: *const PonkoAudioSettings3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            let source = AudioSource3D::new(sound_from_c(sound), settings_from_c(settings));
            c.world.insert(e, source);
            Ok(())
        })
    })
}

/// Stops `entity`'s positional sound and takes it off. Does nothing if it
/// has none.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_remove_audio_source3d(std::ptr::null_mut(), 0) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_remove_audio_source3d(ctx: *mut PonkoCtx, entity_bits: u64) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let e = entity(c, entity_bits)?;
            c.world.remove::<AudioSource3D>(e);
            Ok(())
        })
    })
}

/// Puts the ears somewhere other than the camera, such as on the player;
/// null goes back to following the camera.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_set_audio_listener(std::ptr::null_mut(), std::ptr::null()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `listener` null or readable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_set_audio_listener(
    ctx: *mut PonkoCtx,
    listener: *const PonkoAudioListener3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            c.audio.set_listener(listener.as_ref().map(|l| AudioListener3D {
                position: to_c(l.position),
                rotation: quat_from_c(l.rotation),
            }));
            Ok(())
        })
    })
}

/// Where the ears are now: the listener set with
/// `ponko_ctx_set_audio_listener`, or else the camera.
/// ```
/// assert_eq!(unsafe { ponko_ffi::ponko_ctx_audio_listener(std::ptr::null_mut(), std::ptr::null_mut()) } as u32, 1);
/// ```
/// # Safety
/// `ctx` null or live; `out_listener` writable.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn ponko_ctx_audio_listener(
    ctx: *mut PonkoCtx,
    out_listener: *mut PonkoAudioListener3D,
) -> PonkoStatus {
    // SAFETY: forwarded caller contract.
    boundary(|| unsafe {
        ctx_access(ctx, |c| {
            let l = c.audio.listener();
            out(
                out_listener,
                PonkoAudioListener3D {
                    position: to_c(l.position),
                    rotation: quat_to_c(l.rotation),
                },
            )
        })
    })
}
