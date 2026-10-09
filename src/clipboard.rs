use std::{thread, time::Duration};

pub trait TextClipboard {
    fn read(&mut self) -> Result<(u32, String), arboard::Error>;
    fn write(&mut self, text: &str) -> Result<u32, arboard::Error>;
}

pub struct SystemClipboard(arboard::Clipboard);

impl SystemClipboard {
    pub fn new() -> Result<Self, arboard::Error> {
        arboard::Clipboard::new().map(Self)
    }
}

fn sequence() -> u32 {
    #[cfg(windows)]
    // SAFETY: This API has no pointer arguments or ownership requirements.
    unsafe {
        windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber()
    }
    #[cfg(not(windows))]
    {
        0
    }
}

impl TextClipboard for SystemClipboard {
    fn read(&mut self) -> Result<(u32, String), arboard::Error> {
        // On Windows the builder holds the clipboard open, making text and sequence one snapshot.
        let get = self.0.get();
        let sequence = sequence();
        get.text().map(|text| (sequence, text))
    }

    fn write(&mut self, text: &str) -> Result<u32, arboard::Error> {
        self.0.set_text(text)?;
        Ok(sequence())
    }
}

pub fn read_with_retry<C: TextClipboard>(
    clipboard: &mut C,
) -> Result<Option<(u32, String)>, arboard::Error> {
    retry(|| clipboard.read(), thread::sleep)
        .map(Some)
        .or_else(|error| {
            if matches!(error, arboard::Error::ContentNotAvailable) {
                Ok(None)
            } else {
                Err(error)
            }
        })
}

pub fn write_with_retry<C: TextClipboard>(
    clipboard: &mut C,
    text: &str,
) -> Result<u32, arboard::Error> {
    retry(|| clipboard.write(text), thread::sleep)
}

fn retry<T>(
    mut operation: impl FnMut() -> Result<T, arboard::Error>,
    mut sleep: impl FnMut(Duration),
) -> Result<T, arboard::Error> {
    for delay in [25, 50, 100] {
        match operation() {
            Err(arboard::Error::ClipboardOccupied) => sleep(Duration::from_millis(delay)),
            result => return result,
        }
    }
    operation()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn busy_retries_are_bounded_and_can_recover() {
        let mut attempts = 0;
        let mut delays = vec![];
        let result = retry(
            || {
                attempts += 1;
                if attempts < 3 {
                    Err(arboard::Error::ClipboardOccupied)
                } else {
                    Ok(42)
                }
            },
            |delay| delays.push(delay.as_millis()),
        );
        assert_eq!(result.unwrap(), 42);
        assert_eq!(delays, [25, 50]);
        attempts = 0;
        delays.clear();
        let result: Result<(), _> = retry(
            || {
                attempts += 1;
                Err(arboard::Error::ClipboardOccupied)
            },
            |delay| delays.push(delay.as_millis()),
        );
        assert!(result.is_err());
        assert_eq!(attempts, 4);
        assert_eq!(delays, [25, 50, 100]);
    }

    #[test]
    fn other_errors_are_not_retried() {
        let result: Result<(), _> = retry(
            || Err(arboard::Error::ConversionFailure),
            |_| panic!("unexpected retry"),
        );
        assert!(result.is_err());
    }
}
