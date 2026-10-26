use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    #[error("failed to connect to the audio server")]
    Connect,
    #[error("audio server operation failed: {0}")]
    Operation(&'static str),
    #[error("no output devices found")]
    NoDevices,
    #[error("invalid device number {number} (expected 1..={max}, see `audio list`)")]
    InvalidNumber { number: usize, max: usize },
    #[error("current output is not a Bluetooth device")]
    NotBluetooth,
    #[error("Bluetooth microphone is off (run `audio mic on` first)")]
    MicOff,
    #[error("no suitable {0} profile on the Bluetooth device")]
    NoProfile(&'static str),
    #[error("timed out waiting for the device after a profile change")]
    Timeout,
    #[error(transparent)]
    Pulse(#[from] libpulse_binding::error::PAErr),
}
