//! Local FTS5 + structured modifier search over `message_current` and `message_fts`.

use super::*;

const CHANNEL_MATCH_CLAUSE: &str =
    "m.channel_id = ?1 OR m.channel_id IN (SELECT id FROM conversation
     WHERE workspace_id = m.workspace_id AND name = ?1 COLLATE NOCASE)";

const USER_MATCH_CLAUSE: &str = r#"m.user_id = ?1 OR m.user_id IN (SELECT id FROM "user"
     WHERE workspace_id = m.workspace_id AND (name = ?1 COLLATE NOCASE
       OR display_name = ?1 COLLATE NOCASE OR real_name = ?1 COLLATE NOCASE))"#;

impl Store {
    /// Local search: free text via FTS5 MATCH, modifiers as WHERE clauses. Newest first;
    /// deleted messages are excluded. `in:`/`from:` match an id or a stored name.
    pub fn search(
        &self,
        workspace: &TeamId,
        query: &SearchQuery,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        self.search_sorted(workspace, query, crate::query::SearchSort::Recent, limit)
    }

    /// Local search with explicit sort order (`Recent` = newest first; `Relevant` = FTS5 BM25
    /// relevance when free text is present, breaking ties by newest first).
    pub fn search_sorted(
        &self,
        workspace: &TeamId,
        query: &SearchQuery,
        sort: crate::query::SearchSort,
        limit: usize,
    ) -> Result<Vec<StoredMessage>> {
        let f = &query.filters;
        let fts = query.fts_match();
        let rank_relevant = sort == crate::query::SearchSort::Relevant && fts.is_some();
        let deleted_clause = if f.include_deleted {
            ""
        } else {
            " AND m.deleted = 0"
        };
        let mut sql = if rank_relevant {
            format!(
                "SELECT m.channel_id, m.json, m.deleted FROM message_current m
                 JOIN message_fts ON message_fts.rowid = m.id
                 WHERE m.workspace_id = ?{deleted_clause} AND message_fts MATCH ?"
            )
        } else {
            format!(
                "SELECT m.channel_id, m.json, m.deleted FROM message_current m
                 WHERE m.workspace_id = ?{deleted_clause}"
            )
        };
        let mut args = vec![SqlValue::Text(workspace.as_str().to_owned())];
        if let Some(fts) = fts {
            if rank_relevant {
                args.push(SqlValue::Text(fts));
            } else {
                sql.push_str(
                    " AND m.id IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)",
                );
                args.push(SqlValue::Text(fts));
            }
        } else if let Some(excluded_fts) = query.fts_excluded_match() {
            sql.push_str(
                " AND m.id NOT IN (SELECT rowid FROM message_fts WHERE message_fts MATCH ?)",
            );
            args.push(SqlValue::Text(excluded_fts));
        }
        filter_list(
            &mut sql,
            &mut args,
            &f.in_channels,
            CHANNEL_MATCH_CLAUSE,
            false,
        );
        filter_list(
            &mut sql,
            &mut args,
            &f.exclude_channels,
            CHANNEL_MATCH_CLAUSE,
            true,
        );
        filter_list(&mut sql, &mut args, &f.from_users, USER_MATCH_CLAUSE, false);
        filter_list(
            &mut sql,
            &mut args,
            &f.exclude_users,
            USER_MATCH_CLAUSE,
            true,
        );
        if let Some(before) = f.before {
            sql.push_str(" AND m.created_at < ?");
            args.push(SqlValue::Integer(before.unix_start()));
        }
        if let Some(after) = f.after {
            sql.push_str(" AND m.created_at >= ?");
            args.push(SqlValue::Integer(after.unix_start() + 86_400));
        }
        for has in &f.has {
            sql.push_str(match has {
                Has::Link => " AND m.has_link = 1",
                Has::File => " AND m.has_file = 1",
                Has::Pin => " AND m.is_pinned = 1",
                Has::Reaction => " AND m.has_reaction = 1",
            });
        }
        if let Some(user) = &f.includes_user {
            sql.push_str(" AND instr(m.text, ?) > 0");
            args.push(SqlValue::Text(format!("<@{user}>")));
        }
        if f.threads_only {
            sql.push_str(" AND m.thread_ts IS NOT NULL");
        }
        if rank_relevant {
            sql.push_str(" ORDER BY bm25(message_fts), m.ts_key DESC LIMIT ?");
        } else {
            sql.push_str(" ORDER BY m.ts_key DESC LIMIT ?");
        }
        args.push(SqlValue::Integer(limit as i64));

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params_from_iter(args), raw_row)?;
        collect(rows)
    }
}

/// Appends either ` AND (clause(v1) OR ...)` (`negate == false`) or
/// ` AND NOT COALESCE((clause(v1) OR ...), 0)` (`negate == true`), replacing `?1` in `clause`
/// with positional `?` placeholders. `COALESCE(..., 0)` preserves rows where a nullable column
/// (such as `m.user_id` on bot/webhook messages) makes the inner predicate evaluate to SQL `NULL`.
fn filter_list(
    sql: &mut String,
    args: &mut Vec<SqlValue>,
    values: &[String],
    clause: &str,
    negate: bool,
) {
    if values.is_empty() {
        return;
    }
    let occurrences = clause.matches("?1").count();
    let replaced = clause.replace("?1", "?");
    let parts: Vec<String> = values
        .iter()
        .map(|v| {
            args.extend((0..occurrences).map(|_| SqlValue::Text(v.clone())));
            format!("({replaced})")
        })
        .collect();
    let joined = parts.join(" OR ");
    if negate {
        sql.push_str(&format!(" AND NOT COALESCE(({joined}), 0)"));
    } else {
        sql.push_str(&format!(" AND ({joined})"));
    }
}
