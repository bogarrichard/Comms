//! The subset of signal-cli's JSON that Comms reads. Every field is optional
//! and unknown fields are ignored, so newer signal-cli versions keep working.

use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Envelope {
    pub source: Option<String>,
    pub source_number: Option<String>,
    pub source_uuid: Option<String>,
    pub source_name: Option<String>,
    pub timestamp: Option<i64>,
    pub data_message: Option<DataMessage>,
    pub sync_message: Option<SyncMessage>,
}

impl Envelope {
    /// Who sent it: the phone number when known, else the UUID.
    pub fn sender(&self) -> Option<String> {
        first_non_empty([&self.source_number, &self.source_uuid, &self.source])
    }

    /// Pull the envelope out of a `receive` notification. signal-cli has put it
    /// at `params.envelope` and at `params.result.envelope` in different modes.
    pub fn from_params(params: &Value) -> Option<Envelope> {
        let env = params
            .get("envelope")
            .or_else(|| params.get("result").and_then(|r| r.get("envelope")))?;
        serde_json::from_value(env.clone()).ok()
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DataMessage {
    pub timestamp: Option<i64>,
    pub message: Option<String>,
    pub group_info: Option<GroupInfo>,
    pub attachments: Vec<AttachmentInfo>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupInfo {
    pub group_id: Option<String>,
    pub group_name: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttachmentInfo {
    pub filename: Option<String>,
    pub content_type: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncMessage {
    pub sent_message: Option<SentMessage>,
    pub read_messages: Vec<ReadMessage>,
}

/// A message this account sent from another device (usually the phone).
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SentMessage {
    pub destination: Option<String>,
    pub destination_number: Option<String>,
    pub destination_uuid: Option<String>,
    pub timestamp: Option<i64>,
    pub message: Option<String>,
    pub group_info: Option<GroupInfo>,
    pub attachments: Vec<AttachmentInfo>,
}

impl SentMessage {
    pub fn destination(&self) -> Option<String> {
        first_non_empty([
            &self.destination_number,
            &self.destination_uuid,
            &self.destination,
        ])
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ReadMessage {
    pub sender: Option<String>,
    pub sender_number: Option<String>,
    pub sender_uuid: Option<String>,
    pub timestamp: Option<i64>,
}

impl ReadMessage {
    pub fn sender(&self) -> Option<String> {
        first_non_empty([&self.sender_number, &self.sender_uuid, &self.sender])
    }
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct GroupEntry {
    pub id: Option<String>,
    pub name: Option<String>,
    pub is_member: Option<bool>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ContactEntry {
    pub number: Option<String>,
    pub uuid: Option<String>,
    pub name: Option<String>,
    pub profile_name: Option<String>,
}

impl ContactEntry {
    pub fn display_name(&self) -> Option<String> {
        first_non_empty([&self.name, &self.profile_name])
    }
}

/// `result` of the `send` method.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SendResult {
    pub timestamp: Option<i64>,
    pub results: Vec<RecipientResult>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct RecipientResult {
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

fn first_non_empty<const N: usize>(options: [&Option<String>; N]) -> Option<String> {
    options
        .into_iter()
        .flatten()
        .find(|s| !s.trim().is_empty())
        .cloned()
}
