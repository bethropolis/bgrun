//! Key and mouse handling, PTY byte encoding, and command actions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use bgrun_proto::{Command, JobRecord, KillArgs, LogDigest, RunArgs, TailArgs};
use crossterm::event::{
    DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    MouseButton, MouseEvent, MouseEventKind,
};
use ratatui::layout::Position;
use ratatui::widgets::ListState;
use tokio::sync::mpsc;
use tui_input::Input;
use tui_input::backend::crossterm::EventHandler;

use super::daemon::{ActiveStream, DaemonEvent, fetch_jobs, rpc, rpc_autostart};
use super::settings::SettingsRow;
use super::state::{App, Mode, OverlayKind, PaletteAction, PromptKind};

const PALETTE_ITEMS: [(&str, PaletteAction); 12] = [
    ("View digest", PaletteAction::ViewDigest),
    ("Search logs", PaletteAction::SearchLogs),
    ("Inspect job", PaletteAction::InspectJob),
    ("Export log", PaletteAction::ExportLog),
    ("New job", PaletteAction::NewJob),
    ("Filter workspace", PaletteAction::FilterWorkspace),
    ("Restart job", PaletteAction::RestartJob),
    ("Stop job", PaletteAction::StopJob),
    ("Kill job", PaletteAction::KillJob),
    ("Clean terminated", PaletteAction::CleanTerminated),
    ("Toggle sidebar", PaletteAction::ToggleSidebar),
    ("Settings", PaletteAction::Settings),
];

/// Returns the fuzzy-filtered palette entries for the current input.
pub fn palette_actions(app: &App) -> Vec<(&'static str, PaletteAction)> {
    let query = app.palette_input.value().trim().to_lowercase();
    PALETTE_ITEMS
        .into_iter()
        .filter(|(label, _)| fuzzy_match(&query, &label.to_lowercase()))
        .collect()
}

/// Subsequence match: every query character must appear in order.
fn fuzzy_match(query: &str, candidate: &str) -> bool {
    let mut chars = candidate.chars();
    query.chars().all(|c| chars.any(|candidate| candidate == c))
}

pub async fn handle_key(
    app: &mut App,
    key: KeyEvent,
    active: Option<&ActiveStream>,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    // Kitty's keyboard protocol emits Release events; acting on them would
    // double-send characters. In typing mode, repeats are meaningful.
    let allow_repeat = matches!(app.mode, Mode::PtyType { .. }) && key.kind == KeyEventKind::Repeat;
    if key.kind != KeyEventKind::Press && !allow_repeat {
        return Ok(());
    }

    match app.mode {
        Mode::PtyType { .. } => {
            if let Some(action) = pty_key_action(app, key, app.parser.screen().application_cursor())
                && let Some(stream) = active
                && let PtyKeyAction::Forward(bytes) = action
            {
                stream.send_bytes(bytes);
            }
            Ok(())
        }
        Mode::Palette => handle_palette_key(app, key, socket_path, tx).await,
        Mode::Prompt(kind) => handle_prompt_key(app, key, kind, socket_path, tx).await,
        Mode::Overlay(kind) => {
            let path = super::settings::settings_path();
            handle_overlay_key(app, key, kind, &path).await
        }
        Mode::Normal => handle_normal_key(app, key, socket_path, tx).await,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PtyKeyAction {
    Pending,
    Detach,
    Forward(Vec<u8>),
}

fn pty_key_action(app: &mut App, key: KeyEvent, app_cursor: bool) -> Option<PtyKeyAction> {
    let pending = matches!(
        app.mode,
        Mode::PtyType {
            ctrl_p_pending: true
        }
    );
    let is_ctrl_p = key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('p');
    let is_ctrl_q = key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('q');

    if pending {
        if is_ctrl_q {
            app.mode = Mode::Normal;
            app.set_toast("detached — job still running");
            return Some(PtyKeyAction::Detach);
        }
        // The chord was not completed. Flush the pending Ctrl+P before the
        // current key and clear the chord so the next key is independent.
        app.mode = Mode::PtyType {
            ctrl_p_pending: false,
        };
        let mut bytes = vec![0x10];
        if let Some(current) = key_event_to_bytes(key, app_cursor) {
            bytes.extend(current);
        }
        return Some(PtyKeyAction::Forward(bytes));
    }

    if is_ctrl_p {
        app.mode = Mode::PtyType {
            ctrl_p_pending: true,
        };
        return Some(PtyKeyAction::Pending);
    }
    key_event_to_bytes(key, app_cursor).map(PtyKeyAction::Forward)
}

/// Translates a crossterm key event into terminal byte sequences.
pub fn key_event_to_bytes(key: KeyEvent, app_cursor: bool) -> Option<Vec<u8>> {
    use KeyCode::{
        BackTab, Backspace, Char, Delete, Down, End, Enter, Esc, F, Home, Insert, Left, PageDown,
        PageUp, Right, Tab, Up,
    };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);

    let mut out = Vec::new();
    if alt {
        out.push(0x1b);
    }

    match key.code {
        Char(c) if ctrl => {
            let byte = (c.to_ascii_lowercase() as u8)
                .wrapping_sub(b'a')
                .wrapping_add(1);
            if byte <= 26 {
                out.push(byte);
            } else {
                return None;
            }
        }
        Char(c) => {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
        Enter => out.push(b'\r'),
        Backspace => out.push(0x7f),
        Tab => out.push(b'\t'),
        BackTab => out.extend_from_slice(b"\x1b[Z"),
        Esc => out.push(0x1b),
        Up => out.extend_from_slice(if app_cursor { b"\x1bOA" } else { b"\x1b[A" }),
        Down => out.extend_from_slice(if app_cursor { b"\x1bOB" } else { b"\x1b[B" }),
        Right => out.extend_from_slice(if app_cursor { b"\x1bOC" } else { b"\x1b[C" }),
        Left => out.extend_from_slice(if app_cursor { b"\x1bOD" } else { b"\x1b[D" }),
        Home => out.extend_from_slice(b"\x1b[H"),
        End => out.extend_from_slice(b"\x1b[F"),
        PageUp => out.extend_from_slice(b"\x1b[5~"),
        PageDown => out.extend_from_slice(b"\x1b[6~"),
        Delete => out.extend_from_slice(b"\x1b[3~"),
        Insert => out.extend_from_slice(b"\x1b[2~"),
        F(n) => {
            let sequence: &[u8] = match n {
                1 => b"\x1bOP",
                2 => b"\x1bOQ",
                3 => b"\x1bOR",
                4 => b"\x1bOS",
                5 => b"\x1b[15~",
                6 => b"\x1b[17~",
                7 => b"\x1b[18~",
                8 => b"\x1b[19~",
                9 => b"\x1b[20~",
                10 => b"\x1b[21~",
                11 => b"\x1b[23~",
                12 => b"\x1b[24~",
                _ => return None,
            };
            out.extend_from_slice(sequence);
        }
        _ => return None,
    }
    Some(out)
}

async fn handle_normal_key(
    app: &mut App,
    key: KeyEvent,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('k') {
        open_palette(app);
        return Ok(());
    }

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => app.select_next(),
        KeyCode::Char('k') | KeyCode::Up => app.select_previous(),
        KeyCode::Enter | KeyCode::Char('i') if app.selected_is_alive_pty() => {
            app.scroll_offset = 0;
            app.parser.screen_mut().set_scrollback(0);
            app.mode = Mode::PtyType {
                ctrl_p_pending: false,
            };
        }
        KeyCode::Char('b') => app.show_rail = !app.show_rail,
        KeyCode::Char(',') => open_settings(app),
        KeyCode::Char('n') => {
            app.prompt_input = Input::default();
            app.mode = Mode::Prompt(PromptKind::NewJob);
        }
        KeyCode::Char('r') => restart_selected(app, socket_path, tx).await,
        KeyCode::Char('s') => stop_selected(app, socket_path, tx).await,
        KeyCode::Char('K') => kill_selected(app, socket_path, tx).await,
        KeyCode::Char('l') => {
            app.log_search_query.clear();
            app.log_scroll = 0;
            load_logs(app, socket_path, tx).await?;
            app.mode = Mode::Overlay(OverlayKind::LogsDiff);
        }
        KeyCode::Char('c') => clean_terminated(app, socket_path, tx).await,
        KeyCode::Char('?') => app.mode = Mode::Overlay(OverlayKind::Help),
        KeyCode::PageUp => scroll_stage(app, 10),
        KeyCode::PageDown => scroll_stage(app, -10),
        KeyCode::Char(':') => open_palette(app),
        KeyCode::Esc => app.toast = None,
        KeyCode::Char('q') => app.should_quit = true,
        _ => {}
    }
    Ok(())
}

fn open_palette(app: &mut App) {
    app.palette_input = Input::default();
    app.palette_state.select(Some(0));
    app.mode = Mode::Palette;
}

fn open_settings(app: &mut App) {
    app.settings_state.select(Some(0));
    app.mode = Mode::Overlay(OverlayKind::Settings);
}

async fn handle_palette_key(
    app: &mut App,
    key: KeyEvent,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    let action_count = palette_actions(app).len();
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Down => move_list_selection(&mut app.palette_state, 1, action_count),
        KeyCode::Up => move_list_selection(&mut app.palette_state, -1, action_count),
        KeyCode::Enter => {
            let action = app
                .palette_state
                .selected()
                .and_then(|index| palette_actions(app).into_iter().nth(index))
                .map(|(_, action)| action);
            app.mode = Mode::Normal;
            if let Some(action) = action {
                execute_palette_action(app, action, socket_path, tx).await?;
            }
        }
        _ => {
            app.palette_input.handle_event(&Event::Key(key));
            app.palette_state.select(Some(0));
        }
    }
    Ok(())
}

fn move_list_selection(state: &mut ListState, delta: i32, len: usize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0);
    let next = if delta < 0 {
        current.saturating_sub(delta.unsigned_abs() as usize)
    } else {
        current.saturating_add(usize::try_from(delta).unwrap_or(0))
    }
    .min(len - 1);
    state.select(Some(next));
}

async fn handle_prompt_key(
    app: &mut App,
    key: KeyEvent,
    kind: PromptKind,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    match key.code {
        KeyCode::Esc => app.mode = Mode::Normal,
        KeyCode::Enter => {
            let value = app.prompt_input.value().trim().to_string();
            app.prompt_input = Input::default();
            app.mode = Mode::Normal;
            if !value.is_empty() {
                apply_prompt(app, kind, value, socket_path, tx).await?;
            }
        }
        _ => {
            app.prompt_input.handle_event(&Event::Key(key));
        }
    }
    Ok(())
}

async fn handle_overlay_key(
    app: &mut App,
    key: KeyEvent,
    kind: OverlayKind,
    settings_path: &Path,
) -> Result<()> {
    match kind {
        OverlayKind::Settings => handle_settings_key(app, key, settings_path).await,
        OverlayKind::LogsDiff => {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q') => app.mode = Mode::Normal,
                KeyCode::Char('j') | KeyCode::Down => scroll_logs(app, -1),
                KeyCode::Char('k') | KeyCode::Up => scroll_logs(app, 1),
                KeyCode::PageDown => scroll_logs(app, -10),
                KeyCode::PageUp => scroll_logs(app, 10),
                KeyCode::Char('g') => app.log_scroll = usize::MAX,
                KeyCode::Char('G') => app.log_scroll = 0,
                _ => {}
            }
            Ok(())
        }
        OverlayKind::Digest | OverlayKind::Inspect | OverlayKind::Help => {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q') | KeyCode::Enter) {
                app.mode = Mode::Normal;
            }
            Ok(())
        }
    }
}

async fn handle_settings_key(app: &mut App, key: KeyEvent, settings_path: &Path) -> Result<()> {
    match key.code {
        KeyCode::Esc | KeyCode::Char(',') => {
            if let Err(err) = close_settings(app, settings_path).await {
                app.set_toast(format!("settings save failed: {err:#}"));
            }
        }
        KeyCode::Char('j') | KeyCode::Down => {
            move_list_selection(&mut app.settings_state, 1, SettingsRow::ALL.len());
        }
        KeyCode::Char('k') | KeyCode::Up => {
            move_list_selection(&mut app.settings_state, -1, SettingsRow::ALL.len());
        }
        KeyCode::Char('h' | 'l' | ' ') | KeyCode::Left | KeyCode::Right | KeyCode::Enter => {
            let forwards = !matches!(key.code, KeyCode::Char('h') | KeyCode::Left);
            let index = app.settings_state.selected().unwrap_or(0);
            let row = SettingsRow::ALL[index.min(SettingsRow::ALL.len() - 1)];
            let mouse_before = app.settings.mouse;
            app.settings.cycle(row, forwards);
            if app.settings.mouse != mouse_before {
                apply_mouse_capture(app);
            }
            app.set_poll_ms(app.settings.poll_ms);
        }
        _ => {}
    }
    Ok(())
}

async fn close_settings(app: &mut App, path: &Path) -> Result<()> {
    app.mode = Mode::Normal;
    super::settings::save_to_path(path, &app.settings).await
}

fn apply_mouse_capture(app: &mut App) {
    let result = if app.settings.mouse {
        crossterm::execute!(std::io::stdout(), EnableMouseCapture)
    } else {
        crossterm::execute!(std::io::stdout(), DisableMouseCapture)
    };
    if let Err(err) = result {
        // Mouse support is optional; a terminal that rejects it must not stop
        // the TUI or prevent the preference from being saved.
        app.set_toast(format!("mouse capture unavailable: {err}"));
    }
}

pub fn handle_mouse(app: &mut App, mouse: MouseEvent) {
    if matches!(app.mode, Mode::Overlay(OverlayKind::LogsDiff)) {
        match mouse.kind {
            MouseEventKind::ScrollUp => scroll_logs(app, 3),
            MouseEventKind::ScrollDown => scroll_logs(app, -3),
            _ => {}
        }
        return;
    }

    let position = Position::new(mouse.column, mouse.row);
    if app.rail_inner_rect.contains(position) {
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                let row = mouse.row.saturating_sub(app.rail_inner_rect.y) as usize;
                let list_height = app.rail_inner_rect.height.saturating_sub(2) as usize;
                if row < list_height {
                    let index = app.rail_state.offset() + row;
                    app.select_index(index);
                }
            }
            MouseEventKind::ScrollUp => app.select_previous(),
            MouseEventKind::ScrollDown => app.select_next(),
            _ => {}
        }
        return;
    }

    if matches!(app.mode, Mode::Normal) && app.stage_pty_rect.contains(position) {
        match mouse.kind {
            MouseEventKind::ScrollUp => scroll_stage(app, 3),
            MouseEventKind::ScrollDown => scroll_stage(app, -3),
            _ => {}
        }
    }
}

fn scroll_stage(app: &mut App, delta: i32) {
    app.scroll_offset = if delta >= 0 {
        app.scroll_offset
            .saturating_add(usize::try_from(delta).unwrap_or(0))
    } else {
        app.scroll_offset
            .saturating_sub(delta.unsigned_abs() as usize)
    };
    app.parser.screen_mut().set_scrollback(app.scroll_offset);
}

fn scroll_logs(app: &mut App, delta: i32) {
    app.log_scroll = if delta >= 0 {
        app.log_scroll
            .saturating_add(usize::try_from(delta).unwrap_or(0))
    } else {
        app.log_scroll.saturating_sub(delta.unsigned_abs() as usize)
    };
}

async fn execute_palette_action(
    app: &mut App,
    action: PaletteAction,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    match action {
        PaletteAction::ViewDigest => {
            load_digest(app, socket_path, tx).await?;
            app.mode = Mode::Overlay(OverlayKind::Digest);
        }
        PaletteAction::SearchLogs => {
            app.prompt_input = Input::default();
            app.mode = Mode::Prompt(PromptKind::LogSearch);
        }
        PaletteAction::InspectJob => app.mode = Mode::Overlay(OverlayKind::Inspect),
        PaletteAction::ExportLog => {
            app.prompt_input = Input::default();
            app.mode = Mode::Prompt(PromptKind::ExportPath);
        }
        PaletteAction::NewJob => {
            app.prompt_input = Input::default();
            app.mode = Mode::Prompt(PromptKind::NewJob);
        }
        PaletteAction::FilterWorkspace => {
            app.prompt_input = Input::default();
            app.mode = Mode::Prompt(PromptKind::WorkspaceFilter);
        }
        PaletteAction::RestartJob => restart_selected(app, socket_path, tx).await,
        PaletteAction::StopJob => stop_selected(app, socket_path, tx).await,
        PaletteAction::KillJob => kill_selected(app, socket_path, tx).await,
        PaletteAction::CleanTerminated => clean_terminated(app, socket_path, tx).await,
        PaletteAction::ToggleSidebar => app.show_rail = !app.show_rail,
        PaletteAction::Settings => open_settings(app),
    }
    Ok(())
}

async fn apply_prompt(
    app: &mut App,
    kind: PromptKind,
    value: String,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    match kind {
        PromptKind::NewJob => start_job(app, value, socket_path, tx).await?,
        PromptKind::ExportPath => export_log(app, value, socket_path, tx).await?,
        PromptKind::WorkspaceFilter => {
            app.set_workspace_filter((value != "-").then_some(value));
            let _ = tx.send(DaemonEvent::Toast("workspace filter updated".into()));
        }
        PromptKind::LogSearch => {
            load_logs(app, socket_path, tx).await?;
            let needle = value.to_lowercase();
            app.log_lines
                .retain(|line| line.to_lowercase().contains(&needle));
            app.log_search_query = value;
            app.log_scroll = 0;
            app.mode = Mode::Overlay(OverlayKind::LogsDiff);
        }
    }
    Ok(())
}

async fn start_job(
    app: &mut App,
    command: String,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    let shell_syntax = command.contains(['|', '&', ';', '>', '<', '$']);
    let mut cmd = shlex::split(&command).unwrap_or_else(|| vec![command.clone()]);
    let resolved = if !shell_syntax && cmd.len() == 1 {
        crate::commands::run::try_resolve_from_config(&cmd[0]).await
    } else {
        None
    };
    if let Some(config) = &resolved {
        cmd.clone_from(&config.cmd);
    }
    if shell_syntax || (resolved.is_none() && cmd.len() == 1) {
        cmd = vec!["sh".into(), "-c".into(), command];
    }

    let configured_pty = resolved
        .as_ref()
        .map_or(Some(app.settings.default_pty), |config| Some(config.pty));
    let mut args = resolved.unwrap_or_else(|| RunArgs {
        cmd: Vec::new(),
        name: None,
        workspace: None,
        readiness: None,
        restart: None,
        max_retries: None,
        log_max_size: None,
        log_keep: None,
        memory_max: None,
        cpu_quota: None,
        pids_max: None,
        sandbox: false,
        pty: app.settings.default_pty,
        max_runtime_ms: None,
        env: HashMap::new(),
        after: None,
        cwd: None,
        pty_cols: None,
        pty_rows: None,
        max_rss_mb: None,
        allocate_port: None,
        health_check: None,
        health_interval_secs: None,
        health_threshold: None,
    });
    args.cmd = cmd;
    args.workspace = app.workspace_filter.clone().or(args.workspace);
    args.pty = crate::commands::run::resolve_pty(None, args.sandbox, configured_pty)?;
    let configured_env = std::mem::take(&mut args.env);
    let mut env = crate::commands::run::terminal_env();
    for (key, value) in configured_env {
        env.entry(key).or_insert(value);
    }
    args.env = env;
    if args.cwd.is_none() {
        args.cwd = std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned());
    }

    match rpc_autostart::<JobRecord>(socket_path, Command::Run(args)).await {
        Ok(Some(record)) => {
            let name = record.name.clone().unwrap_or_else(|| record.id.clone());
            if let Ok(jobs) = fetch_jobs(socket_path, app.workspace_filter.clone()).await {
                let _ = tx.send(DaemonEvent::JobsUpdated(jobs));
            }
            app.set_selected(Some(record.id));
            app.set_toast(format!("started {name}"));
        }
        Ok(None) => {}
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("start failed: {err:#}")));
        }
    }
    Ok(())
}

async fn restart_selected(
    app: &mut App,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) {
    let Some(id) = app.selected_id.clone() else {
        return;
    };
    match rpc_autostart::<JobRecord>(
        socket_path,
        Command::Restart {
            id,
            timeout_ms: 10_000,
        },
    )
    .await
    {
        Ok(Some(record)) => {
            let name = record.name.clone().unwrap_or_else(|| record.id.clone());
            app.set_selected(Some(record.id.clone()));
            app.set_toast(format!("restarted {name}"));
        }
        Ok(None) => {}
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("restart failed: {err:#}")));
        }
    }
}

async fn stop_selected(app: &App, socket_path: &Path, tx: &mpsc::UnboundedSender<DaemonEvent>) {
    let Some(id) = app.selected_id.clone() else {
        return;
    };
    match rpc_autostart::<serde_json::Value>(
        socket_path,
        Command::Stop {
            id,
            timeout_ms: 10_000,
        },
    )
    .await
    {
        Ok(_) => {
            let _ = tx.send(DaemonEvent::Toast("stopped".into()));
        }
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("stop failed: {err:#}")));
        }
    }
}

async fn kill_selected(app: &App, socket_path: &Path, tx: &mpsc::UnboundedSender<DaemonEvent>) {
    let Some(id) = app.selected_id.clone() else {
        return;
    };
    match rpc_autostart::<serde_json::Value>(
        socket_path,
        Command::Kill(KillArgs {
            id: Some(id),
            workspace: None,
        }),
    )
    .await
    {
        Ok(_) => {
            let _ = tx.send(DaemonEvent::Toast("killed".into()));
        }
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("kill failed: {err:#}")));
        }
    }
}

async fn clean_terminated(app: &App, socket_path: &Path, tx: &mpsc::UnboundedSender<DaemonEvent>) {
    match rpc_autostart::<serde_json::Value>(
        socket_path,
        Command::Clean {
            workspace: app.workspace_filter.clone(),
        },
    )
    .await
    {
        Ok(Some(value)) => {
            let removed = value["removed"].as_u64().unwrap_or(0);
            let _ = tx.send(DaemonEvent::Toast(format!("cleaned {removed} job(s)")));
        }
        Ok(None) => {}
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("clean failed: {err:#}")));
        }
    }
}

async fn export_log(
    app: &App,
    path: String,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    let Some(id) = app.selected_id.clone() else {
        return Ok(());
    };
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    };
    let path = path.to_string_lossy().into_owned();
    match rpc_autostart::<serde_json::Value>(
        socket_path,
        Command::Export {
            id,
            path: path.clone(),
            lines: None,
            level: None,
            stream: None,
            strip_ansi: false,
            filter_regex: None,
            since_ms: None,
        },
    )
    .await
    {
        Ok(_) => {
            let _ = tx.send(DaemonEvent::Toast(format!("exported to {path}")));
        }
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("export failed: {err:#}")));
        }
    }
    Ok(())
}

async fn load_digest(
    app: &App,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    let Some(id) = app.selected_id.clone() else {
        return Ok(());
    };
    match rpc::<serde_json::Value>(
        socket_path,
        Command::Tail(TailArgs {
            id,
            lines: 0,
            digest: true,
            level: None,
            strip_ansi: false,
            stream: None,
            cursor: None,
            follow: false,
            filter_regex: None,
        }),
    )
    .await
    {
        Ok(Some(value)) => {
            let digest_value = if value.get("digest").is_some() {
                value["digest"].clone()
            } else {
                value
            };
            if let Ok(digest) = serde_json::from_value::<LogDigest>(digest_value) {
                let _ = tx.send(DaemonEvent::DigestLoaded(digest));
            }
        }
        Ok(None) => {}
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("digest failed: {err:#}")));
        }
    }
    Ok(())
}

async fn load_logs(
    app: &App,
    socket_path: &Path,
    tx: &mpsc::UnboundedSender<DaemonEvent>,
) -> Result<()> {
    let Some(id) = app.selected_id.clone() else {
        return Ok(());
    };
    match rpc::<serde_json::Value>(
        socket_path,
        Command::Tail(TailArgs {
            id,
            lines: 200,
            digest: false,
            level: None,
            strip_ansi: false,
            stream: None,
            cursor: None,
            follow: false,
            filter_regex: None,
        }),
    )
    .await
    {
        Ok(Some(value)) => {
            let lines: Vec<String> = value["lines"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|line| line["content"].as_str().map(str::to_string))
                .collect();
            let _ = tx.send(DaemonEvent::LogsLoaded(lines));
        }
        Ok(None) => {}
        Err(err) => {
            let _ = tx.send(DaemonEvent::Error(format!("logs failed: {err:#}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn ctrl_c_encodes_as_0x03() {
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('c'), KeyModifiers::CONTROL), false),
            Some(vec![0x03])
        );
    }

    #[test]
    fn arrows_respect_application_cursor_mode() {
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Up, KeyModifiers::NONE), false),
            Some(b"\x1b[A".to_vec())
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Up, KeyModifiers::NONE), true),
            Some(b"\x1bOA".to_vec())
        );
    }

    #[test]
    fn utf8_and_special_keys_encode() {
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Char('é'), KeyModifiers::NONE), false),
            Some("é".as_bytes().to_vec())
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::Backspace, KeyModifiers::NONE), false),
            Some(vec![0x7f])
        );
        assert_eq!(
            key_event_to_bytes(key(KeyCode::F(5), KeyModifiers::NONE), false),
            Some(b"\x1b[15~".to_vec())
        );
    }

    #[test]
    fn ctrl_p_non_q_clears_pending_and_flushes_0x10() {
        let mut app = App::new();
        app.mode = Mode::PtyType {
            ctrl_p_pending: false,
        };
        assert_eq!(
            pty_key_action(
                &mut app,
                key(KeyCode::Char('p'), KeyModifiers::CONTROL),
                false
            ),
            Some(PtyKeyAction::Pending)
        );
        assert_eq!(
            pty_key_action(&mut app, key(KeyCode::Char('a'), KeyModifiers::NONE), false),
            Some(PtyKeyAction::Forward(vec![0x10, b'a']))
        );
        assert_eq!(
            app.mode,
            Mode::PtyType {
                ctrl_p_pending: false
            }
        );
        assert_eq!(
            pty_key_action(&mut app, key(KeyCode::Char('b'), KeyModifiers::NONE), false),
            Some(PtyKeyAction::Forward(vec![b'b']))
        );
    }

    #[test]
    fn ctrl_p_ctrl_q_exits_pty_type_mode() {
        let mut app = App::new();
        app.mode = Mode::PtyType {
            ctrl_p_pending: false,
        };
        pty_key_action(
            &mut app,
            key(KeyCode::Char('p'), KeyModifiers::CONTROL),
            false,
        );
        assert_eq!(
            pty_key_action(
                &mut app,
                key(KeyCode::Char('q'), KeyModifiers::CONTROL),
                false
            ),
            Some(PtyKeyAction::Detach)
        );
        assert_eq!(app.mode, Mode::Normal);
    }

    #[test]
    fn palette_filters_by_subsequence_and_includes_settings() {
        let mut app = App::new();
        let all = palette_actions(&app).len();
        app.palette_input = Input::new("kll".into());
        let filtered = palette_actions(&app);
        assert!(filtered.len() < all);
        assert!(filtered.contains(&("Kill job", PaletteAction::KillJob)));
        assert!(palette_actions(&App::new()).contains(&("Settings", PaletteAction::Settings)));
    }

    #[tokio::test]
    async fn settings_navigation_and_live_option_cycling() {
        let mut app = App::new();
        app.mode = Mode::Overlay(OverlayKind::Settings);
        app.settings_state.select(Some(0));
        let (tx, _rx) = mpsc::unbounded_channel();
        let socket = Path::new("/tmp/bgrun-tui-test.sock");
        handle_key(
            &mut app,
            key(KeyCode::Right, KeyModifiers::NONE),
            None,
            socket,
            &tx,
        )
        .await
        .unwrap();
        assert_eq!(app.settings.theme, super::super::theme::ThemeKind::Dark);
        handle_key(
            &mut app,
            key(KeyCode::Down, KeyModifiers::NONE),
            None,
            socket,
            &tx,
        )
        .await
        .unwrap();
        handle_key(
            &mut app,
            key(KeyCode::Right, KeyModifiers::NONE),
            None,
            socket,
            &tx,
        )
        .await
        .unwrap();
        assert_eq!(
            app.settings.chrome,
            super::super::theme::ChromeStyle::Minimal
        );
        for _ in 0..2 {
            handle_key(
                &mut app,
                key(KeyCode::Down, KeyModifiers::NONE),
                None,
                socket,
                &tx,
            )
            .await
            .unwrap();
        }
        handle_key(
            &mut app,
            key(KeyCode::Char(' '), KeyModifiers::NONE),
            None,
            socket,
            &tx,
        )
        .await
        .unwrap();
        assert!(!app.settings.default_pty);

        let path =
            std::env::temp_dir().join(format!("bgrun-tui-close-{}.toml", uuid::Uuid::new_v4()));
        handle_settings_key(&mut app, key(KeyCode::Esc, KeyModifiers::NONE), &path)
            .await
            .unwrap();
        assert_eq!(app.mode, Mode::Normal);
        let saved = super::super::settings::load_from_path(&path).await;
        assert_eq!(saved, app.settings);
        tokio::fs::remove_file(path).await.unwrap();
    }

    #[test]
    fn list_selection_clamps_at_both_ends() {
        let mut state = ListState::default();
        move_list_selection(&mut state, -1, 3);
        assert_eq!(state.selected(), Some(0));
        move_list_selection(&mut state, 10, 3);
        assert_eq!(state.selected(), Some(2));
    }
}
