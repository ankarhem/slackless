//! Durable FIFO offline mutation queue (`offline_queue`).

use super::*;

impl Store {
    /// Append a serialized offline mutation payload for `workspace`, returning its row id.
    pub fn enqueue_offline(&mut self, workspace: &TeamId, payload: &str) -> Result<i64> {
        self.ensure_writable()?;
        let mut stmt = self.conn.prepare_cached(
            "INSERT INTO offline_queue (workspace_id, payload, created_at) VALUES (?1, ?2, ?3)",
        )?;
        stmt.execute(params![workspace.as_str(), payload, now_secs()])?;
        Ok(self.conn.last_insert_rowid())
    }

    /// List persisted offline mutations for `workspace` in FIFO order `(id, payload)`.
    pub fn list_offline(&self, workspace: &TeamId) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, payload FROM offline_queue WHERE workspace_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt.query_map(params![workspace.as_str()], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// Remove one persisted offline mutation by row id (after flush or user discard).
    pub fn delete_offline(&mut self, id: i64) -> Result<()> {
        self.ensure_writable()?;
        let mut stmt = self
            .conn
            .prepare_cached("DELETE FROM offline_queue WHERE id = ?1")?;
        stmt.execute(params![id])?;
        Ok(())
    }

    /// Remove all persisted offline mutations for `workspace`.
    pub fn clear_offline(&mut self, workspace: &TeamId) -> Result<()> {
        self.ensure_writable()?;
        let mut stmt = self
            .conn
            .prepare_cached("DELETE FROM offline_queue WHERE workspace_id = ?1")?;
        stmt.execute(params![workspace.as_str()])?;
        Ok(())
    }
}
