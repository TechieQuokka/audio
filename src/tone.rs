use std::f32::consts::TAU;
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

use libpulse_binding::def::BufferAttr;
use libpulse_binding::sample::{Format, Spec};
use libpulse_binding::stream::Direction;
use libpulse_simple_binding::Simple;

use crate::backend::APP_NAME;
use crate::error::Result;

pub const RATE: u32 = 48_000;
/// Peak level of the rendered melody.
const AMPLITUDE: f32 = 0.3;
/// Length of each recorded chunk, which is also how often the level is reported.
const CHUNK_SECONDS: f32 = 0.1;
/// Length of each block of silence written while the microphone test records.
const KEEP_ALIVE_SECONDS: f32 = 0.05;
/// Peak level recordings are normalized to before playback (-1 dBFS).
const PLAYBACK_PEAK: f32 = 0.89;
/// Normalizing never amplifies by more than this (+30 dB), so silence stays quiet.
const MAX_GAIN: f32 = 31.6;

/// Length of one sixteenth note.
const STEP_SECONDS: f32 = 0.15;
/// Notes starting at or after this step belong to the second phrase.
const SPLIT_STEP: u16 = 17;
const ATTACK_SECONDS: f32 = 0.005;
const RELEASE_SECONDS: f32 = 0.3;
/// Base decay rate; higher harmonics decay faster, like a struck string.
const DECAY: f32 = 2.5;
/// (harmonic, relative amplitude) for a soft piano-like timbre.
const HARMONICS: [(f32, f32); 5] = [
    (1.0, 1.0),
    (2.0, 0.5),
    (3.0, 0.25),
    (4.0, 0.12),
    (5.0, 0.06),
];
/// Left hand notes are quieter so the melody stays on top.
const BASS_GAIN: f32 = 0.6;
/// Notes below this MIDI pitch are treated as left hand.
const BASS_BELOW: u8 = 60;

// MIDI pitches.
const E2: u8 = 40;
const A2: u8 = 45;
const E3: u8 = 52;
const GS3: u8 = 56;
const A3: u8 = 57;
const C4: u8 = 60;
const E4: u8 = 64;
const GS4: u8 = 68;
const A4: u8 = 69;
const B4: u8 = 71;
const C5: u8 = 72;
const D5: u8 = 74;
const DS5: u8 = 75;
const E5: u8 = 76;

/// Opening of Beethoven's "Für Elise" as (start step, pitch, length in steps).
#[rustfmt::skip]
const FUR_ELISE: &[(u16, u8, u16)] = &[
    // Pickup and bar 1
    (0, E5, 1), (1, DS5, 1),
    (2, E5, 1), (3, DS5, 1), (4, E5, 1), (5, B4, 1), (6, D5, 1), (7, C5, 1),
    // Bar 2
    (8, A4, 2), (8, A2, 6), (9, E3, 5), (10, A3, 4),
    (11, C4, 1), (12, E4, 1), (13, A4, 1),
    // Bar 3
    (14, B4, 2), (14, E2, 6), (15, E3, 5), (16, GS3, 4),
    (17, E4, 1), (18, GS4, 1), (19, B4, 1),
    // Bar 4
    (20, C5, 2), (20, A2, 6), (21, E3, 5), (22, A3, 4),
    (23, E4, 1), (24, E5, 1), (25, DS5, 1),
    // Bar 5
    (26, E5, 1), (27, DS5, 1), (28, E5, 1), (29, B4, 1), (30, D5, 1), (31, C5, 1),
    // Bar 6
    (32, A4, 2), (32, A2, 6), (33, E3, 5), (34, A3, 4),
    (35, C4, 1), (36, E4, 1), (37, A4, 1),
    // Bar 7
    (38, B4, 2), (38, E2, 6), (39, E3, 5), (40, GS3, 4),
    (41, E4, 1), (42, C5, 1), (43, B4, 1),
    // Bar 8
    (44, A4, 4), (44, A2, 6), (45, E3, 5), (46, A3, 4),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Left,
    Right,
}

impl Channel {
    pub fn label(self) -> &'static str {
        match self {
            Channel::Left => "Left",
            Channel::Right => "Right",
        }
    }

    fn offset(self) -> usize {
        match self {
            Channel::Left => 0,
            Channel::Right => 1,
        }
    }
}

pub fn frequency(pitch: u8) -> f32 {
    440.0 * 2f32.powf((f32::from(pitch) - 69.0) / 12.0)
}

/// Synthesizes one piano-like note, including its release tail.
fn note(pitch: u8, seconds: f32) -> Vec<f32> {
    let freq = frequency(pitch);
    let len = ((seconds + RELEASE_SECONDS) * RATE as f32) as usize;
    (0..len)
        .map(|i| {
            let t = i as f32 / RATE as f32;
            let attack = (t / ATTACK_SECONDS).min(1.0);
            let release = (1.0 - (t - seconds).max(0.0) / RELEASE_SECONDS).max(0.0);
            let tone: f32 = HARMONICS
                .iter()
                .map(|&(k, amp)| amp * (-t * DECAY * k).exp() * (TAU * freq * k * t).sin())
                .sum();
            attack * release * tone
        })
        .collect()
}

/// Renders the melody as interleaved stereo, routing each note with `route`.
fn render(route: impl Fn(u16) -> Channel) -> Vec<f32> {
    let end = FUR_ELISE
        .iter()
        .map(|&(step, _, steps)| step + steps)
        .max()
        .unwrap_or(0);
    let frames = ((f32::from(end) * STEP_SECONDS + RELEASE_SECONDS) * RATE as f32) as usize;
    let mut out = vec![0.0f32; frames * 2];

    for &(step, pitch, steps) in FUR_ELISE {
        let start = (f32::from(step) * STEP_SECONDS * RATE as f32) as usize;
        let gain = if pitch < BASS_BELOW { BASS_GAIN } else { 1.0 };
        let channel = route(step).offset();
        for (i, sample) in note(pitch, f32::from(steps) * STEP_SECONDS)
            .into_iter()
            .enumerate()
        {
            if let Some(slot) = out.get_mut((start + i) * 2 + channel) {
                *slot += gain * sample;
            }
        }
    }

    let peak = peak(&out);
    if peak > 0.0 {
        out.iter_mut().for_each(|s| *s *= AMPLITUDE / peak);
    }
    out
}

/// Frame where the second phrase starts.
fn split_frame() -> usize {
    (f32::from(SPLIT_STEP) * STEP_SECONDS * RATE as f32) as usize
}

/// Plays the melody on one channel, or with the first phrase on the left and the second on the right.
/// `announce` is called as each channel starts playing.
pub fn play_melody(channel: Option<Channel>, mut announce: impl FnMut(Channel)) -> Result<()> {
    let stream = open(Direction::Playback, 2, "test tone", None, None)?;
    match channel {
        Some(channel) => {
            announce(channel);
            stream.write(&to_bytes(&render(|_| channel)))?;
        }
        None => {
            let samples = render(|step| {
                if step < SPLIT_STEP {
                    Channel::Left
                } else {
                    Channel::Right
                }
            });
            let (left, right) = samples.split_at(split_frame() * 2);
            announce(Channel::Left);
            stream.write(&to_bytes(left))?;
            // Wait until the first phrase has been heard before announcing the switch.
            stream.drain()?;
            announce(Channel::Right);
            stream.write(&to_bytes(right))?;
        }
    }
    stream.drain()?;
    Ok(())
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Converts an amplitude to dBFS; silence is negative infinity.
pub fn dbfs(amplitude: f32) -> f32 {
    20.0 * amplitude.log10()
}

/// Maps an RMS value to 0.0..=1.0 on a -60 dBFS..0 dBFS scale.
pub fn level(rms: f32) -> f32 {
    if rms <= 0.0 {
        return 0.0;
    }
    ((dbfs(rms) + 60.0) / 60.0).clamp(0.0, 1.0)
}

/// Scales samples so their peak reaches -1 dBFS, amplifying by at most +30 dB.
pub fn normalize(samples: &mut [f32]) {
    let peak = peak(samples);
    if peak > 0.0 {
        let gain = (PLAYBACK_PEAK / peak).min(MAX_GAIN);
        samples.iter_mut().for_each(|s| *s *= gain);
    }
}

/// Records from `source` and plays the recording back on `sink`.
///
/// The playback stream is opened first and plays silence for the whole recording:
/// some Bluetooth adapters drop a headset's SCO link after a few seconds of recording
/// alone, and reopening playback afterwards loses its start.
/// `on_level` is called with each chunk's RMS, and `on_recorded` may inspect or
/// change the recording before it is played.
pub fn record_and_play(
    source: &str,
    sink: &str,
    seconds: f32,
    mut on_level: impl FnMut(f32),
    on_recorded: impl FnOnce(&mut [f32]),
) -> Result<()> {
    let (send, receive) = mpsc::channel::<Vec<f32>>();
    thread::scope(|scope| {
        let player = scope.spawn(move || -> Result<()> {
            let keep_alive = frames(KEEP_ALIVE_SECONDS);
            let attr = buffer_attr(keep_alive);
            let stream = open(
                Direction::Playback,
                1,
                "microphone playback",
                Some(sink),
                Some(&attr),
            )?;
            let silence = to_bytes(&vec![0.0; keep_alive]);
            let samples = loop {
                match receive.try_recv() {
                    Ok(samples) => break samples,
                    // Blocks until the server has room, which paces the loop.
                    Err(TryRecvError::Empty) => stream.write(&silence)?,
                    Err(TryRecvError::Disconnected) => return Ok(()),
                }
            };
            stream.write(&to_bytes(&samples))?;
            stream.drain()?;
            Ok(())
        });

        let recorded = record(source, seconds, &mut on_level).map(|mut samples| {
            on_recorded(&mut samples);
            // The player only hangs up on its own errors, which `join` reports.
            let _ = send.send(samples);
        });
        drop(send);
        let played = player.join().expect("playback thread panicked");
        recorded.and(played)
    })
}

/// Records from `source`, calling `on_level` with each chunk's RMS.
fn record(source: &str, seconds: f32, on_level: &mut impl FnMut(f32)) -> Result<Vec<f32>> {
    let chunk = frames(CHUNK_SECONDS);
    let attr = buffer_attr(chunk);
    let stream = open(
        Direction::Record,
        1,
        "microphone test",
        Some(source),
        Some(&attr),
    )?;
    let total = frames(seconds);
    let mut samples = Vec::with_capacity(total);
    let mut buffer = vec![0u8; chunk * size_of::<f32>()];
    while samples.len() < total {
        stream.read(&mut buffer)?;
        let block = from_bytes(&buffer);
        on_level(rms(&block));
        samples.extend(block);
    }
    samples.truncate(total);
    Ok(samples)
}

/// Opens a stream on `device`, or on the default device when it is `None`.
fn open(
    direction: Direction,
    channels: u8,
    stream_name: &str,
    device: Option<&str>,
    attr: Option<&BufferAttr>,
) -> Result<Simple> {
    let spec = Spec {
        format: Format::FLOAT32NE,
        channels,
        rate: RATE,
    };
    Ok(Simple::new(
        None,
        APP_NAME,
        direction,
        device,
        stream_name,
        &spec,
        None,
        attr,
    )?)
}

fn frames(seconds: f32) -> usize {
    (seconds * RATE as f32) as usize
}

/// Buffers of about `frames` mono samples for a responsive stream; the rest is left to the server.
fn buffer_attr(frames: usize) -> BufferAttr {
    let bytes = (frames * size_of::<f32>()) as u32;
    BufferAttr {
        maxlength: u32::MAX,
        tlength: bytes,
        prebuf: u32::MAX,
        minreq: u32::MAX,
        fragsize: bytes,
    }
}

fn to_bytes(samples: &[f32]) -> Vec<u8> {
    samples.iter().flat_map(|s| s.to_ne_bytes()).collect()
}

fn from_bytes(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<{ size_of::<f32>() }>();
    chunks.iter().map(|&b| f32::from_ne_bytes(b)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(stereo: &[f32]) -> f32 {
        stereo.len() as f32 / 2.0 / RATE as f32
    }

    fn channel(stereo: &[f32], channel: Channel) -> impl Iterator<Item = f32> + '_ {
        stereo.iter().skip(channel.offset()).step_by(2).copied()
    }

    #[test]
    fn frequency_follows_equal_temperament() {
        assert!((frequency(A4) - 440.0).abs() < 1e-3);
        assert!((frequency(A4 + 12) - 880.0).abs() < 1e-2);
        assert!((frequency(E5) - 659.255).abs() < 1e-2);
    }

    #[test]
    fn melody_lasts_between_three_and_ten_seconds() {
        let length = seconds(&render(|_| Channel::Left));
        assert!((3.0..=10.0).contains(&length), "{length}s");
    }

    #[test]
    fn melody_is_normalized_without_clipping() {
        let samples = render(|_| Channel::Left);
        let peak = samples.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - AMPLITUDE).abs() < 1e-4);
    }

    #[test]
    fn single_channel_leaves_the_other_silent() {
        let samples = render(|_| Channel::Left);
        assert!(channel(&samples, Channel::Right).all(|s| s == 0.0));
        assert!(channel(&samples, Channel::Left).any(|s| s != 0.0));
    }

    #[test]
    fn split_starts_left_and_ends_right() {
        let samples = render(|step| {
            if step < SPLIT_STEP {
                Channel::Left
            } else {
                Channel::Right
            }
        });
        let (first, second) = samples.split_at(split_frame() * 2);
        assert!(channel(first, Channel::Right).all(|s| s == 0.0));
        assert!(channel(first, Channel::Left).any(|s| s != 0.0));
        assert!(channel(second, Channel::Right).any(|s| s != 0.0));
        // Notes from the first phrase may ring into the second, but never past their release.
        let left_end = FUR_ELISE
            .iter()
            .filter(|&&(step, _, _)| step < SPLIT_STEP)
            .map(|&(step, _, steps)| step + steps)
            .max()
            .unwrap();
        let silent_from =
            ((f32::from(left_end) * STEP_SECONDS + RELEASE_SECONDS) * RATE as f32) as usize + 1;
        assert!(
            channel(&samples, Channel::Left)
                .skip(silent_from)
                .all(|s| s == 0.0)
        );
    }

    #[test]
    fn note_starts_silent_and_fades_out() {
        let samples = note(A4, 0.2);
        assert_eq!(samples[0], 0.0);
        assert!(samples.last().unwrap().abs() < 1e-3);
    }

    #[test]
    fn rms_of_constant_signal() {
        assert_eq!(rms(&[]), 0.0);
        assert!((rms(&[0.5, -0.5, 0.5]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn level_maps_decibels() {
        assert_eq!(level(0.0), 0.0);
        assert_eq!(level(1.0), 1.0);
        assert!((level(0.001) - 0.0).abs() < 1e-6); // -60 dBFS
        assert!((level(0.0316) - 0.5).abs() < 0.01); // about -30 dBFS
    }

    #[test]
    fn dbfs_of_known_amplitudes() {
        assert_eq!(dbfs(1.0), 0.0);
        assert!((dbfs(0.1) + 20.0).abs() < 1e-4);
        assert_eq!(dbfs(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn normalize_raises_peak_to_target() {
        let mut samples = [0.05, -0.1, 0.02];
        normalize(&mut samples);
        assert!((peak(&samples) - PLAYBACK_PEAK).abs() < 1e-6);
        assert!((samples[0] / samples[1] + 0.5).abs() < 1e-6);
    }

    #[test]
    fn normalize_limits_gain() {
        let mut samples = [0.001, -0.0005];
        normalize(&mut samples);
        assert!((peak(&samples) - 0.001 * MAX_GAIN).abs() < 1e-6);
    }

    #[test]
    fn normalize_leaves_silence_alone() {
        let mut samples = [0.0; 4];
        normalize(&mut samples);
        assert_eq!(samples, [0.0; 4]);
    }

    #[test]
    fn bytes_round_trip() {
        let samples = [0.0, 0.25, -1.0];
        assert_eq!(from_bytes(&to_bytes(&samples)), samples);
    }
}
