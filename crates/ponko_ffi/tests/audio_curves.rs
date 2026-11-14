//! Positional sound with keyed distance curves from C
//! (`AudioRolloff::Keyed`), and whether audio is unlocked (`Audio::unlocked`).
mod common;

use PonkoStatus::*;
use common::{last_error, run_ctx};
use ponko_ffi::*;
use std::ptr::{null, null_mut};

fn key(distance: f32, gain: f32) -> PonkoAudioKey {
    PonkoAudioKey { distance, gain }
}

const AT: PonkoVec3 = PonkoVec3 { x: 3.0, y: 0.0, z: 0.0 };

#[test]
fn keyed_curves_play_sounds_and_drive_sources() {
    run_ctx(|ctx| unsafe {
        let samples = vec![0.1f32; 800];
        let mut sound = PonkoSound { id: 0, seconds: 0.0 };
        assert_eq!(
            ponko_ctx_create_sound(ctx, 8000, samples.as_ptr(), samples.len(), &mut sound),
            PONKO_OK
        );
        let curve = [key(0.0, 1.0), key(0.5, 0.3), key(1.0, 0.0)];
        let settings = ponko_audio_settings3d_default();
        let mut playing = PonkoPlaying { id: 0 };
        assert_eq!(
            ponko_ctx_play_sound_at_keyed(ctx, sound, AT, &settings, curve.as_ptr(), 3, &mut playing),
            PONKO_OK,
            "{}",
            last_error()
        );
        // Null settings take the defaults; no keys is a silent curve.
        assert_eq!(
            ponko_ctx_play_sound_at_keyed(ctx, sound, AT, null(), null(), 0, null_mut()),
            PONKO_OK
        );

        let w = ponko_ctx_world(ctx);
        let mut transform = 0;
        assert_eq!(ponko_transform3d_component(w, &mut transform), PONKO_OK);
        let t = ponko_transform3d_at(AT);
        let value = PonkoValue {
            component: transform,
            data: (&raw const t).cast(),
            size: size_of::<PonkoTransform3D>(),
        };
        let mut e = 0;
        assert_eq!(ponko_spawn(w, &value, 1, &mut e), PONKO_OK);
        assert_eq!(
            ponko_ctx_set_audio_source3d_keyed(ctx, e, sound, &settings, curve.as_ptr(), 3),
            PONKO_OK
        );
        assert_eq!(ponko_ctx_remove_audio_source3d(ctx, e), PONKO_OK);

        // Unsorted, non-finite or missing keys are refused.
        let unsorted = [key(0.5, 1.0), key(0.0, 0.0)];
        let nan = [key(0.0, f32::NAN)];
        for (keys, count) in [(unsorted.as_ptr(), 2), (nan.as_ptr(), 1), (null(), 1)] {
            assert_eq!(
                ponko_ctx_play_sound_at_keyed(ctx, sound, AT, &settings, keys, count, null_mut()),
                PONKO_INVALID_ARGUMENT
            );
            assert_eq!(
                ponko_ctx_set_audio_source3d_keyed(ctx, e, sound, &settings, keys, count),
                PONKO_INVALID_ARGUMENT
            );
        }
        assert!(last_error().contains("keys"), "{}", last_error());
        // A stale entity is refused.
        assert_eq!(ponko_despawn(w, e), PONKO_OK);
        assert_eq!(
            ponko_ctx_set_audio_source3d_keyed(ctx, e, sound, &settings, curve.as_ptr(), 3),
            PONKO_INVALID_ARGUMENT
        );
    });
}

#[test]
fn audio_is_unlocked_on_the_desktop() {
    run_ctx(|ctx| unsafe {
        let mut unlocked = false;
        assert_eq!(ponko_ctx_audio_unlocked(ctx, &mut unlocked), PONKO_OK);
        assert!(unlocked);
        assert_eq!(ponko_ctx_audio_unlocked(ctx, null_mut()), PONKO_INVALID_ARGUMENT);
    });
}
