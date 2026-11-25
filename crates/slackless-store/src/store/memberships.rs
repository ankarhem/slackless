//! Conversation membership queries and bulk membership synchronization.

use super::*;

/// Only channel rows carry a membership flag worth syncing.
const CHANNEL_KINDS: &str = "('public_channel', 'private_channel')";

impl Store {
    /// Splits `members` into Slack's header counts (see [`MemberCounts`]). The workspace's
    /// built-in `USLACKBOT` is neither a human nor an app; a deactivated member is neither.
    pub fn member_counts(&self, workspace: &TeamId, members: &[UserId]) -> Result<MemberCounts> {
        /// Slack's built-in workspace bot.
        const USLACKBOT: &str = "USLACKBOT";
        let mut seen: HashSet<&str> = HashSet::with_capacity(members.len());
        let ask: Vec<&UserId> = members
            .iter()
            .filter(|id| id.as_str() != USLACKBOT && seen.insert(id.as_str()))
            .collect();
        let mut counts = MemberCounts {
            humans: ask.len() as u32,
            apps: 0,
        };
        if ask.is_empty() {
            return Ok(counts);
        }
        // Everyone starts as a human (unknown identities included); the stored profiles then
        // re-classify the bots and drop the deactivated.
        query_in_chunks(
            &self.conn,
            workspace,
            &ask,
            |id| id.as_str(),
            r#"SELECT is_bot, deleted FROM "user" WHERE workspace_id = ?1 AND id"#,
            |r| Ok((r.get::<_, bool>(0)?, r.get::<_, bool>(1)?)),
            |(is_bot, deleted)| {
                if deleted {
                    counts.humans -= 1;
                } else if is_bot {
                    counts.humans -= 1;
                    counts.apps += 1;
                }
            },
        )?;
        Ok(counts)
    }

    /// Which of `channels` are stored with `is_member = 1` (one query per chunk): the boot hydrate
    /// preserves these flags, because `client.userBoot` objects omit `is_member` and the
    /// membership sync (`users.conversations`) only runs after them.
    pub fn member_flags(
        &self,
        workspace: &TeamId,
        channels: &[ChannelId],
    ) -> Result<HashSet<ChannelId>> {
        if channels.is_empty() {
            return Ok(HashSet::new());
        }
        let mut out = HashSet::new();
        query_in_chunks(
            &self.conn,
            workspace,
            channels,
            ChannelId::as_str,
            "SELECT id FROM conversation
             WHERE workspace_id = ?1 AND is_member = 1 AND id",
            |r| r.get::<_, String>(0),
            |id| {
                out.insert(ChannelId::from(id));
            },
        )?;
        Ok(out)
    }

    /// Records the signed-in user's channel memberships (`users.conversations`): the ids in
    /// `members` become `is_member = 1`, every other stored public/private channel `0`, the
    /// `json` copy of the flag included. One transaction; rows that already agree are not
    /// rewritten. Returns the ids whose flag flipped, in both directions, so callers announce
    /// exactly the changed rows. IM/MPIM rows are never touched: their flag is not channel
    /// membership.
    pub fn set_channel_memberships(
        &mut self,
        workspace: &TeamId,
        members: &[ChannelId],
    ) -> Result<Vec<ChannelId>> {
        self.ensure_writable()?;
        let mut flipped = Vec::new();
        let tx = self.conn.transaction()?;
        let now = now_secs();
        if members.len() <= SQL_IN_CHUNK {
            if !members.is_empty() {
                let in_list = placeholders(members.len());
                flipped.extend(flip_membership_direction(
                    &tx,
                    workspace,
                    now,
                    true,
                    &format!(" AND id IN ({in_list})"),
                    members,
                )?);
                flipped.extend(flip_membership_direction(
                    &tx,
                    workspace,
                    now,
                    false,
                    &format!(" AND id NOT IN ({in_list})"),
                    members,
                )?);
            } else {
                flipped.extend(flip_membership_direction(
                    &tx,
                    workspace,
                    now,
                    false,
                    "",
                    &[],
                )?);
            }
        } else {
            tx.execute_batch(
                "CREATE TEMP TABLE IF NOT EXISTS _slackless_member_ids (id TEXT PRIMARY KEY) WITHOUT ROWID;
                 DELETE FROM _slackless_member_ids;",
            )?;
            {
                let mut ins = tx.prepare_cached(
                    "INSERT OR IGNORE INTO _slackless_member_ids (id) VALUES (?1)",
                )?;
                for id in members {
                    ins.execute(params![id.as_str()])?;
                }
            }
            flipped.extend(flip_membership_direction(
                &tx,
                workspace,
                now,
                true,
                " AND id IN (SELECT id FROM _slackless_member_ids)",
                &[],
            )?);
            flipped.extend(flip_membership_direction(
                &tx,
                workspace,
                now,
                false,
                " AND id NOT IN (SELECT id FROM _slackless_member_ids)",
                &[],
            )?);
            tx.execute("DELETE FROM _slackless_member_ids", [])?;
        }
        tx.commit()?;
        if !flipped.is_empty() {
            Self::bump_message_revision();
        }
        Ok(flipped)
    }
}

/// Runs one join (`joining == true`) or leave (`joining == false`) membership
/// `UPDATE ... RETURNING id`, returning every channel id whose flag flipped.
fn flip_membership_direction(
    tx: &Transaction<'_>,
    workspace: &TeamId,
    now: i64,
    joining: bool,
    id_filter: &str,
    extra_ids: &[ChannelId],
) -> Result<Vec<ChannelId>> {
    let (new_flag, old_flag, json_bool) = if joining {
        (1, 0, "true")
    } else {
        (0, 1, "false")
    };
    let sql = format!(
        "UPDATE conversation
         SET is_member = {new_flag}, json = json_set(json, '$.is_member', json('{json_bool}')),
             updated_at = ?1
         WHERE workspace_id = ?2 AND kind IN {CHANNEL_KINDS} AND is_member = {old_flag}{id_filter}
         RETURNING id"
    );
    let mut args = Vec::with_capacity(extra_ids.len() + 2);
    args.push(SqlValue::Integer(now));
    args.push(SqlValue::Text(workspace.as_str().to_owned()));
    args.extend(
        extra_ids
            .iter()
            .map(|id| SqlValue::Text(id.as_str().to_owned())),
    );
    let mut stmt = tx.prepare(&sql)?;
    let rows = stmt.query_map(params_from_iter(args), |r| r.get::<_, String>(0))?;
    Ok(rows
        .collect::<rusqlite::Result<Vec<_>>>()?
        .into_iter()
        .map(ChannelId::from)
        .collect())
}
