// Keep the console in development, but use a normal GUI executable on Windows in release builds.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

use slint::ComponentHandle;

mod clipboard;
mod history;
mod service;
mod worker;

slint::include_modules!();

fn main() -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    let weak = window.as_weak();
    let worker = worker::Worker::start(move |snapshot| {
        let _ = weak.upgrade_in_event_loop(move |window| {
            use chrono::{Local, TimeZone};
            let rows: Vec<HistoryRow> = snapshot
                .rows
                .into_iter()
                .map(|row| HistoryRow {
                    id: row.id.to_string().into(),
                    preview: row.preview.into(),
                    timestamp: Local
                        .timestamp_millis_opt(row.copied_at)
                        .single()
                        .map(|time| time.format("%d %b %Y · %H:%M:%S").to_string())
                        .unwrap_or_else(|| "Unknown time".into())
                        .into(),
                })
                .collect();
            window.set_history(slint::ModelRc::new(slint::VecModel::from(rows)));
            window.set_selected_id(
                snapshot
                    .selected
                    .map(|id| id.to_string())
                    .unwrap_or_default()
                    .into(),
            );
            window.set_selected_text(snapshot.text.into());
            window.set_status(snapshot.status.into());
        });
    });
    match worker {
        Ok(worker) => {
            window.on_select_entry(callback(
                &window,
                worker.commands.clone(),
                service::Command::Select,
            ));
            window.on_copy_entry(callback(
                &window,
                worker.commands.clone(),
                service::Command::Copy,
            ));
            window.on_delete_entry(callback(
                &window,
                worker.commands.clone(),
                service::Command::Delete,
            ));
            let result = window.run();
            drop(worker);
            result
        }
        Err(_) => {
            window.set_status("Could not start the history worker.".into());
            window.run()
        }
    }
}

fn callback(
    window: &AppWindow,
    commands: worker::Commands,
    command: fn(i64) -> service::Command,
) -> impl Fn(slint::SharedString) + 'static {
    let weak = window.as_weak();
    move |id| {
        if let Ok(id) = id.parse::<i64>()
            && let Err(error) = commands.send(command(id))
            && let Some(window) = weak.upgrade()
        {
            window.set_status(error.into());
        }
    }
}
