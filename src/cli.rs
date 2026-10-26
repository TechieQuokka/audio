use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "audio",
    version,
    about = "Switch audio output devices",
    args_conflicts_with_subcommands = true
)]
pub struct Cli {
    /// Output device number shown by `audio list`
    pub number: Option<usize>,

    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum Command {
    /// List output devices (* marks the current one)
    List,
    /// Control the Bluetooth microphone
    Mic {
        #[command(subcommand)]
        action: MicAction,
    },
    /// Play the opening of "Für Elise" (first phrase left, second phrase right)
    Test {
        /// Play on the left channel only
        #[arg(long, conflicts_with = "right")]
        left: bool,
        /// Play on the right channel only
        #[arg(long)]
        right: bool,
    },
}

#[derive(Debug, PartialEq, Eq, Subcommand)]
pub enum MicAction {
    /// Enable the Bluetooth microphone (headset profile, lower audio quality)
    On,
    /// Disable the Bluetooth microphone (back to high fidelity playback)
    Off,
    /// Show whether the Bluetooth microphone is on
    Status,
    /// Record a few seconds from the Bluetooth microphone and play it back (requires `mic on`)
    Test,
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    fn parse(args: &[&str]) -> Cli {
        Cli::try_parse_from(args).expect("arguments should parse")
    }

    #[test]
    fn verify_cli_definition() {
        Cli::command().debug_assert();
    }

    #[test]
    fn no_arguments() {
        let cli = parse(&["audio"]);
        assert_eq!(cli.number, None);
        assert_eq!(cli.command, None);
    }

    #[test]
    fn number_switches_output() {
        let cli = parse(&["audio", "2"]);
        assert_eq!(cli.number, Some(2));
        assert_eq!(cli.command, None);
    }

    #[test]
    fn list_subcommand() {
        assert_eq!(parse(&["audio", "list"]).command, Some(Command::List));
    }

    #[test]
    fn mic_subcommands() {
        for (arg, action) in [
            ("on", MicAction::On),
            ("off", MicAction::Off),
            ("status", MicAction::Status),
            ("test", MicAction::Test),
        ] {
            assert_eq!(
                parse(&["audio", "mic", arg]).command,
                Some(Command::Mic { action })
            );
        }
    }

    #[test]
    fn test_channel_flags() {
        assert_eq!(
            parse(&["audio", "test", "--left"]).command,
            Some(Command::Test {
                left: true,
                right: false
            })
        );
        assert!(Cli::try_parse_from(["audio", "test", "--left", "--right"]).is_err());
    }

    #[test]
    fn number_conflicts_with_subcommand() {
        assert!(Cli::try_parse_from(["audio", "1", "list"]).is_err());
    }

    #[test]
    fn rejects_non_numeric_device() {
        assert!(Cli::try_parse_from(["audio", "abc"]).is_err());
    }
}
