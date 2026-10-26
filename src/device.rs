use crate::error::{Error, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Speaker,
    Hdmi,
    Bluetooth,
}

impl Kind {
    pub fn from_name(name: &str) -> Self {
        if name.starts_with("bluez_") {
            Kind::Bluetooth
        } else if name.contains("hdmi") {
            Kind::Hdmi
        } else {
            Kind::Speaker
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Speaker => "speaker",
            Kind::Hdmi => "hdmi",
            Kind::Bluetooth => "bluetooth",
        }
    }
}

/// A sink (output) or source (input) reported by the audio server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub index: u32,
    pub name: String,
    pub description: String,
    pub card: Option<u32>,
    pub kind: Kind,
    /// False when every port reports its jack as unplugged.
    pub available: bool,
}

impl Device {
    pub fn new(
        index: u32,
        name: &str,
        description: &str,
        card: Option<u32>,
        available: bool,
    ) -> Self {
        Self {
            index,
            name: name.to_owned(),
            description: description.to_owned(),
            card,
            kind: Kind::from_name(name),
            available,
        }
    }
}

/// Sorts devices by name so the numbers shown by `audio list` stay stable.
pub fn sort(devices: &mut [Device]) {
    devices.sort_by(|a, b| a.name.cmp(&b.name));
}

/// Picks a device by its 1-based number from a sorted list.
pub fn select(devices: &[Device], number: usize) -> Result<&Device> {
    if devices.is_empty() {
        return Err(Error::NoDevices);
    }
    number
        .checked_sub(1)
        .and_then(|i| devices.get(i))
        .ok_or(Error::InvalidNumber {
            number,
            max: devices.len(),
        })
}

/// The first plugged-in non-Bluetooth input, used whenever the Bluetooth microphone is off.
/// Unplugged inputs are skipped because the server would ignore them as the default.
pub fn builtin_mic(sources: &[Device]) -> Option<&Device> {
    sources
        .iter()
        .filter(|d| d.kind != Kind::Bluetooth && d.available)
        .min_by(|a, b| a.name.cmp(&b.name))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// A2DP: high quality playback, no microphone.
    HighFidelity,
    /// HSP/HFP: playback and microphone, call quality.
    Headset,
}

impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Mode::HighFidelity => "A2DP",
            Mode::Headset => "headset",
        }
    }

    fn matches(self, profile: &str) -> bool {
        match self {
            Mode::HighFidelity => profile.starts_with("a2dp"),
            Mode::Headset => profile.starts_with("headset"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Profile {
    pub name: String,
    pub priority: u32,
    pub available: bool,
}

/// A Bluetooth card and its profiles.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub index: u32,
    pub name: String,
    pub active_profile: Option<String>,
    pub profiles: Vec<Profile>,
}

impl Card {
    pub fn mode(&self) -> Option<Mode> {
        let active = self.active_profile.as_deref()?;
        [Mode::HighFidelity, Mode::Headset]
            .into_iter()
            .find(|mode| mode.matches(active))
    }

    pub fn best_profile(&self, mode: Mode) -> Option<&str> {
        let candidates = self
            .profiles
            .iter()
            .filter(|p| p.available && mode.matches(&p.name));
        // The generic headset profile lets the server negotiate the best codec (mSBC over CVSD).
        if mode == Mode::Headset
            && let Some(generic) = candidates.clone().find(|p| p.name == "headset-head-unit")
        {
            return Some(&generic.name);
        }
        candidates
            .max_by_key(|p| p.priority)
            .map(|p| p.name.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(index: u32, name: &str) -> Device {
        Device::new(index, name, name, None, true)
    }

    fn profile(name: &str, priority: u32) -> Profile {
        Profile {
            name: name.to_owned(),
            priority,
            available: true,
        }
    }

    fn airpods(active: &str) -> Card {
        Card {
            index: 73,
            name: "bluez_card.00_11_22_33_44_55".to_owned(),
            active_profile: Some(active.to_owned()),
            profiles: vec![
                profile("off", 0),
                profile("headset-head-unit", 1),
                profile("a2dp-sink", 18),
                profile("a2dp-sink-sbc_xq", 17),
                profile("headset-head-unit-cvsd", 2),
            ],
        }
    }

    #[test]
    fn classifies_devices_by_name() {
        let cases = [
            ("alsa_output.pci-0000_00_1f.3.analog-stereo", Kind::Speaker),
            ("alsa_output.pci-0000_01_00.1.hdmi-stereo", Kind::Hdmi),
            ("bluez_output.00_11_22_33_44_55.1", Kind::Bluetooth),
            ("bluez_input.00_11_22_33_44_55.0", Kind::Bluetooth),
        ];
        for (name, kind) in cases {
            assert_eq!(Kind::from_name(name), kind, "{name}");
        }
    }

    #[test]
    fn sorts_by_name() {
        let mut devices = vec![
            device(97, "bluez_output.00_11_22_33_44_55.1"),
            device(58, "alsa_output.pci-0000_01_00.1.hdmi-stereo"),
            device(48, "alsa_output.pci-0000_00_1f.3.analog-stereo"),
        ];
        sort(&mut devices);
        let order: Vec<u32> = devices.iter().map(|d| d.index).collect();
        assert_eq!(order, [48, 58, 97]);
    }

    #[test]
    fn selects_by_one_based_number() {
        let devices = [device(1, "a"), device(2, "b")];
        assert_eq!(select(&devices, 1).unwrap().name, "a");
        assert_eq!(select(&devices, 2).unwrap().name, "b");
    }

    #[test]
    fn rejects_out_of_range_numbers() {
        let devices = [device(1, "a"), device(2, "b")];
        for number in [0, 3] {
            assert!(matches!(
                select(&devices, number),
                Err(Error::InvalidNumber { max: 2, .. })
            ));
        }
        assert!(matches!(select(&[], 1), Err(Error::NoDevices)));
    }

    #[test]
    fn builtin_mic_skips_bluetooth() {
        let sources = [
            device(1, "bluez_input.00_11_22_33_44_55.0"),
            device(49, "alsa_input.pci-0000_00_1f.3.analog-stereo"),
        ];
        assert_eq!(builtin_mic(&sources).unwrap().index, 49);
        assert!(builtin_mic(&sources[..1]).is_none());
    }

    #[test]
    fn builtin_mic_skips_unplugged_inputs() {
        let mut unplugged = device(49, "alsa_input.pci-0000_00_1f.3.analog-stereo");
        unplugged.available = false;
        let usb = device(60, "alsa_input.usb-mic.mono");
        assert!(builtin_mic(std::slice::from_ref(&unplugged)).is_none());
        assert_eq!(builtin_mic(&[unplugged, usb]).unwrap().index, 60);
    }

    #[test]
    fn detects_card_mode() {
        assert_eq!(airpods("a2dp-sink").mode(), Some(Mode::HighFidelity));
        assert_eq!(
            airpods("headset-head-unit-cvsd").mode(),
            Some(Mode::Headset)
        );
        assert_eq!(airpods("off").mode(), None);
    }

    #[test]
    fn picks_highest_priority_a2dp_profile() {
        assert_eq!(
            airpods("off").best_profile(Mode::HighFidelity),
            Some("a2dp-sink")
        );
    }

    #[test]
    fn prefers_generic_headset_profile() {
        assert_eq!(
            airpods("off").best_profile(Mode::Headset),
            Some("headset-head-unit")
        );
    }

    #[test]
    fn falls_back_to_priority_for_headset_profile() {
        let mut card = airpods("off");
        card.profiles.retain(|p| p.name != "headset-head-unit");
        assert_eq!(
            card.best_profile(Mode::Headset),
            Some("headset-head-unit-cvsd")
        );
    }

    #[test]
    fn ignores_unavailable_profiles() {
        let mut card = airpods("off");
        for p in &mut card.profiles {
            p.available = !p.name.starts_with("a2dp");
        }
        assert_eq!(card.best_profile(Mode::HighFidelity), None);
    }
}
