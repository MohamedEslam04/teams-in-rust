//! Middle tier: user profiles and avatars.

use crate::client::Client;
use crate::error::Result;
use reqwest::Method;
use serde_json::{Value, json};

/// A user's display profile.
#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub mri: String,
    pub display_name: String,
    pub email: Option<String>,
    pub job_title: Option<String>,
    pub department: Option<String>,
    /// The sign-in name (`userPrincipalName`) when the answer has one: a guest's carries `#EXT#`
    /// while `email` may be their home address.
    pub upn: Option<String>,
    /// `userType` (`Member`, `Guest`), when the answer has it.
    pub user_type: Option<String>,
    /// The person's home tenant (`tenantId`), when the answer has it.
    pub tenant_id: Option<String>,
}

/// Parses a `fetchShortProfile` response.
pub fn parse_profiles(v: &Value) -> Vec<Profile> {
    v.get("value")
        .or_else(|| v.get("resolvedUsers"))
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .filter_map(|p| {
                    let mri = p.get("mri").and_then(Value::as_str)?.to_string();
                    let display_name = p
                        .get("displayName")
                        .and_then(Value::as_str)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .or_else(|| {
                            let g = p
                                .get("givenName")
                                .and_then(Value::as_str)
                                .unwrap_or_default();
                            let s = p.get("surname").and_then(Value::as_str).unwrap_or_default();
                            let full = format!("{g} {s}").trim().to_string();
                            (!full.is_empty()).then_some(full)
                        })
                        .unwrap_or_else(|| mri.clone());
                    let email = p
                        .get("email")
                        .or_else(|| p.get("userPrincipalName"))
                        .and_then(Value::as_str)
                        .map(str::to_owned);
                    let opt = |k: &str| {
                        p.get(k)
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    };
                    Some(Profile {
                        mri,
                        display_name,
                        email,
                        job_title: opt("jobTitle"),
                        department: opt("department"),
                        upn: opt("userPrincipalName"),
                        user_type: opt("userType"),
                        tenant_id: opt("tenantId"),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

impl Client {
    /// Fetches display names for up to ~100 MRIs (`8:orgid:…`) per call.
    pub async fn profiles(&self, mris: &[String]) -> Result<Vec<Profile>> {
        let mut out = vec![];
        let mt = self.middle_tier().await;
        for chunk in mris.chunks(100) {
            let url = format!(
                "{mt}/beta/users/fetchShortProfile?isMailAddress=false&enableGuest=true&includeIBBarredUsers=true&skypeTeamsInfo=true&includeBots=true"
            );
            let v = self
                .json("profiles", Method::POST, &url, Some(&json!(chunk)))
                .await?;
            out.extend(parse_profiles(&v));
        }
        Ok(out)
    }

    /// Downloads a user's avatar (JPEG/PNG bytes) at [`AVATAR_SIZE`]. `self_mri` is our own MRI.
    pub async fn avatar(&self, self_mri: &str, mri: &str) -> Result<Vec<u8>> {
        self.avatar_sized(self_mri, mri, AVATAR_SIZE).await
    }

    /// A user's avatar at one of the sizes the service offers (`HR96x96`, `HR196x196`,
    /// `HR432x432`, `HR648x648`); [`AVATAR_PREVIEW_SIZE`] is what the photo preview asks for.
    pub async fn avatar_sized(&self, self_mri: &str, mri: &str, size: &str) -> Result<Vec<u8>> {
        let url = avatar_url(&self.middle_tier().await, self_mri, mri, size);
        self.bytes(&url, &[], 4 * 1024 * 1024).await
    }

    /// Downloads a team's picture (JPEG/PNG bytes). UNVERIFIED: read from the reference API
    /// description (`fetchTeamPicture`), not yet sent by IronTeams to any account. The sized form
    /// is tried first; if the service refuses the `size` parameter the plain form is read.
    pub async fn team_picture(
        &self,
        self_mri: &str,
        group_id: &str,
        name: &str,
    ) -> Result<Vec<u8>> {
        let mt = self.middle_tier().await;
        let sized = team_picture_url(&mt, self_mri, group_id, name, Some(AVATAR_SIZE));
        match self.bytes(&sized, &[], 4 * 1024 * 1024).await {
            Ok(b) => Ok(b),
            Err(_) => {
                let plain = team_picture_url(&mt, self_mri, group_id, name, None);
                self.bytes(&plain, &[], 4 * 1024 * 1024).await
            }
        }
    }
}

/// The avatar size every list, chat and card photo is read at: the largest of them is drawn 96
/// logical pixels wide, which is 192 physical pixels on a 2x screen.
pub const AVATAR_SIZE: &str = "HR196x196";
/// The size of the photo preview, which shows the picture as large as the window allows.
pub const AVATAR_PREVIEW_SIZE: &str = "HR648x648";

/// `GET <middleTier>/beta/users/<selfGuid>/profilepicturev2/<mri>?size=<size>`: a user's avatar.
pub fn avatar_url(middle_tier: &str, self_mri: &str, mri: &str, size: &str) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let self_guid = self_mri
        .trim_start_matches("8:")
        .trim_start_matches("orgid:");
    format!(
        "{middle_tier}/beta/users/{}/profilepicturev2/{}?size={size}",
        enc(self_guid),
        enc(mri)
    )
}

/// `GET <middleTier>/beta/users/<selfGuid>/profilepicturev2/teams/<groupId>?displayName=<name>`:
/// a team's picture, keyed by its Microsoft 365 group id, optionally at a `size` (UNVERIFIED for
/// teams: the user form takes it).
pub fn team_picture_url(
    middle_tier: &str,
    self_mri: &str,
    group_id: &str,
    name: &str,
    size: Option<&str>,
) -> String {
    let enc = |s: &str| url::form_urlencoded::byte_serialize(s.as_bytes()).collect::<String>();
    let self_guid = self_mri
        .trim_start_matches("8:")
        .trim_start_matches("orgid:");
    let size = size.map(|s| format!("&size={s}")).unwrap_or_default();
    format!(
        "{middle_tier}/beta/users/{}/profilepicturev2/teams/{}?displayName={}{size}",
        enc(self_guid),
        enc(group_id),
        enc(name)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const MT: &str = "https://mt.example/api/mt/part/emea-02";
    const ME: &str = "8:orgid:00000000-0000-0000-0000-000000000001";

    #[test]
    fn team_picture_url_names_the_group_and_the_team() {
        assert_eq!(
            team_picture_url(
                MT,
                ME,
                "00000000-0000-0000-0000-0000000000c1",
                "Contoso & Co",
                None
            ),
            "https://mt.example/api/mt/part/emea-02/beta/users/00000000-0000-0000-0000-000000000001/profilepicturev2/teams/00000000-0000-0000-0000-0000000000c1?displayName=Contoso+%26+Co"
        );
    }

    #[test]
    fn a_team_picture_asks_for_the_avatar_size_when_told_to() {
        let url = team_picture_url(MT, ME, "g1", "Team", Some(AVATAR_SIZE));
        assert!(url.ends_with("?displayName=Team&size=HR196x196"), "{url}");
    }

    #[test]
    fn avatars_are_requested_big_enough_for_a_two_x_screen() {
        let other = "8:orgid:00000000-0000-0000-0000-000000000002";
        assert_eq!(
            avatar_url(MT, ME, other, AVATAR_SIZE),
            "https://mt.example/api/mt/part/emea-02/beta/users/00000000-0000-0000-0000-000000000001/profilepicturev2/8%3Aorgid%3A00000000-0000-0000-0000-000000000002?size=HR196x196"
        );
        // 72 and 96 logical pixels are the largest avatars: 192 physical pixels at 2x.
        let px: u32 = AVATAR_SIZE
            .trim_start_matches("HR")
            .split('x')
            .next()
            .unwrap()
            .parse()
            .unwrap();
        assert!(px >= 192, "{AVATAR_SIZE} is below 96 px at 2x");
        assert!(avatar_url(MT, ME, other, AVATAR_PREVIEW_SIZE).ends_with("?size=HR648x648"));
    }

    #[test]
    fn parses_profiles() {
        let p = parse_profiles(&json!({"value": [
            {"mri": "8:orgid:a", "displayName": "Sara Hassan", "email": "sara@x.com", "jobTitle": "Engineer", "department": ""},
            {"mri": "8:orgid:b", "displayName": "", "givenName": "Omar", "surname": "Khaled"},
            {"mri": "8:orgid:c"},
            {"displayName": "no mri"}
        ]}));
        assert_eq!(p.len(), 3);
        assert_eq!(p[0].email.as_deref(), Some("sara@x.com"));
        assert_eq!(p[0].job_title.as_deref(), Some("Engineer"));
        assert_eq!(p[0].department, None);
        assert_eq!(p[1].job_title, None);
        assert_eq!(p[1].display_name, "Omar Khaled");
        assert_eq!(p[2].display_name, "8:orgid:c");
    }

    #[test]
    fn parses_what_a_profile_says_about_a_guest() {
        let p = parse_profiles(&json!({"value": [
            {"mri": "8:orgid:g", "displayName": "Guest User", "email": "guest.user@home.example",
             "userPrincipalName": "guest.user_home.example#EXT#@contoso.example",
             "userType": "Guest", "tenantId": "tenant-1"},
            {"mri": "8:orgid:m", "displayName": "Member", "userPrincipalName": "member@contoso.example"}
        ]}));
        assert_eq!(p[0].email.as_deref(), Some("guest.user@home.example"));
        assert_eq!(
            p[0].upn.as_deref(),
            Some("guest.user_home.example#EXT#@contoso.example")
        );
        assert_eq!(p[0].user_type.as_deref(), Some("Guest"));
        assert_eq!(p[0].tenant_id.as_deref(), Some("tenant-1"));
        assert_eq!(p[1].upn.as_deref(), Some("member@contoso.example"));
        assert_eq!(
            (p[1].user_type.as_deref(), p[1].tenant_id.as_deref()),
            (None, None)
        );
    }
}
