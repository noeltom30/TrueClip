use crate::{
    clipboard::SystemClipboard,
    history::History,
    service::{Command, Service, Snapshot},
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    thread::{self, JoinHandle},
};

type Publish = Arc<dyn Fn(Snapshot) + Send + Sync>;

#[derive(Clone)]
pub struct Commands {
    sender: SyncSender<Command>,
    wake: platform::Wake,
}

impl Commands {
    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        self.sender
            .try_send(command)
            .map_err(|_| "History worker is busy or unavailable. Try again.")?;
        self.wake
            .notify()
            .map_err(|_| "Could not notify the history worker.")
    }
}

pub struct Worker {
    pub commands: Commands,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    pub fn start(publish: impl Fn(Snapshot) + Send + Sync + 'static) -> std::io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(64);
        let (ready_sender, ready_receiver) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = stop.clone();
        let publish: Publish = Arc::new(publish);
        let thread = thread::Builder::new()
            .name("clipboard-history".into())
            .spawn(move || {
                // Initialize the native message queue before exposing the command sender.
                let wake = platform::Wake::current();
                if ready_sender.send(wake).is_err() {
                    return;
                }
                let mut service = match open_history().and_then(|history| {
                    SystemClipboard::new()
                        .map(|clipboard| {
                            let output = publish.clone();
                            Service::new(history, clipboard, move |snapshot| output(snapshot))
                        })
                        .map_err(|_| "Could not initialize clipboard access.".to_string())
                }) {
                    Ok(service) => Some(service),
                    Err(error) => {
                        report(&publish, error);
                        None
                    }
                };
                let listener = platform::Listener::new();
                if let Some(service) = service.as_mut() {
                    match &listener {
                        Ok(_) => {
                            if !service.capture() {
                                service
                                    .refresh("Ready. Copied text is saved while TrueClip is open.");
                            }
                        }
                        Err(error) => service.refresh(error.clone()),
                    }
                }
                loop {
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    match platform::next(&receiver) {
                        Ok(platform::Event::Capture) => {
                            if listener.is_ok()
                                && let Some(service) = service.as_mut()
                            {
                                service.capture();
                            }
                        }
                        Ok(platform::Event::Commands) => {
                            while let Ok(command) = receiver.try_recv() {
                                if worker_stop.load(Ordering::Acquire)
                                    || matches!(command, Command::Stop)
                                {
                                    return;
                                }
                                if let Some(service) = service.as_mut() {
                                    service.command(command);
                                }
                            }
                        }
                        #[cfg(not(windows))]
                        Ok(platform::Event::Command(command)) => {
                            if matches!(command, Command::Stop) {
                                break;
                            }
                            if let Some(service) = service.as_mut() {
                                service.command(command);
                            }
                        }
                        Err(error) => {
                            report(&publish, error);
                            break;
                        }
                    }
                }
                // Listener, clipboard, and SQLite are dropped on their owning thread.
            })?;
        let wake = ready_receiver
            .recv()
            .map_err(|_| std::io::Error::other("Clipboard worker could not start"))?;
        Ok(Self {
            commands: Commands { sender, wake },
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.commands.sender.try_send(Command::Stop);
        let _ = self.commands.wake.notify();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn open_history() -> Result<History, String> {
    let directory = std::env::var_os("TRUECLIP_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| directories::BaseDirs::new().map(|dirs| dirs.data_local_dir().join("TrueClip")))
        .ok_or_else(|| "Could not locate the local application data folder.".to_string())?;
    std::fs::create_dir_all(&directory)
        .map_err(|_| "Could not create the history folder.".to_string())?;
    History::open(&directory.join("history.sqlite3"))
        .map_err(|_| "Could not open the history database. The existing file has been preserved. Check its permissions or restore a backup, then restart TrueClip.".to_string())
}

fn report(publish: &Publish, status: String) {
    publish(Snapshot {
        rows: vec![],
        selected: None,
        text: String::new(),
        status,
    });
}

#[cfg(windows)]
mod platform {
    use super::*;
    use windows_sys::{
        Win32::{
            Foundation::HWND,
            System::{
                DataExchange::{AddClipboardFormatListener, RemoveClipboardFormatListener},
                Threading::GetCurrentThreadId,
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DestroyWindow, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG,
                PM_NOREMOVE, PeekMessageW, PostThreadMessageW, WM_APP, WM_CLIPBOARDUPDATE,
            },
        },
        w,
    };

    #[derive(Clone, Copy)]
    pub struct Wake(u32);
    impl Wake {
        pub fn current() -> Self {
            // SAFETY: MSG is initialized; PeekMessage creates this thread's queue.
            unsafe {
                let mut message: MSG = std::mem::zeroed();
                PeekMessageW(&mut message, std::ptr::null_mut(), 0, 0, PM_NOREMOVE);
                Self(GetCurrentThreadId())
            }
        }
        pub fn notify(self) -> Result<(), ()> {
            // SAFETY: Posting an integer message transfers no pointers or ownership.
            if unsafe { PostThreadMessageW(self.0, WM_APP, 0, 0) } == 0 {
                Err(())
            } else {
                Ok(())
            }
        }
    }

    pub struct Listener(HWND);
    impl Listener {
        pub fn new() -> Result<Self, String> {
            // SAFETY: STATIC is a built-in window class. This message-only window is created,
            // used, and destroyed on this worker thread. No borrowed pointers are retained.
            unsafe {
                let window = CreateWindowExW(
                    0,
                    w!("STATIC"),
                    w!("TrueClip listener"),
                    0,
                    0,
                    0,
                    0,
                    0,
                    HWND_MESSAGE,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                );
                if window.is_null() {
                    return Err("Could not create the clipboard listener.".into());
                }
                if AddClipboardFormatListener(window) == 0 {
                    DestroyWindow(window);
                    return Err("Could not listen for clipboard changes.".into());
                }
                Ok(Self(window))
            }
        }
    }
    impl Drop for Listener {
        fn drop(&mut self) {
            // SAFETY: This live handle belongs to the current thread; cleanup occurs once.
            unsafe {
                RemoveClipboardFormatListener(self.0);
                DestroyWindow(self.0);
            }
        }
    }

    pub enum Event {
        Capture,
        Commands,
    }
    pub fn next(_: &Receiver<Command>) -> Result<Event, String> {
        // SAFETY: MSG is valid for GetMessage/DispatchMessage. Windows owns message data.
        unsafe {
            let mut message: MSG = std::mem::zeroed();
            loop {
                if GetMessageW(&mut message, std::ptr::null_mut(), 0, 0) <= 0 {
                    return Err("Clipboard listener stopped unexpectedly.".into());
                }
                match message.message {
                    WM_CLIPBOARDUPDATE => return Ok(Event::Capture),
                    WM_APP => return Ok(Event::Commands),
                    _ => {
                        DispatchMessageW(&message);
                    }
                }
            }
        }
    }

    #[test]
    fn native_listener_wakes_and_shuts_down_without_clipboard_access() {
        let (ready_tx, ready_rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let wake = Wake::current();
            let _listener = Listener::new().unwrap();
            ready_tx.send(wake).unwrap();
            let (_tx, rx) = mpsc::sync_channel(1);
            loop {
                if matches!(next(&rx).unwrap(), Event::Commands) {
                    break;
                }
            }
        });
        ready_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .unwrap()
            .notify()
            .unwrap();
        thread.join().unwrap();
    }
}

#[cfg(not(windows))]
mod platform {
    use super::*;
    #[derive(Clone, Copy)]
    pub struct Wake;
    impl Wake {
        pub fn current() -> Self {
            Self
        }
        pub fn notify(self) -> Result<(), ()> {
            Ok(())
        }
    }
    pub struct Listener;
    impl Listener {
        pub fn new() -> Result<Self, String> {
            Err("Automatic clipboard capture is currently supported only on Windows.".into())
        }
    }
    #[allow(dead_code)]
    pub enum Event {
        Capture,
        Commands,
        Command(Command),
    }
    pub fn next(receiver: &Receiver<Command>) -> Result<Event, String> {
        receiver
            .recv()
            .map(Event::Command)
            .map_err(|_| "History worker disconnected.".into())
    }
}
