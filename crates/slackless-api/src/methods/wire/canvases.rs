//! Slack Canvas creation, content, and edit-operation wire types.

use serde::Deserialize;
use serde_json::Value;

use crate::de::or_default;

/// `conversations.canvases.create`: the id of the created channel canvas.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct CanvasCreated {
    #[serde(default, deserialize_with = "or_default")]
    pub canvas_id: String,
}

/// One section-level edit operation for `canvases.edit` (`F-COLLAB-132`, `F-SHELL-264`, `F-SHELL-277`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanvasEditOp {
    InsertAtStart {
        markdown: String,
    },
    InsertAtEnd {
        markdown: String,
    },
    InsertAfter {
        section_id: String,
        markdown: String,
    },
    InsertBefore {
        section_id: String,
        markdown: String,
    },
    /// Replace `section_id`, or the entire canvas when `section_id` is `None`.
    Replace {
        section_id: Option<String>,
        markdown: String,
    },
    Delete {
        section_id: String,
    },
    Rename {
        title: String,
    },
}

impl CanvasEditOp {
    pub(in crate::methods) fn to_change_json(&self) -> Value {
        let doc = |md: &str| serde_json::json!({ "type": "markdown", "markdown": md });
        match self {
            Self::InsertAtStart { markdown } => serde_json::json!({
                "operation": "insert_at_start",
                "document_content": doc(markdown),
            }),
            Self::InsertAtEnd { markdown } => serde_json::json!({
                "operation": "insert_at_end",
                "document_content": doc(markdown),
            }),
            Self::InsertAfter {
                section_id,
                markdown,
            } => serde_json::json!({
                "operation": "insert_after",
                "section_id": section_id,
                "document_content": doc(markdown),
            }),
            Self::InsertBefore {
                section_id,
                markdown,
            } => serde_json::json!({
                "operation": "insert_before",
                "section_id": section_id,
                "document_content": doc(markdown),
            }),
            Self::Replace {
                section_id,
                markdown,
            } => {
                let mut obj = serde_json::Map::new();
                obj.insert("operation".to_owned(), Value::String("replace".to_owned()));
                if let Some(id) = section_id.as_deref().filter(|s| !s.is_empty()) {
                    obj.insert("section_id".to_owned(), Value::String(id.to_owned()));
                }
                obj.insert("document_content".to_owned(), doc(markdown));
                Value::Object(obj)
            }
            Self::Delete { section_id } => serde_json::json!({
                "operation": "delete",
                "section_id": section_id,
            }),
            Self::Rename { title } => serde_json::json!({
                "operation": "rename",
                "title_content": doc(title),
            }),
        }
    }
}

#[derive(Deserialize, Default)]
pub(in crate::methods) struct CanvasContentWire {
    #[serde(default)]
    pub(in crate::methods) content: String,
}
