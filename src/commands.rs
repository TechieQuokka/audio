use std::io::{self, Write};

use crate::backend::Backend;
use crate::cli::{Cli, Command, MicAction};
use crate::device::{self, Card, Device, Kind, Mode};
use crate::error::{Error, Result};
use crate::tone::{self, Channel};

const MIC_TEST_SECONDS: f32 = 3.0;
const LEVEL_BAR_WIDTH: usize = 30;
/// Recordings peaking below this are reported as nearly silent.
const QUIET_DBFS: f32 = -50.0;

pub fn run(cli: Cli) -> Result<()> {
    match (cli.command, cli.number) {
        (None, Some(number)) => switch(number),
        (None, None) | (Some(Command::List), _) => list(),
        (Some(Command::Mic { action }), _) => match action {
            MicAction::On => mic(Mode::Headset),
            MicAction::Off => mic(Mode::HighFidelity),
            MicAction::Status => mic_status(),
            MicAction::Test => mic_test(),
        },
        (Some(Command::Test { left, right }), _) => test(left, right),
    }
}

fn list() -> Result<()> {
    let mut backend = Backend::connect()?;
    let defaults = backend.defaults()?;
    let mut sinks = backend.sinks()?;
    if sinks.is_empty() {
        return Err(Error::NoDevices);
    }
    device::sort(&mut sinks);

    let width = sinks.iter().map(|d| d.description.len()).max().unwrap_or(0);
    for (i, sink) in sinks.iter().enumerate() {
        let marker = if defaults.sink.as_deref() == Some(sink.name.as_str()) {
            '*'
        } else {
            ' '
        };
        println!(
            "{marker} {:>2}  {:<width$}  {}",
            i + 1,
            sink.description,
            sink.kind.label()
        );
    }

    let sources = backend.sources()?;
    if let Some(mic) = defaults
        .source
        .and_then(|name| sources.into_iter().find(|d| d.name == name))
    {
        println!("\n🎤 {}", mic.description);
    }
    Ok(())
}

fn switch(number: usize) -> Result<()> {
    let mut backend = Backend::connect()?;
    let mut sinks = backend.sinks()?;
    device::sort(&mut sinks);
    let mut target = device::select(&sinks, number)?.clone();

    // Leaving or entering a Bluetooth output always turns its microphone off,
    // so no card is left stuck in call quality.
    for card in backend.bluetooth_cards()? {
        if card.mode() != Some(Mode::Headset) {
            continue;
        }
        let profile = card
            .best_profile(Mode::HighFidelity)
            .ok_or(Error::NoProfile(Mode::HighFidelity.label()))?;
        backend.set_card_profile(&card.name, profile)?;
        if target.card == Some(card.index) {
            target = backend.wait_for_sink(card.index)?;
        }
    }

    let mic = builtin_mic(&mut backend)?;
    apply(&mut backend, &target, mic.as_ref())
}

fn mic(mode: Mode) -> Result<()> {
    let mut backend = Backend::connect()?;
    let (_, card) = current_bluetooth_card(&mut backend)?;
    let card_index = card.index;

    if card.mode() == Some(mode) {
        let state = if mode == Mode::Headset { "on" } else { "off" };
        println!("🎤 Bluetooth microphone is already {state}");
        return Ok(());
    }

    let profile = card
        .best_profile(mode)
        .ok_or(Error::NoProfile(mode.label()))?;
    backend.set_card_profile(&card.name, profile)?;
    let sink = backend.wait_for_sink(card_index)?;
    let mic = match mode {
        Mode::Headset => Some(backend.wait_for_source(card_index)?),
        Mode::HighFidelity => builtin_mic(&mut backend)?,
    };
    apply(&mut backend, &sink, mic.as_ref())
}

fn mic_status() -> Result<()> {
    let mut backend = Backend::connect()?;
    let (sink, card) = current_bluetooth_card(&mut backend)?;
    let state = if card.mode() == Some(Mode::Headset) {
        "on"
    } else {
        "off"
    };
    println!("🎤 Bluetooth microphone: {state} ({})", sink.description);
    Ok(())
}

fn mic_test() -> Result<()> {
    let mut backend = Backend::connect()?;
    let (sink, card) = current_bluetooth_card(&mut backend)?;
    if card.mode() != Some(Mode::Headset) {
        return Err(Error::MicOff);
    }
    let mic = backend
        .sources()?
        .into_iter()
        .find(|d| d.card == Some(card.index))
        .ok_or(Error::MicOff)?;
    drop(backend);

    println!(
        "🎤 Recording {MIC_TEST_SECONDS}s from {}... speak now",
        mic.description
    );
    tone::record_and_play(
        &mic.name,
        &sink.name,
        MIC_TEST_SECONDS,
        |rms| {
            let filled = (tone::level(rms) * LEVEL_BAR_WIDTH as f32).round() as usize;
            print!(
                "\r   [{}{}]",
                "#".repeat(filled),
                " ".repeat(LEVEL_BAR_WIDTH - filled)
            );
            let _ = io::stdout().flush();
        },
        |samples| {
            println!();
            let peak = tone::dbfs(tone::peak(samples));
            let rms = tone::dbfs(tone::rms(samples));
            println!("   peak {peak:.1} dBFS · rms {rms:.1} dBFS");
            if peak < QUIET_DBFS {
                println!("⚠️  Almost nothing was recorded");
            }
            // Headset microphones record quietly, so play the recording back at full level.
            tone::normalize(samples);
            println!("🔊 Playing back...");
        },
    )
}

fn test(left: bool, right: bool) -> Result<()> {
    let channel = match (left, right) {
        (true, _) => Some(Channel::Left),
        (_, true) => Some(Channel::Right),
        _ => None,
    };
    tone::play_melody(channel, |channel| println!("🔊 {}", channel.label()))
}

/// Makes the given devices the defaults, moves running streams and prints the result.
fn apply(backend: &mut Backend, sink: &Device, mic: Option<&Device>) -> Result<()> {
    backend.set_default_sink(&sink.name)?;
    backend.move_playback(&sink.name)?;
    if let Some(mic) = mic {
        backend.set_default_source(&mic.name)?;
        backend.move_recording(&mic.name)?;
    }

    match mic {
        Some(mic) => println!("🔊 {} · 🎤 {}", sink.description, mic.description),
        None => println!("🔊 {}", sink.description),
    }
    Ok(())
}

fn current_sink(backend: &mut Backend) -> Result<Device> {
    let name = backend.defaults()?.sink.ok_or(Error::NoDevices)?;
    backend
        .sinks()?
        .into_iter()
        .find(|d| d.name == name)
        .ok_or(Error::NoDevices)
}

/// The current output and its card, or an error when the output is not Bluetooth.
fn current_bluetooth_card(backend: &mut Backend) -> Result<(Device, Card)> {
    let current = current_sink(backend)?;
    if current.kind != Kind::Bluetooth {
        return Err(Error::NotBluetooth);
    }
    let index = current.card.ok_or(Error::NotBluetooth)?;
    let card = backend
        .bluetooth_cards()?
        .into_iter()
        .find(|c| c.index == index)
        .ok_or(Error::NotBluetooth)?;
    Ok((current, card))
}

fn builtin_mic(backend: &mut Backend) -> Result<Option<Device>> {
    Ok(device::builtin_mic(&backend.sources()?).cloned())
}
