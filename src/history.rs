use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::{path::Path, time::Duration};

pub const MAX_ENTRIES: usize = 200;
pub const MAX_TEXT_BYTES: usize = 1024 * 1024;
const PREVIEW_CHARS: usize = 100;

#[derive(Clone, Debug)]
pub struct Row {
    pub id: i64,
    pub copied_at: i64,
    pub preview: String,
}

#[derive(Debug, PartialEq)]
pub enum SaveOutcome {
    Saved,
    Empty,
    TooLarge,
}

pub struct History {
    connection: Connection,
}

impl History {
    pub fn open(path: &Path) -> rusqlite::Result<Self> {
        let connection = Connection::open(path)?;
        Self::initialize(connection)
    }

    fn initialize(connection: Connection) -> rusqlite::Result<Self> {
        connection.busy_timeout(Duration::from_millis(250))?;
        // Read before writing: a corrupt file must fail without being replaced.
        let _: i64 = connection.query_row("PRAGMA schema_version", [], |r| r.get(0))?;
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                text TEXT NOT NULL UNIQUE COLLATE BINARY,
                copied_at INTEGER NOT NULL,
                recency INTEGER NOT NULL
            );",
        )?;
        // Also validate existing schemas before accepting captures.
        connection.prepare("SELECT id, text, copied_at, recency FROM history LIMIT 0")?;
        Ok(Self { connection })
    }

    pub fn save(&mut self, text: &str, copied_at: i64) -> rusqlite::Result<SaveOutcome> {
        if text.is_empty() {
            return Ok(SaveOutcome::Empty);
        }
        if text.len() > MAX_TEXT_BYTES {
            return Ok(SaveOutcome::TooLarge);
        }
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let recency: i64 = tx.query_row(
            "SELECT COALESCE(MAX(recency), 0) + 1 FROM history",
            [],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO history(text, copied_at, recency) VALUES (?1, ?2, ?3)
             ON CONFLICT(text) DO UPDATE SET copied_at = excluded.copied_at,
                                             recency = excluded.recency",
            params![text, copied_at, recency],
        )?;
        tx.execute(
            "DELETE FROM history WHERE id NOT IN
             (SELECT id FROM history ORDER BY recency DESC LIMIT ?1)",
            [MAX_ENTRIES as i64],
        )?;
        tx.commit()?;
        Ok(SaveOutcome::Saved)
    }

    pub fn rows(&self) -> rusqlite::Result<Vec<Row>> {
        // SQLite counts Unicode characters, not bytes. Full strings never enter the list model.
        let mut statement = self.connection.prepare(
            "SELECT id, copied_at, substr(text, 1, ?1), length(text) FROM history
             ORDER BY recency DESC LIMIT ?2",
        )?;
        statement
            .query_map(params![PREVIEW_CHARS as i64, MAX_ENTRIES as i64], |r| {
                let fragment: String = r.get(2)?;
                let length: i64 = r.get(3)?;
                let mut preview = fragment.split_whitespace().collect::<Vec<_>>().join(" ");
                if preview.is_empty() {
                    preview = "(Whitespace)".into();
                }
                if length > PREVIEW_CHARS as i64 {
                    preview.push('…');
                }
                Ok(Row {
                    id: r.get(0)?,
                    copied_at: r.get(1)?,
                    preview,
                })
            })?
            .collect()
    }

    pub fn text(&self, id: i64) -> rusqlite::Result<Option<String>> {
        self.connection
            .query_row("SELECT text FROM history WHERE id = ?1", [id], |r| r.get(0))
            .optional()
    }

    pub fn delete(&self, id: i64) -> rusqlite::Result<()> {
        self.connection
            .execute("DELETE FROM history WHERE id = ?1", [id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub fn memory() -> History {
        History::initialize(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn exact_round_trip_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let text = "  नमस्ते 🦀\r\nhttps://example.test\n\tfn main() {}  ";
        let id;
        {
            let mut history = History::open(&path).unwrap();
            history.save(text, 123).unwrap();
            id = history.rows().unwrap()[0].id;
            assert_eq!(history.text(id).unwrap().as_deref(), Some(text));
        }
        let history = History::open(&path).unwrap();
        assert_eq!(history.rows().unwrap()[0].copied_at, 123);
        assert_eq!(history.text(id).unwrap().as_deref(), Some(text));
        history.delete(id).unwrap();
        assert!(history.rows().unwrap().is_empty());
    }

    #[test]
    fn duplicates_keep_id_and_order_even_with_equal_or_backward_clock() {
        let mut history = memory();
        history.save("first", 100).unwrap();
        let first_id = history.rows().unwrap()[0].id;
        history.save("second", 100).unwrap();
        history.save("first", 50).unwrap();
        let rows = history.rows().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, first_id);
        assert_eq!(rows[0].copied_at, 50);
    }

    #[test]
    fn retention_removes_least_recently_copied() {
        let mut history = memory();
        for n in 0..MAX_ENTRIES {
            history.save(&n.to_string(), 0).unwrap();
        }
        history.save("0", 0).unwrap();
        history.save("200", 0).unwrap();
        let rows = history.rows().unwrap();
        assert_eq!(rows.len(), MAX_ENTRIES);
        assert_eq!(rows[0].preview, "200");
        assert!(rows.iter().any(|r| r.preview == "0"));
        assert!(!rows.iter().any(|r| r.preview == "1"));
    }

    #[test]
    fn empty_whitespace_unicode_and_size_boundary() {
        let mut history = memory();
        assert_eq!(history.save("", 0).unwrap(), SaveOutcome::Empty);
        history.save(" \n\t", 0).unwrap();
        assert_eq!(history.rows().unwrap()[0].preview, "(Whitespace)");
        history.save(&"🦀".repeat(101), 0).unwrap();
        assert_eq!(
            history.rows().unwrap()[0].preview,
            format!("{}…", "🦀".repeat(100))
        );
        assert_eq!(
            history.save(&"x".repeat(MAX_TEXT_BYTES), 0).unwrap(),
            SaveOutcome::Saved
        );
        assert_eq!(
            history.save(&"x".repeat(MAX_TEXT_BYTES + 1), 0).unwrap(),
            SaveOutcome::TooLarge
        );
        assert_eq!(history.rows().unwrap().len(), 3);
    }

    #[test]
    fn failed_transaction_rolls_back_and_corrupt_file_is_preserved() {
        let mut history = memory();
        for n in 0..MAX_ENTRIES {
            history.save(&n.to_string(), 0).unwrap();
        }
        history
            .connection
            .execute_batch(
                "CREATE TRIGGER fail_delete BEFORE DELETE ON history BEGIN
             SELECT RAISE(ABORT, 'controlled failure'); END;",
            )
            .unwrap();
        assert!(history.save("new", 0).is_err());
        let rows = history.rows().unwrap();
        assert_eq!(rows.len(), MAX_ENTRIES);
        assert!(!rows.iter().any(|r| r.preview == "new"));
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite3");
        let bytes = b"this is deliberately not a database";
        std::fs::write(&path, bytes).unwrap();
        assert!(History::open(&path).is_err());
        assert_eq!(std::fs::read(path).unwrap(), bytes);
    }
}
