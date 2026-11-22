use super::*;

/// A person in the share dialog's picker.
#[derive(Debug, Clone, PartialEq)]
pub struct PersonData {
    /// The MRI (`8:orgid:<oid>`).
    pub id: String,
    pub name: String,
    pub detail: String,
    pub picked: bool,
    pub(super) email: Option<String>,
}

/// Who or what has access, as the "Manage access" list shows it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct PermData {
    pub id: String,
    pub who: String,
    /// `read`, `write` or `owner`.
    pub role: String,
    pub link: bool,
    pub detail: String,
    /// The role can be changed and the access removed.
    pub editable: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ShareData {
    pub name: String,
    pub results: Vec<PersonData>,
    pub picked: Vec<PersonData>,
    pub perms: Vec<PermData>,
    /// `read` or `write`.
    pub role: &'static str,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VersionData {
    pub id: String,
    pub when: String,
    pub by: String,
    pub size: String,
    pub current: bool,
}

#[derive(Clone, Default)]
pub struct PickerData {
    pub purpose: String,
    pub title: String,
    /// Folders only (the destination of Move to and Copy to); else files can be picked too.
    pub folders_only: bool,
    pub rows: Vec<RowData>,
    pub crumbs: Vec<String>,
    pub loading: bool,
    pub can_choose: bool,
}

#[derive(Clone, Default)]
pub struct PreviewData {
    pub title: String,
    /// `image`, `text`, `markdown`.
    pub kind: &'static str,
    pub image: Option<Pixels>,
    pub text: String,
    /// Pages previews only: the page shown (from 1) and how many there are.
    pub page: usize,
    pub pages: usize,
}
