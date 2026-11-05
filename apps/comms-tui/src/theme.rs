//! Per-connection colouring: each backend gets an accent colour so you can tell
//! at a glance which service you are looking at. Signal is blue, Teams purple.
//! (Teams' own brand indigo, #6264A7, reads as blue next to Signal's, so a
//! clearly violet purple is used instead.)

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Shown in the sidebar title.
    pub name: &'static str,
    /// Focused borders, the mode badge, your own name, the open conversation.
    pub accent: Color,
}

impl Theme {
    /// Signal's brand blue (#3A76F0).
    pub const SIGNAL: Theme = Theme {
        name: "Signal",
        accent: Color::Rgb(0x3A, 0x76, 0xF0),
    };
    /// Purple (#7F5FD0).
    pub const TEAMS: Theme = Theme {
        name: "Teams",
        accent: Color::Rgb(0x7F, 0x5F, 0xD0),
    };
    /// Anything we have no brand colour for.
    pub const DEFAULT: Theme = Theme {
        name: "Comms",
        accent: Color::Cyan,
    };

    /// `kind` is `ProviderKind::as_str()`. The demo backend ("fake") imitates Teams.
    pub fn for_provider(kind: &str) -> Theme {
        match kind {
            "signal" => Self::SIGNAL,
            "msgraph" | "fake" => Self::TEAMS,
            _ => Self::DEFAULT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_backend_has_its_own_accent() {
        assert_eq!(Theme::for_provider("signal"), Theme::SIGNAL);
        assert_eq!(Theme::for_provider("msgraph"), Theme::TEAMS);
        assert_eq!(Theme::for_provider("fake"), Theme::TEAMS);
        assert_eq!(Theme::for_provider("something-new"), Theme::DEFAULT);
        assert_ne!(Theme::SIGNAL.accent, Theme::TEAMS.accent);
    }

    #[test]
    fn signal_is_blue_and_teams_is_purple() {
        let Color::Rgb(r, g, b) = Theme::SIGNAL.accent else {
            panic!("rgb")
        };
        assert!(b > r + 80 && b > g + 80, "blue dominates: {r},{g},{b}");
        let Color::Rgb(r, g, b) = Theme::TEAMS.accent else {
            panic!("rgb")
        };
        assert!(
            b > g && r > g,
            "purple = red and blue over green: {r},{g},{b}"
        );
    }
}
