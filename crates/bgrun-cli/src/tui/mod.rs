//! Minimal Ratatui TUI: job rail, embedded PTY viewport, and overlays.

mod daemon;
mod keys;
mod settings;
mod state;
mod theme;
mod ui;

use std::path::PathBuf;

use anyhow::Result;
use crossterm::event::{
    DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture, Event,
    EventStream,
};
use futures_util::StreamExt;
use tokio::sync::mpsc;

use self::daemon::{ActiveStream, DaemonEvent};
use self::state::{App, Mode};

/// Enters the TUI. The daemon and all jobs keep running after quitting.
pub async fn run() -> Result<()> {
    let socket_path = bgrun_proto::paths::socket_path();
    crate::autostart::ensure_daemon_running(&socket_path).await?;

    let settings = settings::load().await;
    let mouse_enabled = settings.mouse;
    let mut terminal = ratatui::init();
    if mouse_enabled {
        // Mouse capture is optional; Settings can retry if the terminal rejects it.
        let _ = crossterm::execute!(std::io::stdout(), EnableMouseCapture);
    }
    let paste_enabled = crossterm::execute!(std::io::stdout(), EnableBracketedPaste).is_ok();

    let result = run_loop(&mut terminal, socket_path, settings).await;

    if paste_enabled {
        let _ = crossterm::execute!(std::io::stdout(), DisableBracketedPaste);
    }
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    socket_path: PathBuf,
    settings: settings::TuiSettings,
) -> Result<()> {
    let mut app = App::with_settings(settings);
    let mut events = EventStream::new();
    let (tx, mut rx) = mpsc::unbounded_channel::<DaemonEvent>();

    let poller = daemon::spawn_poller(socket_path.clone(), tx.clone(), app.poll_ctl.clone());
    let mut active_stream: Option<ActiveStream> = None;

    terminal.draw(|frame| ui::render(frame, &mut app))?;

    loop {
        tokio::select! {
            maybe_event = events.next() => match maybe_event {
                Some(Ok(Event::Key(key))) => {
                    if let Err(err) = keys::handle_key(
                        &mut app, key, active_stream.as_ref(), &socket_path, &tx,
                    ).await {
                        app.apply_daemon_event(DaemonEvent::Error(format!("{err:#}")));
                    }
                }
                Some(Ok(Event::Paste(text))) => {
                    if matches!(app.mode, Mode::PtyType { .. })
                        && let Some(stream) = &active_stream
                    {
                        stream.send_bytes(text.into_bytes());
                    }
                }
                // The next draw updates the canvas size and triggers ResizePty.
                Some(Ok(Event::Mouse(mouse))) if app.settings.mouse => {
                    keys::handle_mouse(&mut app, mouse);
                }
                Some(Ok(
                    Event::Resize(_, _)
                    | Event::FocusGained
                    | Event::FocusLost
                    | Event::Mouse(_),
                )) => {}
                Some(Err(err)) => {
                    app.apply_daemon_event(DaemonEvent::Error(err.to_string()));
                }
                None => break,
            },
            Some(daemon_event) = rx.recv() => app.apply_daemon_event(daemon_event),
        }

        if app.should_quit {
            break;
        }

        app.expire_toast();
        daemon::sync_active_stream(&mut app, &mut active_stream, &socket_path, &tx).await;
        terminal.draw(|frame| ui::render(frame, &mut app))?;
        daemon::sync_pty_size(&app, &mut active_stream, &socket_path);
    }

    poller.abort();
    if let Some(stream) = active_stream {
        stream.cancel();
    }
    Ok(())
}
