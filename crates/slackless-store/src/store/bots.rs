//! Persisted `bots.info` profiles (`bot_profile`).

use super::*;

impl Store {
    /// Persist a resolved `bots.info` profile so cold launches render bot names and icons
    /// immediately on frame 0 without waiting on network lookups.
    pub fn upsert_bot_profile(
        &mut self,
        workspace: &TeamId,
        bot_id: &BotId,
        profile: &BotProfile,
    ) -> Result<()> {
        self.ensure_writable()?;
        let json = serde_json::to_string(profile)?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO bot_profile (workspace_id, bot_id, json, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (workspace_id, bot_id) DO UPDATE SET
                json = excluded.json,
                updated_at = excluded.updated_at",
        )?;
        stmt.execute(params![
            workspace.as_str(),
            bot_id.as_str(),
            json,
            now_epoch_secs()
        ])?;
        Ok(())
    }

    /// All persisted `bots.info` profiles for `workspace`.
    pub fn bot_profiles(&self, workspace: &TeamId) -> Result<HashMap<BotId, BotProfile>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT bot_id, json FROM bot_profile WHERE workspace_id = ?1")?;
        let rows = stmt.query_map(params![workspace.as_str()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut out = HashMap::new();
        for row in rows {
            let (id, raw) = row?;
            if let Ok(profile) = serde_json::from_str::<BotProfile>(&raw) {
                out.insert(BotId::from(id), profile);
            }
        }
        Ok(out)
    }
}
