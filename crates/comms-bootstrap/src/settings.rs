use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use comms_provider_msgraph::scopes;
use serde::Deserialize;

/// How much of Teams channels to ask Microsoft for.
/// Which messaging service to connect to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    #[default]
    Teams,
    Signal,
}

/// The `[signal]` table.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct SignalSettings {
    /// Your Signal phone number in international format, e.g. `+36201234567`.
    pub account: Option<String>,
    /// The `signal-cli` executable (name on PATH, or a full path).
    pub cli_path: String,
}

impl Default for SignalSettings {
    fn default() -> Self {
        Self {
            account: None,
            cli_path: "signal-cli".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChannelAccess {
    /// Chats only.
    Off,
    /// List teams/channels and post to them. Reading needs `read`.
    Basic,
    /// Also read channel messages (**needs administrator consent** in most tenants).
    Read,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct PollSettings {
    pub chats_secs: u64,
    pub channels_secs: u64,
    pub open_secs: u64,
}

impl Default for PollSettings {
    fn default() -> Self {
        Self {
            chats_secs: 60,
            channels_secs: 300,
            open_secs: 15,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    /// `teams` (default) or `signal`.
    pub provider: Backend,
    pub signal: SignalSettings,
    /// Application (client) ID of your Entra app registration.
    pub client_id: Option<String>,
    /// `organizations`, `common` or a tenant ID/domain.
    pub tenant: String,
    pub channels: ChannelAccess,
    /// Messages fetched the first time a conversation is opened.
    pub initial_messages: u32,
    pub poll: PollSettings,
    /// Key overrides: action name → list of key strings (see docs).
    pub keys: BTreeMap<String, Vec<String>>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            provider: Backend::default(),
            signal: SignalSettings::default(),
            client_id: None,
            tenant: "organizations".to_owned(),
            channels: ChannelAccess::Read,
            initial_messages: 100,
            poll: PollSettings::default(),
            keys: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub database: PathBuf,
    pub log_file: PathBuf,
    /// Signal messages received so far (signal-cli keeps no history).
    pub signal_inbox: PathBuf,
}

impl Paths {
    /// Platform locations; `COMMS_CONFIG_DIR` / `COMMS_DATA_DIR` override them.
    pub fn discover() -> Result<Self> {
        let dirs = directories::ProjectDirs::from("", "", "comms")
            .context("cannot determine the user's home directory")?;
        let config_dir = std::env::var_os("COMMS_CONFIG_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs.config_dir().to_path_buf());
        let data_dir = std::env::var_os("COMMS_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| dirs.data_dir().to_path_buf());
        Ok(Self {
            config_file: config_dir.join("config.toml"),
            database: data_dir.join("comms.db"),
            log_file: data_dir.join("comms.log"),
            signal_inbox: data_dir.join("signal-inbox.jsonl"),
        })
    }
}

impl Settings {
    /// Load `config.toml` (missing file = defaults), then apply environment overrides.
    pub fn load() -> Result<Self> {
        let paths = Paths::discover()?;
        let mut s = Self::from_file(&paths.config_file)?;
        s.apply_env(|k| std::env::var(k).ok());
        Ok(s)
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text)
                .with_context(|| format!("invalid configuration in {}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        Ok(toml::from_str(text)?)
    }

    pub fn apply_env(&mut self, get: impl Fn(&str) -> Option<String>) {
        match get("COMMS_PROVIDER")
            .map(|v| v.trim().to_lowercase())
            .as_deref()
        {
            Some("teams") => self.provider = Backend::Teams,
            Some("signal") => self.provider = Backend::Signal,
            _ => {}
        }
        if let Some(v) = get("COMMS_SIGNAL_ACCOUNT").filter(|v| !v.trim().is_empty()) {
            self.signal.account = Some(v.trim().to_owned());
        }
        if let Some(v) = get("COMMS_CLIENT_ID").filter(|v| !v.trim().is_empty()) {
            self.client_id = Some(v.trim().to_owned());
        }
        if let Some(v) = get("COMMS_TENANT").filter(|v| !v.trim().is_empty()) {
            self.tenant = v.trim().to_owned();
        }
        if let Some(v) = get("COMMS_CHANNELS") {
            match v.trim().to_lowercase().as_str() {
                "off" => self.channels = ChannelAccess::Off,
                "basic" => self.channels = ChannelAccess::Basic,
                "read" => self.channels = ChannelAccess::Read,
                _ => {}
            }
        }
    }

    /// Delegated permissions to request, least privilege for the chosen features.
    pub fn scopes(&self) -> Vec<String> {
        let mut v: Vec<&str> = scopes::BASE.to_vec();
        if matches!(self.channels, ChannelAccess::Basic | ChannelAccess::Read) {
            v.extend(scopes::CHANNELS_BASIC);
        }
        if self.channels == ChannelAccess::Read {
            v.extend(scopes::CHANNELS_READ);
        }
        v.into_iter().map(str::to_owned).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_sensible() {
        let s = Settings::parse("").unwrap();
        assert_eq!(s.tenant, "organizations");
        assert_eq!(s.channels, ChannelAccess::Read);
        assert_eq!(s.initial_messages, 100);
        assert!(s.client_id.is_none());
    }

    #[test]
    fn full_file_parses() {
        let s = Settings::parse(
            r#"
            client_id = "abc"
            tenant = "bme.hu"
            channels = "basic"
            initial_messages = 50
            [poll]
            open_secs = 20
            [keys]
            quit = ["q", "ctrl+c"]
            "#,
        )
        .unwrap();
        assert_eq!(s.client_id.as_deref(), Some("abc"));
        assert_eq!(s.channels, ChannelAccess::Basic);
        assert_eq!(s.poll.open_secs, 20);
        assert_eq!(s.poll.chats_secs, 60);
        assert_eq!(s.keys["quit"], ["q", "ctrl+c"]);
    }

    #[test]
    fn provider_defaults_to_teams_and_signal_table_parses() {
        assert_eq!(Settings::parse("").unwrap().provider, Backend::Teams);
        let s = Settings::parse(
            "provider = \"signal\"\n[signal]\naccount = \"+36201234567\"\ncli_path = \"/opt/signal-cli\"",
        )
        .unwrap();
        assert_eq!(s.provider, Backend::Signal);
        assert_eq!(s.signal.account.as_deref(), Some("+36201234567"));
        assert_eq!(s.signal.cli_path, "/opt/signal-cli");
        assert_eq!(Settings::parse("").unwrap().signal.cli_path, "signal-cli");
        assert!(Settings::parse("provider = \"irc\"").is_err());
        assert!(Settings::parse("[signal]\nacount = \"x\"").is_err());
    }

    #[test]
    fn environment_can_select_signal() {
        let mut s = Settings::default();
        s.apply_env(|k| match k {
            "COMMS_PROVIDER" => Some(" Signal ".into()),
            "COMMS_SIGNAL_ACCOUNT" => Some("+3611".into()),
            _ => None,
        });
        assert_eq!(s.provider, Backend::Signal);
        assert_eq!(s.signal.account.as_deref(), Some("+3611"));
    }

    #[test]
    fn unknown_keys_are_rejected_with_a_message() {
        let err = Settings::parse("clientid = \"typo\"")
            .unwrap_err()
            .to_string();
        assert!(err.contains("clientid"), "{err}");
    }

    #[test]
    fn missing_file_means_defaults_and_bad_file_names_the_path() {
        let dir = std::env::temp_dir().join(format!("comms-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        assert!(
            Settings::from_file(&dir.join("nope.toml"))
                .unwrap()
                .client_id
                .is_none()
        );
        let bad = dir.join("bad.toml");
        std::fs::write(&bad, "channels = \"sometimes\"").unwrap();
        let err = format!("{:#}", Settings::from_file(&bad).unwrap_err());
        assert!(err.contains("bad.toml"), "{err}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn environment_overrides_the_file() {
        let mut s = Settings::parse("client_id = \"file\"").unwrap();
        s.apply_env(|k| match k {
            "COMMS_CLIENT_ID" => Some("env".into()),
            "COMMS_CHANNELS" => Some("OFF".into()),
            "COMMS_TENANT" => Some("  ".into()),
            _ => None,
        });
        assert_eq!(s.client_id.as_deref(), Some("env"));
        assert_eq!(s.channels, ChannelAccess::Off);
        assert_eq!(s.tenant, "organizations", "blank values are ignored");
    }

    #[test]
    fn scopes_follow_the_channel_setting_and_never_exceed_it() {
        let mut s = Settings::default();
        let has = |s: &Settings, scope: &str| s.scopes().iter().any(|x| x == scope);

        s.channels = ChannelAccess::Off;
        assert!(has(&s, "Chat.Read") && has(&s, "ChatMessage.Send") && has(&s, "offline_access"));
        assert!(!has(&s, "Team.ReadBasic.All") && !has(&s, "ChannelMessage.Read.All"));

        s.channels = ChannelAccess::Basic;
        assert!(has(&s, "Team.ReadBasic.All") && has(&s, "ChannelMessage.Send"));
        assert!(
            !has(&s, "ChannelMessage.Read.All"),
            "admin-consent scope only on request"
        );

        s.channels = ChannelAccess::Read;
        assert!(has(&s, "ChannelMessage.Read.All"));

        for level in [
            ChannelAccess::Off,
            ChannelAccess::Basic,
            ChannelAccess::Read,
        ] {
            s.channels = level;
            assert!(
                !has(&s, "Chat.ReadWrite"),
                "no write-everything scope (least privilege)"
            );
            assert!(!s.scopes().iter().any(|x| x.ends_with(".ReadWrite.All")));
        }
    }
}
