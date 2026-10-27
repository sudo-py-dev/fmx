//! The sound FMX plays when the queue finishes.
//!
//! The built-in sounds are synthesized here, so they need no sound files and
//! sound the same everywhere. A custom file can be MP3, OGG, FLAC, WAV or M4A.
//!
//! - Windows and macOS play in-process with rodio, so the sound starts at once.
//! - Linux plays a WAV with the system's player (`pw-play`, `paplay` or
//!   `aplay`, whichever works), so FMX needs no audio libraries to build or
//!   start; custom files are converted to WAV with the bundled ffmpeg first.

use std::f32::consts::TAU;
use std::path::PathBuf;
use std::time::Duration;

use crate::settings::CompletionSound;

const SAMPLE_RATE: u32 = 48_000;
/// Loudest point of a built-in sound: a gentle notification, not an alarm.
const PEAK: f32 = 0.45;
/// A custom file stops after this long, so a whole song never plays.
const MAX_CUSTOM: Duration = Duration::from_secs(10);
/// Fade-in on every note, so it starts without a click.
const ATTACK: f32 = 0.006;
/// Fade-out at the end, so the cut-off tail ends without a click.
const RELEASE: f32 = 0.1;

/// Audio file types a custom sound can be.
pub const CUSTOM_EXTENSIONS: &[&str] = &["mp3", "wav", "ogg", "oga", "flac", "m4a"];

/// One struck note: its pitch, when it starts, how long it rings, and the
/// overtones (frequency ratio, loudness) that give it its timbre.
struct Note {
    hz: f32,
    start: f32,
    decay: f32,
    partials: &'static [(f32, f32)],
}

/// A soft chime: mostly the fundamental, with a quiet octave and twelfth.
const CHIME: &[(f32, f32)] = &[(1.0, 1.0), (2.0, 0.35), (3.0, 0.12)];
/// A bell: inharmonic overtones, like a struck metal bowl.
const BELL: &[(f32, f32)] = &[(1.0, 1.0), (2.0, 0.5), (2.76, 0.35), (5.4, 0.15)];

const C5: f32 = 523.25;
const E5: f32 = 659.25;
const G5: f32 = 783.99;
const A5: f32 = 880.0;
const C6: f32 = 1046.5;

/// The notes of a built-in sound; `None` for `Off` and custom files.
fn notes(sound: &CompletionSound) -> Option<Vec<Note>> {
    let note = |hz, start, decay, partials| Note {
        hz,
        start,
        decay,
        partials,
    };
    Some(match sound {
        // Two rising notes: done.
        CompletionSound::Default => vec![note(E5, 0.0, 0.35, CHIME), note(A5, 0.13, 0.5, CHIME)],
        CompletionSound::Bell => vec![note(A5, 0.0, 0.9, BELL)],
        // A quick major arpeggio that lands on a held high note.
        CompletionSound::Victory => vec![
            note(C5, 0.0, 0.18, CHIME),
            note(E5, 0.1, 0.18, CHIME),
            note(G5, 0.2, 0.18, CHIME),
            note(C6, 0.3, 0.6, CHIME),
        ],
        CompletionSound::Off | CompletionSound::Custom(_) => return None,
    })
}

/// Renders notes to mono samples at [`SAMPLE_RATE`], scaled to [`PEAK`].
fn render(notes: &[Note]) -> Vec<f32> {
    // Until the last note has faded to about -43 dB: quiet enough to end there.
    let length = notes
        .iter()
        .map(|n| n.start + n.decay * 5.0)
        .fold(0.0, f32::max);
    let mut samples = vec![0.0f32; (length * SAMPLE_RATE as f32) as usize];
    for note in notes {
        let first = (note.start * SAMPLE_RATE as f32) as usize;
        for (i, sample) in samples.iter_mut().enumerate().skip(first) {
            let t = (i - first) as f32 / SAMPLE_RATE as f32;
            let attack = (t / ATTACK).min(1.0);
            for &(ratio, loudness) in note.partials {
                // Higher overtones die away sooner, as in real bells.
                let envelope = (-t * ratio.sqrt() / note.decay).exp();
                *sample += attack * loudness * envelope * (TAU * note.hz * ratio * t).sin();
            }
        }
    }
    let release = (RELEASE * SAMPLE_RATE as f32) as usize;
    let total = samples.len();
    for (i, sample) in samples
        .iter_mut()
        .enumerate()
        .skip(total.saturating_sub(release))
    {
        *sample *= (total - i) as f32 / release as f32;
    }
    let loudest = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
    if loudest > 0.0 {
        samples.iter_mut().for_each(|s| *s *= PEAK / loudest);
    }
    samples
}

/// A 16-bit mono WAV file holding `samples`.
fn wav_bytes(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + samples.len() * 2);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes()); // format chunk size
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    out.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes()); // bytes per second
    out.extend_from_slice(&2u16.to_le_bytes()); // bytes per frame
    out.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16).to_le_bytes());
    }
    out
}

/// Plays `sound` on a background thread; problems only go to the log.
/// `ffmpeg` converts custom files on Linux.
pub fn play(sound: CompletionSound, ffmpeg: Option<PathBuf>) {
    if sound == CompletionSound::Off {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("fmx-sound".into())
        .spawn(move || {
            if let Err(e) = play_now(&sound, ffmpeg) {
                tracing::warn!(error = %e, ?sound, "could not play the completion sound");
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "could not start the completion sound");
    }
}

/// Plays `sound` and returns once it has finished.
#[cfg(not(target_os = "linux"))]
fn play_now(
    sound: &CompletionSound,
    _ffmpeg: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::num::NonZero;

    use rodio::Source as _;

    let mut output = rodio::DeviceSinkBuilder::open_default_sink()?;
    output.log_on_drop(false);
    let player = rodio::Player::connect_new(output.mixer());
    match sound {
        CompletionSound::Custom(path) => {
            let decoder = rodio::Decoder::try_from(std::fs::File::open(path)?)?;
            player.append(decoder.take_duration(MAX_CUSTOM));
        }
        builtin => {
            let Some(notes) = notes(builtin) else {
                return Ok(());
            };
            let mono = NonZero::new(1).expect("one channel");
            let rate = NonZero::new(SAMPLE_RATE).expect("non-zero rate");
            player.append(rodio::buffer::SamplesBuffer::new(
                mono,
                rate,
                render(&notes),
            ));
        }
    }
    player.sleep_until_end();
    Ok(())
}

/// Players to try in turn, with their arguments before the file.
#[cfg(target_os = "linux")]
const PLAYERS: &[(&str, &[&str])] = &[("pw-play", &[]), ("paplay", &[]), ("aplay", &["-q"])];

/// Plays `sound` and returns once it has finished.
#[cfg(target_os = "linux")]
fn play_now(
    sound: &CompletionSound,
    ffmpeg: Option<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    use std::path::Path;

    use crate::process::command;

    let wav = tempfile::Builder::new()
        .prefix("fmx-sound")
        .suffix(".wav")
        .tempfile()?;
    let file: PathBuf = match sound {
        // Converted to a short WAV every player can play; without ffmpeg the
        // players try the file as it is (most read OGG, FLAC and WAV).
        CompletionSound::Custom(path) => match ffmpeg {
            Some(ffmpeg) => {
                let status = command(&ffmpeg)
                    .args(["-v", "error", "-nostdin", "-y", "-i"])
                    .arg(path)
                    .args(["-t", &MAX_CUSTOM.as_secs().to_string(), "-ac", "2"])
                    .arg(wav.path())
                    .status()?;
                if !status.success() {
                    return Err(format!("ffmpeg could not read {}", path.display()).into());
                }
                wav.path().to_path_buf()
            }
            None => path.clone(),
        },
        builtin => {
            let Some(notes) = notes(builtin) else {
                return Ok(());
            };
            std::fs::write(wav.path(), wav_bytes(&render(&notes)))?;
            wav.path().to_path_buf()
        }
    };
    let mut failures = Vec::new();
    for (player, args) in PLAYERS {
        // A player that exists but can't reach a sound server fails, and the next one tries.
        match command(Path::new(player)).args(*args).arg(&file).status() {
            Ok(status) if status.success() => return Ok(()),
            Ok(status) => failures.push(format!("{player}: {status}")),
            Err(e) => failures.push(format!("{player}: {e}")),
        }
    }
    Err(format!("no audio player worked ({})", failures.join("; ")).into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn built_in_sounds_are_short_gentle_and_different() {
        let sounds = [
            CompletionSound::Default,
            CompletionSound::Bell,
            CompletionSound::Victory,
        ];
        let rendered: Vec<Vec<f32>> = sounds
            .iter()
            .map(|s| render(&notes(s).expect("built in")))
            .collect();
        for samples in &rendered {
            let seconds = samples.len() as f32 / SAMPLE_RATE as f32;
            assert!((0.5..=7.0).contains(&seconds), "{seconds} s");
            let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!((peak - PEAK).abs() < 1e-3, "peak {peak}");
            // Starts silently (no click) and ends silently.
            assert!(samples[0].abs() < 0.01);
            assert!(samples.last().expect("samples").abs() < 0.01);
        }
        assert_ne!(rendered[0], rendered[1]);
        assert_ne!(rendered[1], rendered[2]);
    }

    #[test]
    fn wav_header_matches_the_samples() {
        let bytes = wav_bytes(&[0.0, 1.0, -1.0]);
        assert_eq!(&bytes[..4], b"RIFF");
        assert_eq!(&bytes[8..16], b"WAVEfmt ");
        assert_eq!(bytes.len(), 44 + 6);
        assert_eq!(
            u32::from_le_bytes(bytes[40..44].try_into().expect("4 bytes")),
            6
        );
        assert_eq!(i16::from_le_bytes([bytes[46], bytes[47]]), i16::MAX);
    }

    /// Run with `cargo test plays_each_built_in_sound -- --ignored` to hear them.
    #[test]
    #[ignore = "plays sounds on this computer's speakers"]
    fn plays_each_built_in_sound() {
        for sound in [
            CompletionSound::Default,
            CompletionSound::Bell,
            CompletionSound::Victory,
        ] {
            play_now(&sound, None).expect("played");
        }
    }

    #[test]
    fn off_and_custom_have_no_notes() {
        assert!(notes(&CompletionSound::Off).is_none());
        assert!(notes(&CompletionSound::Custom("a.mp3".into())).is_none());
    }
}
