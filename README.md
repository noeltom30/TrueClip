# TrueClip

A desktop text clipboard history built with Rust and Slint. On Windows it captures
the current text when opened, then automatically saves copied text while the window
is open. Closing the window stops capture and exits the worker and app.

Select a history row to read its full text. **Copy** restores that exact text to the
clipboard so you can paste it in another app; **Delete** removes the saved entry
without changing your clipboard. Copying through TrueClip does not reorder history.
New external copies preserve your selection; duplicate text moves its existing row
to the top. The latest **200 unique entries** survive restarts.

Links and code are plain text. Empty and non-text contents are ignored; whitespace,
Unicode, and line breaks are preserved. Text over **1 MiB of UTF-8** is skipped with
a status message, never truncated. Images, search, categories, pins, tray behavior,
global shortcuts, and automatic pasting are future steps.

## Local storage

The database is `%LOCALAPPDATA%\TrueClip\history.sqlite3` on Windows. History is
stored locally as **unencrypted text**, including anything sensitive that you copy
while the app is open. The app does not upload history or log copied content.
Retention limits stored entries; deleting an entry is not a secure file wipe.

Capture, Copy, and storage failures appear in the status line. An unreadable database
is preserved rather than replaced. Check permissions or restore a backup and restart
after fixing it. Close TrueClip before making a database backup.

For isolated development checks, set `TRUECLIP_DATA_DIR` to a separate directory
before launch; `history.sqlite3` will be created there instead of your normal data
folder. For example:

```powershell
$env:TRUECLIP_DATA_DIR = "$PWD\.cache\manual-history"
cargo run
Remove-Item Env:TRUECLIP_DATA_DIR
```

## Prerequisites

- Rust 1.92 or newer, installed with [rustup](https://rustup.rs/).
- On Windows, the MSVC Rust toolchain and Visual Studio's **Desktop development
  with C++** workload, including a Windows SDK.
- Internet access for Cargo to download dependencies on the first build.

Rust 1.99 and the MSVC tools were detected on the development machine. Validation
targets Windows first; macOS and Linux have not been checked. Node.js, npm, Qt,
and a browser runtime are not required for this configuration.

## Run and check

Run these commands from the project folder:

```powershell
cargo run
cargo fmt --all -- --check
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
cargo build --release --locked
```

The first dependency build can take several minutes. Subsequent builds reuse
Cargo's local build output. Keep `Cargo.lock` in version control: it records the
dependency versions used by the application. A development build keeps its
console so errors are visible.

The Windows release executable is `target/release/trueclip.exe`. Starting this
executable from File Explorer opens only the application window, without an extra
console window. No installer is included yet.

## Validation

Windows is the supported capture target for this step. Other platforms report that
automatic capture is unsupported; their builds and clipboard behavior are unverified.

Automated checks cover exact text round-tripping, duplicate ordering and stable IDs,
reopening, deletion, retention, Unicode previews, empty text, the size boundary,
bounded retries, transaction rollback, capture/copy failures, self-copy suppression,
stable selection and expiration, and native listener wake/shutdown.

On 9 October 2026, formatting, compilation, Clippy with warnings denied, all
**12 tests**, and the release build passed with Rust 1.99 and MSVC.

The Windows release smoke check uses synthetic text and a separate database, preserves
the original clipboard in memory, and restores it afterward. It covers startup and
automatic capture, read-only detail, Copy/native paste, selection, self-copy ordering,
deletion, persistence after restart, empty-state buttons, oversized text, resizing,
and exit. This check passed on Windows at 175% display scaling. Screenshots at
initial, minimum, larger, and empty states were inspected for readable contrast.
Initial dimensions are 640 × 420, minimum 480 × 320 logical pixels.

The updated Windows release executable is **15,166,976 bytes (14.46 MiB)**;
the welcome-window baseline was **10,838,016 bytes (10.34 MiB)**. Executable size
is disk size, not RAM use, startup time, UI latency, or installer footprint; those
performance measurements remain unverified.

## How the files connect

| File | Role |
| --- | --- |
| `Cargo.toml` | Names the application, selects dependencies, and configures release builds. |
| `build.rs` | Compiles the Slint interface into Rust code during the build. |
| `ui/app-window.slint` | Defines the virtualized history list, read-only detail, buttons, and status. |
| `src/main.rs` | Connects callbacks and applies worker snapshots on Slint's UI thread. |
| `src/clipboard.rs` | Uses arboard for exact text reads/writes and bounded busy retries. |
| `src/history.rs` | SQLite schema, previews, full text, duplicate handling, retention, and deletion. |
| `src/service.rs` | Coordinates capture, selection, Copy/Delete, errors, and self-copy suppression. |
| `src/worker.rs` | Owns the worker, data path, Windows notification listener, and clean shutdown. |
| `Cargo.lock` | Records the exact resolved dependency versions. |
| `docs/decisions.md` | Short explanations of the storage, threading, and UI choices. |

Slint's **event loop** waits for window events such as resizing and closing.
Closing the only window ends that loop; `main` stops and joins the worker. A
message-only Windows window receives native clipboard-change notifications. UI
commands wake the same worker through Windows messages and a bounded Rust channel.
That thread serializes clipboard operations and SQLite access; Slint receives its
results through `upgrade_in_event_loop`. No clipboard polling timer is used.

The list reads short previews instead of loading every full entry. Slint's `ListView`
instantiates visible rows as you scroll. IDs keep selection stable, and a separate
recency counter keeps newest-first ordering correct when timestamps tie or the
system clock moves backward. Rapid clipboard replacements can overwrite intermediate
values before the listener reads them.

Slint's default features are disabled. We explicitly use Winit for desktop
window integration, the software renderer to draw the interface, and native
accessibility integration. Only one renderer is compiled; Qt, GPU renderers,
and tray integration are not enabled. This is a starting configuration, not a
measured RAM or speed guarantee. Release builds use thin link-time optimization,
one code-generation unit, and stripped debug information.

## Editing the interface

Edit `ui/app-window.slint` and run `cargo run` again. Slint recompiles the interface
as part of the Cargo build. For editor support, install the **Slint** extension
and **rust-analyzer** in VS Code. The Slint extension also provides a live preview;
preview tooling is separate from the shipped application.

Try changing the title's color or font size to see how UI properties affect the
window. Try copying multiline synthetic text, selecting an older row, and copying
another value: the list updates while your selected detail stays in place.

## Attribution and references

Powered by [Slint](https://slint.dev/).

- [Slint Rust integration](https://docs.slint.dev/latest/docs/rust/slint/)
- [Slint feature flags and renderer selection](https://docs.slint.dev/latest/docs/rust/slint/docs/cargo_features/)
- [Slint licensing](https://slint.dev/license/)
- [arboard text clipboard API](https://docs.rs/arboard/3.6.1/arboard/struct.Clipboard.html)
- [rusqlite / SQLite integration](https://docs.rs/rusqlite/0.40.2/rusqlite/)
- [Windows clipboard-change notifications](https://learn.microsoft.com/en-us/windows/win32/api/winuser/nf-winuser-addclipboardformatlistener)

The personal `learning.md` journal, Cargo build output, and local caches are
ignored by Git. `AGENTS.md` contains the project's collaboration instructions.
