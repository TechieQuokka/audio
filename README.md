# audio

A tiny CLI to switch audio output devices on Linux (PulseAudio / PipeWire via `pipewire-pulse`).

Switching outputs from the desktop settings is tedious, especially with Bluetooth headsets:
connecting one does not always move the sound, and changing the default output leaves already
playing streams behind. `audio` does it in one command and moves everything that is playing.

## Features

- **Numbered device list** — pick an output by number, no long device names.
- **Moves playing streams** — music, browser and calls follow the new output immediately.
- **Bluetooth microphone on demand** — keep high fidelity A2DP by default, switch to the headset profile only when you need the mic.
- **Test sound** — the opening of Beethoven's *Für Elise*, split between left and right, to check channels at a glance.
- **Microphone test** — record a few seconds and play them back, with a live level meter.
- **Native** — talks to the audio server through `libpulse`, no shelling out to `pactl`.

## Usage

```
audio list            # list outputs (* = current)
audio <number>        # switch output and move playing streams
audio mic on          # enable the Bluetooth microphone (headset profile)
audio mic off         # back to high fidelity Bluetooth playback (A2DP)
audio mic status      # show whether the Bluetooth microphone is on
audio mic test        # record 3s from the Bluetooth microphone and play it back
audio test            # "Für Elise" opening: first phrase left, second phrase right
audio test --left     # whole melody on the left channel
audio test --right    # whole melody on the right channel
```

Running `audio` with no arguments is the same as `audio list`.

### Example

```
$ audio list
   1  Built-in Audio Analog Stereo                                  speaker
   2  GA106 High Definition Audio Controller Digital Stereo (HDMI)  hdmi
*  3  AirPods Pro                                                   bluetooth

$ audio 1
🔊 Built-in Audio Analog Stereo

$ audio 3
🔊 AirPods Pro

$ audio mic on
🔊 AirPods Pro · 🎤 AirPods Pro

$ audio mic status
🎤 Bluetooth microphone: on (AirPods Pro)

$ audio mic off
🔊 AirPods Pro
```

## Behavior

- Device numbers are assigned by sorting device names, not by server IDs, so they stay stable between runs.
  Connecting or disconnecting a device can still shift the numbers.
- Switching output moves every playing stream to the new device and sets the input to the built-in microphone, if one is plugged in.
- Switching output always returns Bluetooth devices to A2DP, so a device is never left in call quality.
- `audio mic on|off|status|test` fails with an error when the current output is not a Bluetooth device.
- `audio mic test` fails with an error when the Bluetooth microphone is off.
- `audio mic test` keeps the headset output open (playing silence) while it records, prints the recorded
  peak and RMS level, and plays the recording back normalized to full level.

### Why is the Bluetooth microphone optional?

Bluetooth cannot carry high quality playback and a microphone at the same time.

| Profile | Playback | Microphone | Quality |
|---|---|---|---|
| A2DP (default) | yes | no | high (music) |
| HSP/HFP (`audio mic on`) | yes | yes | low (calls) |

## Requirements

- Linux with PulseAudio, or PipeWire with `pipewire-pulse`
- The PulseAudio client library and headers

```
sudo apt install libpulse-dev pkg-config   # Debian / Ubuntu
```

## Install

```
git clone https://github.com/TechieQuokka/audio.git
cd audio
cargo install --path .
```

## Development

```
cargo test
cargo clippy --all-targets
```

| File | Role |
|---|---|
| `src/cli.rs` | Command line definition (clap) |
| `src/commands.rs` | Command logic |
| `src/backend.rs` | libpulse wrapper: devices, defaults, streams, Bluetooth profiles |
| `src/device.rs` | Device classification, numbering and profile selection |
| `src/tone.rs` | Test melody synthesis, recording and playback |
| `src/error.rs` | Error types |

Versioning: new features bump the minor version, bug fixes bump the patch version.

## Known issues

- On some USB Bluetooth adapters (seen with a Realtek RTL8761BU), the headset's SCO link drops a few seconds
  into a recording when nothing is playing to the headset, and the kernel logs `corrupted SCO packet`.
  Everything recorded after that is silence. `audio mic test` works around this by playing silence while it
  records; other record-only tools such as `pw-record` are still affected. Calls are fine, since they play and
  record at the same time.
