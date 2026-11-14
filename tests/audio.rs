//! Sounds, checked headless: loading and decoding work, playing is silent.

use ponko::prelude::*;

fn in_tick(f: impl FnMut(&mut Ctx) + 'static) {
    Game::new("audio", 1, 1).sim_system(f).run_headless(1, |_| {});
}

// Decoding sound files needs the `audio` feature: without it a sound only
// has to exist, and lasts zero seconds.

/// A mono 16-bit PCM WAV file.
#[cfg(feature = "audio")]
fn wav(sample_rate: u32, samples: &[i16]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

#[cfg(feature = "audio")]
fn temp_file(name: &str, bytes: &[u8]) -> String {
    let path = std::env::temp_dir().join(format!("ponko-audio-{}-{name}", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn generated_sounds_know_their_length() {
    in_tick(|ctx| {
        let beep = ctx.audio.create(8000, &vec![0.0; 4000]);
        assert!((beep.seconds() - 0.5).abs() < 1e-3, "{}", beep.seconds());
        let playing = ctx.audio.play(beep);
        ctx.audio.stop(playing); // silent headless, but fine to call
    });
}

#[cfg(feature = "audio")]
#[test]
fn wav_files_load() {
    let path = temp_file("tone.wav", &wav(22_050, &vec![1000; 22_050 / 4]));
    in_tick(move |ctx| {
        let tone = ctx.audio.load(&path);
        assert!((tone.seconds() - 0.25).abs() < 0.01, "{}", tone.seconds());
        ctx.audio.play_ex(
            tone,
            SoundParams {
                volume: 0.5,
                speed: 2.0,
                looping: true,
            },
        );
    });
}

#[cfg(feature = "audio")]
#[test]
#[should_panic(expected = "could not decode the sound")]
fn files_that_are_not_sound_say_so() {
    let path = temp_file("bad.ogg", b"this is not audio");
    Game::new("audio", 1, 1).run_headless(0, move |ctx| {
        ctx.audio.load(&path);
    });
}

#[test]
#[should_panic(expected = "not found in assets/")]
fn missing_sounds_say_where_they_looked() {
    Game::new("audio", 1, 1).run_headless(0, |ctx| {
        ctx.audio.load("no-such-sound.ogg");
    });
}

#[cfg(feature = "audio")]
#[test]
fn loading_a_sound_file_twice_gives_the_same_sound() {
    let path = temp_file("twice.wav", &wav(22_050, &vec![1000; 2205]));
    in_tick(move |ctx| {
        let a = ctx.audio.load(&path);
        let b = ctx.audio.load(&path);
        assert_eq!(a, b);
        assert_ne!(a, ctx.audio.create(8000, &[0.0; 80]));
    });
}
