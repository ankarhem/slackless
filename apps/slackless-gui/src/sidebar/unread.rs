//! Unread & mention count aggregation, dock badge calculation, and row/section stepping.

use slackless_core::NotifyLevel;
use slackless_core::types::{ChannelId, Conversation, Message, Ts};

use super::helpers::mentions_user;
use super::{Sidebar, Unread};

impl Sidebar {
    /// The dock badge count: unread Activity items plus unread DM messages.
    pub fn badge_count(&self) -> u32 {
        let watermark = self.read_watermark();
        self.activity_unread
            + self
                .total_unread(&self.fixtures.dms, watermark.as_ref())
                .count
    }

    /// Whether any channel or DM in this workspace currently has unread messages.
    pub fn has_any_unreads(&self) -> bool {
        let watermark = self.read_watermark();
        self.activity_unread > 0
            || self
                .total_unread(&self.fixtures.channels, watermark.as_ref())
                .count
                > 0
            || self
                .total_unread(&self.fixtures.dms, watermark.as_ref())
                .count
                > 0
            || self.total_unread(&self.joined, watermark.as_ref()).count > 0
    }

    /// Newest message in the open conversation: the fixture-mode "read" watermark. Live mode
    /// will replace this with the per-conversation `last_read` from `slackless-store`.
    pub(super) fn read_watermark(&self) -> Option<Ts> {
        self.fixtures
            .messages
            .get(&self.selected)
            .and_then(|history| history.iter().map(|message| message.ts.clone()).max())
    }

    /// Messages in `conversation` the signed-in user has not read (see [`Unread`]).
    pub(super) fn unread_messages(
        &self,
        conversation: &Conversation,
        watermark: Option<&Ts>,
    ) -> Vec<&Message> {
        if conversation.id == self.selected || self.read.contains(&conversation.id) {
            return Vec::new();
        }
        let history = self.fixtures.messages.get(&conversation.id);
        history
            .into_iter()
            .flatten()
            .filter(|message| {
                let from_self = message
                    .user
                    .as_ref()
                    .is_some_and(|user| *user == self.fixtures.self_user);
                !from_self && watermark.is_none_or(|mark| message.ts > *mark)
            })
            .collect()
    }

    pub(super) fn unread_in(&self, conversation: &Conversation, watermark: Option<&Ts>) -> Unread {
        // Live: Slack's own unread state. A DM's unread messages all count as mentions.
        let unread = if let Some(counts) = self.fixtures.counts.get(&conversation.id) {
            if conversation.id == self.selected || self.read.contains(&conversation.id) {
                return Unread::default();
            }
            Unread {
                count: u32::from(counts.is_unread()),
                mentions: counts.effective_mentions(),
            }
        } else {
            let messages = self.unread_messages(conversation, watermark);
            Unread {
                count: messages.len() as u32,
                mentions: messages
                    .iter()
                    .filter(|message| mentions_user(message, &self.fixtures.self_user))
                    .count() as u32,
            }
        };
        // Muted conversations never surface as unread unless they mention you (matches Slack and
        // `unreads::local_groups`).
        if self.notify_level(&conversation.id) == Some(NotifyLevel::Muted) && unread.mentions == 0 {
            return Unread::default();
        }
        unread
    }

    pub(super) fn total_unread<'a>(
        &self,
        conversations: impl IntoIterator<Item = &'a Conversation>,
        watermark: Option<&Ts>,
    ) -> Unread {
        let mut total = Unread::default();
        for conversation in conversations {
            let unread = self.unread_in(conversation, watermark);
            total.count += unread.count;
            total.mentions += unread.mentions;
        }
        total
    }

    /// The conversation row above (`forward` false) or below `from` in the Home list as laid
    /// out, or the nearest unread one with `unread_only` (Slack's Opt+Up/Down and
    /// Opt+Shift+Up/Down); from a conversation not in the list, the first or last match.
    pub fn step(&self, from: &ChannelId, forward: bool, unread_only: bool) -> Option<ChannelId> {
        let rows = self.rows.borrow();
        let at = rows.iter().position(|(id, _)| id == from);
        let pick = |row: &&(ChannelId, bool)| !unread_only || row.1;
        let found = if forward {
            rows.iter().skip(at.map_or(0, |at| at + 1)).find(pick)
        } else {
            rows[..at.unwrap_or(rows.len())].iter().rev().find(pick)
        };
        found.map(|(id, _)| id.clone())
    }

    /// Jump to the first conversation in the next or previous sidebar section (`F-SHELL-270`).
    pub fn step_section(&self, from: &ChannelId, forward: bool) -> Option<ChannelId> {
        let mut heads: Vec<ChannelId> = Vec::new();
        for section in &self.fixtures.sections {
            if let Some(first) = section
                .channel_ids
                .iter()
                .find(|id| self.fixtures.conversation(id).is_some())
                && !heads.contains(first)
            {
                heads.push(first.clone());
            }
        }
        if let Some(first_ch) = self.fixtures.channels.first()
            && !heads.contains(&first_ch.id)
        {
            heads.push(first_ch.id.clone());
        }
        if let Some(first_dm) = self.fixtures.dms.first()
            && !heads.contains(&first_dm.id)
        {
            heads.push(first_dm.id.clone());
        }
        if heads.is_empty() {
            return None;
        }
        let rows = self.rows.borrow();
        let cur_row = rows.iter().position(|(id, _)| id == from);
        let head_positions: Vec<(usize, ChannelId)> = heads
            .into_iter()
            .filter_map(|id| rows.iter().position(|(r, _)| r == &id).map(|pos| (pos, id)))
            .collect();
        if head_positions.is_empty() {
            return None;
        }
        if forward {
            let cur = cur_row.unwrap_or(0);
            head_positions
                .iter()
                .find(|(pos, _)| *pos > cur)
                .or_else(|| head_positions.first())
                .map(|(_, id)| id.clone())
        } else {
            let cur = cur_row.unwrap_or(usize::MAX);
            head_positions
                .iter()
                .rev()
                .find(|(pos, _)| *pos < cur)
                .or_else(|| head_positions.last())
                .map(|(_, id)| id.clone())
        }
    }

    /// Conversations with unread messages, in sidebar row order: the search pill's
    /// dropdown "Unreads & drafts" section (the root adds drafted conversations).
    pub fn unread_ids(&self) -> Vec<ChannelId> {
        let watermark = self.read_watermark();
        self.fixtures
            .channels
            .iter()
            .chain(&self.fixtures.dms)
            .filter(|conversation| self.unread_in(conversation, watermark.as_ref()).count > 0)
            .map(|conversation| conversation.id.clone())
            .collect()
    }

    /// The search dropdown's unread state, alongside [`Self::unread_ids`]: per conversation
    /// unread-mention counts (conversations with any only), and the followed-threads state
    /// (unread replies present, mentions among them) for its Threads row and badge.
    pub fn search_unread_counts(&self) -> (Vec<(ChannelId, u32)>, (bool, u32)) {
        let watermark = self.read_watermark();
        let mentions = self
            .fixtures
            .channels
            .iter()
            .chain(&self.fixtures.dms)
            .filter_map(|conversation| {
                let mentions = self.unread_in(conversation, watermark.as_ref()).mentions;
                (mentions > 0).then(|| (conversation.id.clone(), mentions))
            })
            .collect();
        (mentions, self.threads_unread)
    }
}
