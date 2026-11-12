//! Slack wire types (only the fields Comms reads) and their mapping.

use chrono::{DateTime, Utc};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ResponseMetadata {
    #[serde(default)]
    pub next_cursor: String,
}

/// The cursor for the next page, `None` when Slack says there is none.
pub fn next_cursor(meta: Option<&ResponseMetadata>) -> Option<String> {
    meta.map(|m| m.next_cursor.trim())
        .filter(|c| !c.is_empty())
        .map(str::to_owned)
}

#[derive(Debug, Deserialize)]
pub struct AuthTest {
    pub user_id: String,
    #[serde(default)]
    pub user: String,
}

#[derive(Debug, Deserialize)]
pub struct UserInfo {
    pub user: SUser,
}

#[derive(Debug, Deserialize)]
pub struct SUser {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub real_name: String,
    #[serde(default)]
    pub profile: SProfile,
}

#[derive(Debug, Default, Deserialize)]
pub struct SProfile {
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub real_name: String,
}

impl SUser {
    /// Best human name: display name, then real name, then handle.
    pub fn best_name(&self) -> Option<String> {
        [
            &self.profile.display_name,
            &self.profile.real_name,
            &self.real_name,
            &self.name,
        ]
        .into_iter()
        .map(|s| s.trim())
        .find(|s| !s.is_empty())
        .map(str::to_owned)
    }
}

#[derive(Debug, Deserialize)]
pub struct ConversationsList {
    pub channels: Vec<SChannel>,
    pub response_metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct ConversationInfo {
    pub channel: SChannel,
}

#[derive(Debug, Deserialize)]
pub struct SChannel {
    pub id: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub is_im: bool,
    #[serde(default)]
    pub is_mpim: bool,
    /// Direct messages only: the other person.
    #[serde(default)]
    pub user: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct History {
    pub messages: Vec<SMessage>,
    pub response_metadata: Option<ResponseMetadata>,
}

#[derive(Debug, Deserialize)]
pub struct PostResponse {
    pub ts: String,
    #[serde(default)]
    pub message: Option<SMessage>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SMessage {
    pub ts: String,
    #[serde(default)]
    pub user: Option<String>,
    #[serde(default)]
    pub bot_id: Option<String>,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub subtype: Option<String>,
    #[serde(default)]
    pub thread_ts: Option<String>,
    #[serde(default)]
    pub edited: Option<SEdited>,
    #[serde(default)]
    pub files: Vec<SFile>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SEdited {
    pub ts: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SFile {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
}

impl SFile {
    pub fn label(&self) -> &str {
        self.name
            .as_deref()
            .or(self.title.as_deref())
            .unwrap_or("file")
    }
}

impl SMessage {
    /// Join/leave/topic changes and similar. Bot and `/me` posts are real messages.
    pub fn is_system(&self) -> bool {
        match self.subtype.as_deref() {
            None => false,
            Some(
                "bot_message" | "me_message" | "thread_broadcast" | "file_share"
                | "reply_broadcast",
            ) => false,
            Some(_) => true,
        }
    }
}

/// Parse a Slack timestamp such as `1700000000.000100` (seconds.microseconds).
pub fn parse_ts(ts: &str) -> Option<DateTime<Utc>> {
    let (secs, frac) = ts.split_once('.').unwrap_or((ts, ""));
    if !frac.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let mut micros = frac.chars().take(6).collect::<String>();
    while micros.len() < 6 {
        micros.push('0');
    }
    DateTime::from_timestamp(secs.parse().ok()?, micros.parse::<u32>().ok()? * 1000)
}

/// `mpdm-alice--bob--carol-1` to `alice, bob, carol`.
pub fn pretty_mpim_name(name: &str) -> String {
    let inner = name.strip_prefix("mpdm-").unwrap_or(name);
    let inner = match inner.rsplit_once('-') {
        Some((head, tail)) if tail.bytes().all(|b| b.is_ascii_digit()) && !tail.is_empty() => head,
        _ => inner,
    };
    inner.split("--").collect::<Vec<_>>().join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_parse_to_microseconds() {
        let t = parse_ts("1700000000.000100").unwrap();
        assert_eq!(t.timestamp(), 1_700_000_000);
        assert_eq!(t.timestamp_subsec_micros(), 100);
        assert_eq!(parse_ts("1700000000").unwrap().timestamp_subsec_micros(), 0);
        assert!(parse_ts("soon").is_none());
        assert!(parse_ts("17.0x").is_none());
    }

    #[test]
    fn group_dm_names_are_readable() {
        assert_eq!(
            pretty_mpim_name("mpdm-alice--bob--carol-1"),
            "alice, bob, carol"
        );
        assert_eq!(pretty_mpim_name("weird"), "weird");
    }

    #[test]
    fn empty_cursor_means_no_more_pages() {
        let none = ResponseMetadata {
            next_cursor: String::new(),
        };
        let some = ResponseMetadata {
            next_cursor: "abc".into(),
        };
        assert_eq!(next_cursor(None), None);
        assert_eq!(next_cursor(Some(&none)), None);
        assert_eq!(next_cursor(Some(&some)).as_deref(), Some("abc"));
    }

    #[test]
    fn join_messages_are_system_but_bot_posts_are_not() {
        let m = |subtype: Option<&str>| SMessage {
            ts: "1.0".into(),
            user: None,
            bot_id: None,
            username: None,
            text: String::new(),
            subtype: subtype.map(str::to_owned),
            thread_ts: None,
            edited: None,
            files: vec![],
        };
        assert!(m(Some("channel_join")).is_system());
        assert!(!m(Some("bot_message")).is_system());
        assert!(!m(None).is_system());
    }
}
