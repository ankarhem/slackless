//! File listing, external upload v2, favorites, collections, and recently deleted wire types.

use serde::Deserialize;

use crate::de::{lenient_vec, or_default};
use crate::types::{ChannelId, File, UserId};

/// One pending file to upload through Slack's external upload v2 flow.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadFile {
    pub filename: String,
    pub bytes: Vec<u8>,
}

/// Answer to `files.getUploadURLExternal`.
#[derive(Debug, Clone, Deserialize)]
pub struct UploadSlot {
    pub file_id: String,
    pub upload_url: String,
}

/// Filters for `files.list` (`FilesFilter::channel` / `FilesFilter::workspace` plus the
/// optional narrowings).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FilesFilter {
    /// The conversation whose shared files to list (the Files & links tab); `None` asks
    /// for every file the signed-in user can see workspace-wide (the Files view).
    pub channel: Option<ChannelId>,
    /// Only files uploaded by this person.
    pub user: Option<UserId>,
    /// Comma-delimited file kinds: Slack documents `images`, `pdfs`, `snippets`, `spaces`
    /// (legacy Posts) and more; `canvas` (canvases) is observed but undocumented.
    pub types: Option<String>,
}

impl FilesFilter {
    /// The files shared with one conversation.
    pub fn channel(channel: ChannelId) -> Self {
        Self {
            channel: Some(channel),
            ..Default::default()
        }
    }

    /// Every file the signed-in user can see in the workspace.
    pub fn workspace() -> Self {
        Self::default()
    }
}

#[derive(Deserialize)]
pub(in crate::methods) struct FilesWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) files: Vec<File>,
}

/// One custom file collection/section (`files.collections.list`, `F-CORE-133`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileCollection {
    pub id: String,
    pub name: String,
    pub emoji: String,
    pub file_ids: Vec<String>,
}

/// One recently deleted canvas or list (`files.recentlyDeleted`, `F-CORE-133`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeletedFile {
    pub id: String,
    pub title: String,
    pub date_delete: i64,
    pub file_type: String,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct FileFavoritesWire {
    #[serde(default, deserialize_with = "or_default")]
    pub(in crate::methods) file_ids: Vec<String>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct FileCollectionFileWire {
    #[serde(default)]
    pub(in crate::methods) id: String,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct FileCollectionWire {
    #[serde(default)]
    pub(in crate::methods) id: String,
    #[serde(default)]
    pub(in crate::methods) name: String,
    #[serde(default)]
    pub(in crate::methods) emoji: String,
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) files: Vec<FileCollectionFileWire>,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct FileCollectionsListWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) collections: Vec<FileCollectionWire>,
}

impl From<FileCollectionWire> for FileCollection {
    fn from(w: FileCollectionWire) -> Self {
        Self {
            id: w.id,
            name: w.name,
            emoji: w.emoji,
            file_ids: w
                .files
                .into_iter()
                .map(|f| f.id)
                .filter(|id| !id.is_empty())
                .collect(),
        }
    }
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct DeletedFileWire {
    #[serde(default)]
    pub(in crate::methods) id: String,
    #[serde(default)]
    pub(in crate::methods) title: String,
    #[serde(default)]
    pub(in crate::methods) date_delete: i64,
    #[serde(default)]
    pub(in crate::methods) file_type: String,
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct RecentlyDeletedWire {
    #[serde(default, deserialize_with = "lenient_vec")]
    pub(in crate::methods) files: Vec<DeletedFileWire>,
}
