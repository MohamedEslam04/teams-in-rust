//! Chat service aggregator: the list of teams, their channels, and chat metadata.

use crate::client::Client;
use crate::error::Result;
use reqwest::Method;
use serde_json::Value;

/// A channel inside a team.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub id: String,
    pub name: String,
    pub is_general: bool,
}

/// A team and its channels.
#[derive(Debug, Clone, PartialEq)]
pub struct Team {
    pub id: String,
    pub name: String,
    pub channels: Vec<Channel>,
}

/// Chat metadata from the aggregator (titles and members are not in the chat-service list).
#[derive(Debug, Clone, PartialEq)]
pub struct ChatMeta {
    pub id: String,
    pub title: Option<String>,
    pub is_one_on_one: bool,
    pub members: Vec<String>,
    /// MRIs of the members who muted the chat (the `isMuted` flag of each member).
    pub muted_by: Vec<String>,
    pub hidden: bool,
}

/// Teams, channels and chats.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub teams: Vec<Team>,
    pub chats: Vec<ChatMeta>,
}

fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)
        .and_then(Value::as_str)
        .filter(|x| !x.is_empty())
        .map(str::to_owned)
}

/// Parses the `teams/users/me` response.
pub fn parse_snapshot(v: &Value) -> Snapshot {
    let teams = v
        .get("teams")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter(|t| {
                    !t.get("isArchived")
                        .and_then(Value::as_bool)
                        .unwrap_or(false)
                })
                .filter_map(|t| {
                    let channels = t
                        .get("channels")
                        .and_then(Value::as_array)
                        .map(|c| {
                            c.iter()
                                .filter(|ch| {
                                    !ch.get("isDeleted")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false)
                                })
                                .filter_map(|ch| {
                                    let is_general = ch
                                        .get("isGeneral")
                                        .and_then(Value::as_bool)
                                        .unwrap_or(false);
                                    Some(Channel {
                                        id: s(ch, "id")?,
                                        // Only the General channel arrives without a name. Any
                                        // other channel without one is unknown, not "General".
                                        name: s(ch, "displayName")
                                            .or_else(|| is_general.then(|| "General".into()))
                                            .unwrap_or_default(),
                                        is_general,
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(Team {
                        id: s(t, "id")?,
                        name: s(t, "displayName").unwrap_or_default(),
                        channels,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let chats = v
        .get("chats")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|c| {
                    Some(ChatMeta {
                        id: s(c, "id")?,
                        title: s(c, "title"),
                        is_one_on_one: c
                            .get("isOneOnOne")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        members: c
                            .get("members")
                            .and_then(Value::as_array)
                            .map(|m| m.iter().filter_map(|x| s(x, "mri")).collect())
                            .unwrap_or_default(),
                        muted_by: c
                            .get("members")
                            .and_then(Value::as_array)
                            .map(|m| {
                                m.iter()
                                    .filter(|x| {
                                        x.get("isMuted").and_then(Value::as_bool) == Some(true)
                                    })
                                    .filter_map(|x| s(x, "mri"))
                                    .collect()
                            })
                            .unwrap_or_default(),
                        // `hidden` is true for many active 1:1 chats, so it is not a visibility flag;
                        // deleted conversations are the ones to drop.
                        hidden: c
                            .get("isConversationDeleted")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    Snapshot { teams, chats }
}

impl Client {
    /// Fetches teams, channels and chat metadata.
    pub async fn snapshot(&self) -> Result<Snapshot> {
        let url = "https://teams.microsoft.com/api/csa/api/v1/teams/users/me?isPrefetch=false&enableMembershipSummary=true";
        let v = self.json("csa", Method::GET, url, None).await?;
        Ok(parse_snapshot(&v))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_teams_and_chats() {
        let snap = parse_snapshot(&json!({
            "teams": [
                {"id": "19:t1@thread.tacv2", "displayName": "Contoso Ltd.", "channels": [
                    {"id": "19:t1@thread.tacv2", "displayName": "General", "isGeneral": true},
                    {"id": "19:c2@thread.tacv2", "displayName": "tech only"},
                    {"id": "19:c3@thread.tacv2", "displayName": "old", "isDeleted": true}
                ]},
                {"id": "19:arch@thread.tacv2", "displayName": "Archived", "isArchived": true, "channels": []}
            ],
            "chats": [
                {"id": "19:g@thread.v2", "title": "Morning Standup", "isOneOnOne": false, "members": [{"mri": "8:orgid:a"}, {"mri": "8:orgid:b", "isMuted": true}]},
                {"id": "19:a_b@unq.gbl.spaces", "title": "", "isOneOnOne": true, "hidden": true, "members": [{"mri": "8:orgid:a"}]},
                {"id": "19:gone@thread.v2", "isConversationDeleted": true}
            ]
        }));
        assert_eq!(snap.teams.len(), 1);
        assert_eq!(snap.teams[0].channels.len(), 2);
        assert!(snap.teams[0].channels[0].is_general);
        assert_eq!(snap.chats[0].title.as_deref(), Some("Morning Standup"));
        assert_eq!(snap.chats[1].title, None);
        assert!(snap.chats[1].is_one_on_one);
        assert!(
            !snap.chats[1].hidden,
            "hidden:true must not hide an active 1:1 chat"
        );
        assert!(snap.chats[2].hidden);
        assert_eq!(snap.chats[0].members, vec!["8:orgid:a", "8:orgid:b"]);
        assert_eq!(snap.chats[0].muted_by, vec!["8:orgid:b"]);
        assert!(snap.chats[1].muted_by.is_empty());
    }

    #[test]
    fn tolerates_empty() {
        assert_eq!(parse_snapshot(&json!({})), Snapshot::default());
    }

    #[test]
    fn a_channel_without_a_name_is_general_only_when_it_is_the_general_channel() {
        let snap = parse_snapshot(&json!({"teams": [{"id": "19:t@thread.tacv2", "channels": [
            {"id": "19:t@thread.tacv2", "isGeneral": true},
            {"id": "19:p@thread.tacv2", "channelType": 1},
            {"id": "19:q@thread.tacv2", "displayName": "", "channelType": 1}
        ]}]}));
        let names: Vec<&str> = snap.teams[0]
            .channels
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["General", "", ""]);
    }
}
