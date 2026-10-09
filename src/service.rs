use crate::{
    clipboard::{TextClipboard, read_with_retry, write_with_retry},
    history::{History, Row, SaveOutcome},
};

#[derive(Clone)]
pub enum Command {
    Select(i64),
    Copy(i64),
    Delete(i64),
    Stop,
}

pub struct Snapshot {
    pub rows: Vec<Row>,
    pub selected: Option<i64>,
    pub text: String,
    pub status: String,
}

// Single owner: clipboard and connection are used only on the worker thread.
pub struct Service<C: TextClipboard> {
    history: History,
    clipboard: C,
    selected: Option<i64>,
    last_sequence: Option<u32>,
    self_copy: Option<(u32, String)>,
    publish: Box<dyn Fn(Snapshot) + Send>,
}

impl<C: TextClipboard> Service<C> {
    pub fn new(
        history: History,
        clipboard: C,
        publish: impl Fn(Snapshot) + Send + 'static,
    ) -> Self {
        Self {
            history,
            clipboard,
            selected: None,
            last_sequence: None,
            self_copy: None,
            publish: Box::new(publish),
        }
    }

    pub fn refresh(&mut self, status: impl Into<String>) {
        let mut status = status.into();
        let rows = match self.history.rows() {
            Ok(rows) => rows,
            Err(_) => {
                (self.publish)(Snapshot {
                    rows: vec![],
                    selected: None,
                    text: String::new(),
                    status: "Could not load history. The database has been preserved.".into(),
                });
                return;
            }
        };
        if !rows.iter().any(|row| Some(row.id) == self.selected) {
            self.selected = rows.first().map(|row| row.id);
        }
        let text = match self.selected.map(|id| self.history.text(id)).transpose() {
            Ok(Some(Some(text))) => text,
            Ok(_) => String::new(),
            Err(_) => {
                self.selected = None;
                status = "Could not read the selected entry.".into();
                String::new()
            }
        };
        (self.publish)(Snapshot {
            rows,
            selected: self.selected,
            text,
            status,
        });
    }

    pub fn capture(&mut self) -> bool {
        match read_with_retry(&mut self.clipboard) {
            Ok(Some((sequence, text))) => {
                if self.last_sequence == Some(sequence) {
                    return false;
                }
                self.last_sequence = Some(sequence);
                if self
                    .self_copy
                    .as_ref()
                    .is_some_and(|(own_sequence, own_text)| {
                        *own_sequence == sequence && own_text == &text
                    })
                {
                    self.self_copy = None;
                    return false;
                }
                self.self_copy = None;
                match self
                    .history
                    .save(&text, chrono::Utc::now().timestamp_millis())
                {
                    Ok(SaveOutcome::Saved) => self.refresh("Saved copied text."),
                    Ok(SaveOutcome::TooLarge) => {
                        self.refresh("Skipped text larger than 1 MiB. Nothing was truncated.")
                    }
                    Ok(SaveOutcome::Empty) => return false,
                    Err(_) => {
                        self.refresh("Could not save copied text. It was not added to history.")
                    }
                }
            }
            Ok(None) => return false,
            Err(_) => self.refresh(
                "Could not read the clipboard after bounded retries. Copy again to retry.",
            ),
        }
        true
    }

    pub fn command(&mut self, command: Command) -> bool {
        match command {
            Command::Stop => return false,
            Command::Select(id) => {
                self.selected = Some(id);
                self.refresh("Ready.");
            }
            Command::Copy(id) => match self.history.text(id) {
                Ok(Some(text)) => match write_with_retry(&mut self.clipboard, &text) {
                    Ok(sequence) => {
                        self.self_copy = Some((sequence, text));
                        self.refresh("Copied to clipboard. Paste it wherever you need it.");
                    }
                    Err(_) => self.refresh("Could not copy to the clipboard. Try again."),
                },
                Ok(None) => self.refresh("That entry is no longer in history."),
                Err(_) => self.refresh("Could not read the saved text. Clipboard unchanged."),
            },
            Command::Delete(id) => match self.history.delete(id) {
                Ok(()) => self.refresh("Deleted saved entry. Clipboard unchanged."),
                Err(_) => self.refresh("Could not delete the saved entry."),
            },
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct FakeClipboard {
        sequence: u32,
        text: String,
        fail_write: bool,
        fail_read: bool,
        non_text: bool,
    }
    impl TextClipboard for FakeClipboard {
        fn read(&mut self) -> Result<(u32, String), arboard::Error> {
            if self.fail_read {
                return Err(arboard::Error::ConversionFailure);
            }
            if self.non_text {
                Err(arboard::Error::ContentNotAvailable)
            } else {
                Ok((self.sequence, self.text.clone()))
            }
        }
        fn write(&mut self, text: &str) -> Result<u32, arboard::Error> {
            if self.fail_write {
                return Err(arboard::Error::ConversionFailure);
            }
            self.sequence += 1;
            self.text = text.into();
            Ok(self.sequence)
        }
    }
    fn fixture() -> (Service<FakeClipboard>, Arc<Mutex<Vec<Snapshot>>>) {
        let history = History::open(&std::path::PathBuf::from(":memory:")).unwrap();
        let snapshots = Arc::new(Mutex::new(vec![]));
        let output = snapshots.clone();
        (
            Service::new(
                history,
                FakeClipboard {
                    sequence: 1,
                    text: "first".into(),
                    fail_write: false,
                    fail_read: false,
                    non_text: false,
                },
                move |snapshot| output.lock().unwrap().push(snapshot),
            ),
            snapshots,
        )
    }
    fn external(service: &mut Service<FakeClipboard>, text: &str) {
        service.clipboard.sequence += 1;
        service.clipboard.text = text.into();
        service.capture();
    }

    #[test]
    fn selection_self_copy_external_duplicates_delete_and_stop() {
        let (mut service, snapshots) = fixture();
        service.capture();
        let first_id = service.selected.unwrap();
        external(&mut service, "second");
        assert_eq!(service.selected, Some(first_id));
        service.command(Command::Copy(first_id));
        service.capture();
        assert_eq!(service.history.rows().unwrap()[0].preview, "second");
        assert_eq!(service.clipboard.text, "first");
        external(&mut service, "first");
        assert_eq!(service.history.rows().unwrap()[0].id, first_id);
        let clipboard_before = service.clipboard.text.clone();
        service.command(Command::Delete(first_id));
        assert_ne!(service.selected, Some(first_id));
        assert_eq!(service.clipboard.text, clipboard_before);
        assert!(!service.command(Command::Stop));
        assert!(
            snapshots
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .status
                .starts_with("Deleted")
        );
    }

    #[test]
    fn expired_selection_selects_newest() {
        let (mut service, _) = fixture();
        service.capture();
        let first_id = service.selected;
        for n in 0..200 {
            external(&mut service, &format!("synthetic {n}"));
        }
        assert_ne!(service.selected, first_id);
        assert_eq!(
            service.selected,
            Some(service.history.rows().unwrap()[0].id)
        );
    }

    #[test]
    fn failed_save_and_copy_are_reported_without_success() {
        let (mut service, snapshots) = fixture();
        service.capture();
        // A second connection holds an exclusive lock to induce a real storage failure.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite3");
        let mut writable = History::open(&path).unwrap();
        writable.save("saved", 0).unwrap();
        let lock = rusqlite::Connection::open(&path).unwrap();
        service.history = writable;
        lock.execute_batch("BEGIN EXCLUSIVE").unwrap();
        external(&mut service, "cannot save");
        // Loading fails too under this lock: neither status can claim a save.
        assert!(
            snapshots
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .status
                .starts_with("Could not")
        );
        lock.execute_batch("ROLLBACK").unwrap();
        assert_eq!(service.history.rows().unwrap().len(), 1);
        service.refresh("Ready.");
        service.clipboard.fail_write = true;
        service.command(Command::Copy(service.selected.unwrap()));
        assert!(
            snapshots
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .status
                .starts_with("Could not copy")
        );
        service.clipboard.non_text = true;
        service.capture();
        assert_eq!(service.history.rows().unwrap().len(), 1);
        service.clipboard.fail_read = true;
        service.capture();
        assert!(
            snapshots
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .status
                .starts_with("Could not read the clipboard")
        );
    }

    #[test]
    fn external_text_racing_a_copy_is_not_suppressed() {
        let (mut service, _) = fixture();
        service.capture();
        service.command(Command::Copy(service.selected.unwrap()));
        // Simulate observing an external sequence immediately after our write.
        // A sequence alone is insufficient; exact content must match too.
        service.clipboard.text = "external replacement".into();
        service.capture();
        assert_eq!(
            service.history.rows().unwrap()[0].preview,
            "external replacement"
        );
    }
}
