//! Configurable key bindings for normal mode.

use std::collections::BTreeMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    Quit,
    Down,
    Up,
    Top,
    Bottom,
    PageDown,
    PageUp,
    Open,
    Back,
    Compose,
    Search,
    Help,
    NextFilter,
    Refresh,
    LoadOlder,
    Retry,
    Thread,
    NextThread,
    PrevThread,
    FocusLeft,
    FocusRight,
}

impl Action {
    pub const ALL: [Action; 21] = [
        Action::Quit,
        Action::Down,
        Action::Up,
        Action::Top,
        Action::Bottom,
        Action::PageDown,
        Action::PageUp,
        Action::Open,
        Action::Back,
        Action::Compose,
        Action::Search,
        Action::Help,
        Action::NextFilter,
        Action::Refresh,
        Action::LoadOlder,
        Action::Retry,
        Action::Thread,
        Action::NextThread,
        Action::PrevThread,
        Action::FocusLeft,
        Action::FocusRight,
    ];

    /// Name used in `config.toml` under `[keys]`.
    pub fn name(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Down => "down",
            Action::Up => "up",
            Action::Top => "top",
            Action::Bottom => "bottom",
            Action::PageDown => "page_down",
            Action::PageUp => "page_up",
            Action::Open => "open",
            Action::Back => "back",
            Action::Compose => "compose",
            Action::Search => "search",
            Action::Help => "help",
            Action::NextFilter => "next_filter",
            Action::Refresh => "refresh",
            Action::LoadOlder => "load_older",
            Action::Retry => "retry",
            Action::Thread => "thread",
            Action::NextThread => "next_thread",
            Action::PrevThread => "prev_thread",
            Action::FocusLeft => "focus_left",
            Action::FocusRight => "focus_right",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Action::Quit => "quit",
            Action::Down => "move down / scroll down",
            Action::Up => "move up / scroll up",
            Action::Top => "jump to top",
            Action::Bottom => "jump to bottom",
            Action::PageDown => "half page down",
            Action::PageUp => "half page up",
            Action::Open => "open conversation",
            Action::Back => "back to conversation list",
            Action::Compose => "write a message",
            Action::Search => "filter conversations",
            Action::Help => "show / hide this help",
            Action::NextFilter => "cycle All / Chats / Channels",
            Action::Refresh => "refresh now",
            Action::LoadOlder => "load older messages",
            Action::Retry => "resend last failed message",
            Action::Thread => "open / close the thread pane",
            Action::NextThread => "next thread",
            Action::PrevThread => "previous thread",
            Action::FocusLeft => "move to the panel on the left",
            Action::FocusRight => "move to the panel on the right",
        }
    }

    fn defaults(self) -> &'static [&'static str] {
        match self {
            Action::Quit => &["q", "ctrl+c"],
            Action::Down => &["j", "down"],
            Action::Up => &["k", "up"],
            Action::Top => &["g", "home"],
            Action::Bottom => &["G", "end"],
            Action::PageDown => &["ctrl+d", "pagedown"],
            Action::PageUp => &["ctrl+u", "pageup"],
            Action::Open => &["enter", "l"],
            Action::Back => &["esc", "h"],
            Action::Compose => &["i"],
            Action::Search => &["/"],
            Action::Help => &["?", "f1"],
            Action::NextFilter => &["tab"],
            Action::Refresh => &["r", "ctrl+r"],
            Action::LoadOlder => &["o"],
            Action::Retry => &["R"],
            Action::Thread => &["t"],
            Action::NextThread => &["]"],
            Action::PrevThread => &["["],
            Action::FocusLeft => &["ctrl+h"],
            Action::FocusRight => &["ctrl+l"],
        }
    }

    fn from_name(name: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.name() == name)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Binding {
    code: KeyCode,
    mods: KeyModifiers,
}

/// Plain characters are matched without their SHIFT bit: terminals disagree
/// on reporting it for `G` or `?`.
fn normalise(code: KeyCode, mods: KeyModifiers) -> Binding {
    let mods = if matches!(code, KeyCode::Char(_)) {
        mods - KeyModifiers::SHIFT
    } else {
        mods
    };
    Binding { code, mods }
}

fn parse_binding(spec: &str) -> Result<Binding, String> {
    let lower = spec.to_lowercase();
    let mut mods = KeyModifiers::NONE;
    let mut rest = spec;
    // Modifier prefixes are case-insensitive; the key itself keeps its case.
    loop {
        let l = rest.to_lowercase();
        if let Some(r) = l.strip_prefix("ctrl+") {
            mods |= KeyModifiers::CONTROL;
            rest = &rest[rest.len() - r.len()..];
        } else if let Some(r) = l.strip_prefix("alt+") {
            mods |= KeyModifiers::ALT;
            rest = &rest[rest.len() - r.len()..];
        } else {
            break;
        }
    }
    let code = match rest.to_lowercase().as_str() {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backspace" => KeyCode::Backspace,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "space" => KeyCode::Char(' '),
        f if f.len() > 1 && f.starts_with('f') && f[1..].parse::<u8>().is_ok() => {
            KeyCode::F(f[1..].parse().map_err(|_| format!("bad key `{spec}`"))?)
        }
        _ => {
            let mut chars = rest.chars();
            match (chars.next(), chars.next()) {
                (Some(c), None) => {
                    // ctrl+X is conventionally lower-case.
                    KeyCode::Char(if mods.contains(KeyModifiers::CONTROL) {
                        c.to_ascii_lowercase()
                    } else {
                        c
                    })
                }
                _ => return Err(format!("unknown key `{spec}`")),
            }
        }
    };
    let _ = lower;
    Ok(normalise(code, mods))
}

#[derive(Debug, Clone)]
pub struct KeyMap {
    bindings: Vec<(Binding, Action)>,
    labels: BTreeMap<Action, Vec<String>>,
}

impl KeyMap {
    /// Defaults, with any action listed in `overrides` fully replaced.
    pub fn new(overrides: &BTreeMap<String, Vec<String>>) -> Result<Self, String> {
        for name in overrides.keys() {
            if Action::from_name(name).is_none() {
                let valid: Vec<_> = Action::ALL.iter().map(|a| a.name()).collect();
                return Err(format!(
                    "unknown key action `{name}` (valid: {})",
                    valid.join(", ")
                ));
            }
        }
        let mut bindings = Vec::new();
        let mut labels = BTreeMap::new();
        for action in Action::ALL {
            let specs: Vec<String> = match overrides.get(action.name()) {
                Some(v) => v.clone(),
                None => action.defaults().iter().map(|s| (*s).to_owned()).collect(),
            };
            for spec in &specs {
                let b =
                    parse_binding(spec).map_err(|e| format!("[keys] {}: {e}", action.name()))?;
                bindings.push((b, action));
            }
            labels.insert(action, specs);
        }
        Ok(Self { bindings, labels })
    }

    #[cfg(test)]
    pub fn default_map() -> Self {
        Self::new(&BTreeMap::new()).expect("built-in key bindings are valid")
    }

    pub fn lookup(&self, key: KeyEvent) -> Option<Action> {
        let b = normalise(key.code, key.modifiers);
        self.bindings.iter().find(|(x, _)| *x == b).map(|(_, a)| *a)
    }

    pub fn labels(&self, action: Action) -> String {
        self.labels
            .get(&action)
            .map(|v| v.join(" / "))
            .unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn defaults_cover_the_suggested_vim_style_bindings() {
        let m = KeyMap::default_map();
        assert_eq!(
            m.lookup(key(KeyCode::Char('j'), KeyModifiers::NONE)),
            Some(Action::Down)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(Action::Up)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Enter, KeyModifiers::NONE)),
            Some(Action::Open)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('/'), KeyModifiers::NONE)),
            Some(Action::Search)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('i'), KeyModifiers::NONE)),
            Some(Action::Compose)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Esc, KeyModifiers::NONE)),
            Some(Action::Back)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('q'), KeyModifiers::NONE)),
            Some(Action::Quit)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('c'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
    }

    #[test]
    fn shift_on_characters_is_ignored() {
        let m = KeyMap::default_map();
        assert_eq!(
            m.lookup(key(KeyCode::Char('G'), KeyModifiers::SHIFT)),
            Some(Action::Bottom)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('G'), KeyModifiers::NONE)),
            Some(Action::Bottom)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('?'), KeyModifiers::SHIFT)),
            Some(Action::Help)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('g'), KeyModifiers::NONE)),
            Some(Action::Top)
        );
    }

    #[test]
    fn overrides_replace_the_defaults_of_that_action_only() {
        let mut o = BTreeMap::new();
        o.insert("quit".to_owned(), vec!["ctrl+q".to_owned()]);
        o.insert(
            "down".to_owned(),
            vec!["n".to_owned(), "Ctrl+Down".to_owned()],
        );
        let m = KeyMap::new(&o).unwrap();
        assert_eq!(m.lookup(key(KeyCode::Char('q'), KeyModifiers::NONE)), None);
        assert_eq!(
            m.lookup(key(KeyCode::Char('q'), KeyModifiers::CONTROL)),
            Some(Action::Quit)
        );
        assert_eq!(
            m.lookup(key(KeyCode::Char('n'), KeyModifiers::NONE)),
            Some(Action::Down)
        );
        assert_eq!(m.lookup(key(KeyCode::Char('j'), KeyModifiers::NONE)), None);
        assert_eq!(
            m.lookup(key(KeyCode::Char('k'), KeyModifiers::NONE)),
            Some(Action::Up)
        );
        assert_eq!(m.labels(Action::Down), "n / Ctrl+Down");
    }

    #[test]
    fn invalid_configuration_is_reported_clearly() {
        let bad_action = BTreeMap::from([("quitt".to_owned(), vec!["q".to_owned()])]);
        assert!(KeyMap::new(&bad_action).unwrap_err().contains("quitt"));
        let bad_key = BTreeMap::from([("quit".to_owned(), vec!["ctrl+nonsense".to_owned()])]);
        let err = KeyMap::new(&bad_key).unwrap_err();
        assert!(err.contains("quit") && err.contains("nonsense"), "{err}");
    }

    #[test]
    fn special_keys_parse() {
        for s in [
            "enter",
            "esc",
            "tab",
            "pageup",
            "f1",
            "space",
            "alt+x",
            "ctrl+Enter",
        ] {
            assert!(parse_binding(s).is_ok(), "{s}");
        }
    }
}
