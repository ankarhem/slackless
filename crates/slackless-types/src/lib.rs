//! Shared Slack data model for slackless. Pure data + serde; no I/O.
//!
//! Patchability: Slack's web-client API is private and drifts. Every response struct tolerates
//! missing and unknown fields, string enums carry an `Other(String)` catch-all, and tagged JSON
//! (`blocks`, rich text elements, websocket events) carries an `Unknown { r#type, raw }` variant,
//! so new Slack data never fails a parse. URLs and method names live in [`endpoints`].

mod de;
mod macros;

pub mod conversation;
pub mod endpoints;
pub mod event;
pub mod file;
pub mod ids;
pub mod message;
pub mod reaction;
pub mod response;
pub mod rich_text;
pub mod section;
pub mod user;

pub use conversation::{Conversation, ConversationKind, Latest, TopicPurpose};
pub use event::{MessageEvent, SlackEvent};
pub use file::{File, FileMode};
pub use ids::{BotId, ChannelId, FileId, TeamId, Ts, UserId, UsergroupId};
pub use message::{Attachment, BotProfile, Edited, Message, MessageSubtype};
pub use reaction::Reaction;
pub use response::{ApiEnvelope, ApiResponse, Empty, ResponseError, ResponseMetadata};
pub use rich_text::{Block, RichTextBlock, RichTextContainer, RichTextElement, TextStyle};
pub use section::{ChannelSection, SectionKind};
pub use user::{DndStatus, Presence, Profile, Team, User, UserPresence};
