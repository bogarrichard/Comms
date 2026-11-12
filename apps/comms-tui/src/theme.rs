//! Per-connection colouring: each backend gets an accent colour so you can tell
//! at a glance which service you are looking at. Signal is blue, Teams purple, Slack green-blue-grey.
//! (Teams' own brand indigo, #6264A7, reads as blue next to Signal's, so a
//! clearly violet purple is used instead.)

use ratatui::style::Color;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Theme {
    /// Shown in the sidebar title.
    pub name: &'static str,
    /// Focused borders, the mode badge, your own name, the open conversation.
    pub accent: Color,
    /// Second colour: selection, thread markers, the active rail item.
    pub secondary: Color,
    /// Timestamps, unfocused borders, hints.
    pub muted: Color,
    /// Colours cycled through for other people's names.
    pub names: &'static [Color],
    /// Workspace layout: a left rail (Home / DMs / Activity) and a thread pane,
    /// like the Slack app.
    pub workspace_ui: bool,
}

const PLAIN_NAMES: &[Color] = &[
    Color::Green,
    Color::Magenta,
    Color::Yellow,
    Color::Blue,
    Color::LightRed,
    Color::LightCyan,
];

/// Greens, blues and greys only.
const SLACK_NAMES: &[Color] = &[
    Color::Rgb(0x2E, 0xB6, 0x7D),
    Color::Rgb(0x36, 0xA3, 0xD9),
    Color::Rgb(0x7F, 0xD6, 0xA9),
    Color::Rgb(0x8C, 0xCB, 0xEE),
    Color::Rgb(0x1F, 0xA6, 0xA0),
    Color::Rgb(0xB0, 0xB5, 0xBD),
];

impl Theme {
    /// Signal's brand blue (#3A76F0).
    pub const SIGNAL: Theme = Theme {
        name: "Signal",
        accent: Color::Rgb(0x3A, 0x76, 0xF0),
        secondary: Color::Rgb(0x3A, 0x76, 0xF0),
        muted: Color::DarkGray,
        names: PLAIN_NAMES,
        workspace_ui: false,
    };
    /// Purple (#7F5FD0).
    pub const TEAMS: Theme = Theme {
        name: "Teams",
        accent: Color::Rgb(0x7F, 0x5F, 0xD0),
        secondary: Color::Rgb(0x7F, 0x5F, 0xD0),
        muted: Color::DarkGray,
        names: PLAIN_NAMES,
        workspace_ui: false,
    };
    /// Slack: green, blue and grey, with the workspace layout.
    pub const SLACK: Theme = Theme {
        name: "Slack",
        accent: Color::Rgb(0x2E, 0xB6, 0x7D),
        secondary: Color::Rgb(0x36, 0xA3, 0xD9),
        muted: Color::Rgb(0x8D, 0x93, 0x9C),
        names: SLACK_NAMES,
        workspace_ui: true,
    };
    /// Anything we have no brand colour for.
    pub const DEFAULT: Theme = Theme {
        name: "Comms",
        accent: Color::Cyan,
        secondary: Color::Cyan,
        muted: Color::DarkGray,
        names: PLAIN_NAMES,
        workspace_ui: false,
    };

    /// `kind` is `ProviderKind::as_str()`. The demo backend ("fake") imitates Teams.
    pub fn for_provider(kind: &str) -> Theme {
        match kind {
            "signal" => Self::SIGNAL,
            "slack" => Self::SLACK,
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
        assert_eq!(Theme::for_provider("slack"), Theme::SLACK);
        assert_eq!(Theme::for_provider("msgraph"), Theme::TEAMS);
        assert_eq!(Theme::for_provider("fake"), Theme::TEAMS);
        assert_eq!(Theme::for_provider("something-new"), Theme::DEFAULT);
        assert_ne!(Theme::SIGNAL.accent, Theme::TEAMS.accent);
        assert_ne!(Theme::SLACK.accent, Theme::TEAMS.accent);
        assert_ne!(Theme::SLACK.accent, Theme::SIGNAL.accent);
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

    #[test]
    fn slack_uses_only_greens_blues_and_greys_and_the_workspace_layout() {
        const { assert!(Theme::SLACK.workspace_ui) };
        const { assert!(!Theme::TEAMS.workspace_ui && !Theme::SIGNAL.workspace_ui) };
        let ok = |c: Color| {
            let Color::Rgb(r, g, b) = c else {
                panic!("rgb")
            };
            // Nothing red-dominant or yellow/orange/purple.
            g >= r && b >= r.saturating_sub(0x20) || r.abs_diff(g) < 0x18 && g.abs_diff(b) < 0x18
        };
        for c in Theme::SLACK.names.iter().chain([
            &Theme::SLACK.accent,
            &Theme::SLACK.secondary,
            &Theme::SLACK.muted,
        ]) {
            assert!(ok(*c), "{c:?} is not green, blue or grey");
        }
    }
}
