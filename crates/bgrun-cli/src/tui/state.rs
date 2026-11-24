//! Application state for the interactive TUI.

use std::sync::{Arc, Mutex};

use bgrun_proto::{JobRecord, JobState, LogDigest, ResourceStats};
use tui_input::Input;
use tui_term::vt100;

use super::daemon::DaemonEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mode {
    /// Navigate the rail, view the live terminal, use single-key shortcuts.
    Normal,
    /// Keystrokes go straight to the selected job's PTY stdin.
    PtyType { ctrl_p_pending: bool },
    /// Fuzzy `Ctrl+K` command palette.
    Palette,
    /// Single-line prompt overlay.
    Prompt(PromptKind),
    /// Read-only modal overlay.
    Overlay(OverlayKind),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptKind {
    NewJob,
    ExportPath,
    WorkspaceFilter,
    LogSearch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayKind {
    Digest,
    LogsDiff,
    Inspect,
    Help,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaletteAction {
    ViewDigest,
    SearchLogs,
    InspectJob,
    ExportLog,
    NewJob,
    FilterWorkspace,
    RestartJob,
    StopJob,
    KillJob,
    CleanTerminated,
    ToggleSidebar,
}

/// Returns whether a job state represents a running process.
pub fn is_alive(state: &JobState) -> bool {
    !matches!(
        state,
        JobState::Exited | JobState::Crashed | JobState::Killed
    )
}

/// Shared inputs for the background polling task.
#[derive(Debug, Default)]
pub struct PollCtl {
    pub selected: Option<String>,
    pub workspace: Option<String>,
}

pub struct App {
    pub should_quit: bool,
    pub mode: Mode,
    pub show_rail: bool,

    // Daemon data.
    pub workspace_filter: Option<String>,
    pub jobs: Vec<JobRecord>,
    pub selected_id: Option<String>,
    pub selected_stats: Option<ResourceStats>,

    // Embedded terminal for the selected job (live PTY or replayed logs).
    pub parser: vt100::Parser,
    /// `(cols, rows)` of the PTY canvas, set by the renderer each frame.
    pub stage_inner_size: (u16, u16),
    pub scroll_offset: usize,

    // Transient UI state.
    pub palette_input: Input,
    pub palette_selected: usize,
    pub prompt_input: Input,
    pub digest_cache: Option<LogDigest>,
    pub log_lines: Vec<String>,
    pub log_search_query: String,
    pub toast: Option<String>,

    /// Shared with the poller so it can scope `List` and request stats.
    pub poll_ctl: Arc<Mutex<PollCtl>>,
}

impl std::fmt::Debug for App {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("App")
            .field("mode", &self.mode)
            .field("jobs", &self.jobs.len())
            .field("selected_id", &self.selected_id)
            .finish_non_exhaustive()
    }
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    pub fn new() -> Self {
        Self {
            should_quit: false,
            mode: Mode::Normal,
            show_rail: true,
            workspace_filter: None,
            jobs: Vec::new(),
            selected_id: None,
            selected_stats: None,
            parser: vt100::Parser::new(24, 80, 2000),
            stage_inner_size: (80, 24),
            scroll_offset: 0,
            palette_input: Input::default(),
            palette_selected: 0,
            prompt_input: Input::default(),
            digest_cache: None,
            log_lines: Vec::new(),
            log_search_query: String::new(),
            toast: None,
            poll_ctl: Arc::new(Mutex::new(PollCtl::default())),
        }
    }

    pub fn selected_job(&self) -> Option<&JobRecord> {
        self.selected_id
            .as_ref()
            .and_then(|id| self.jobs.iter().find(|job| &job.id == id))
    }

    /// Whether typing into the embedded terminal is meaningful right now.
    pub fn selected_is_alive_pty(&self) -> bool {
        self.selected_job()
            .is_some_and(|job| job.pty && is_alive(&job.state))
    }

    pub fn set_selected(&mut self, id: Option<String>) {
        self.selected_id.clone_from(&id);
        if let Ok(mut ctl) = self.poll_ctl.lock() {
            ctl.selected = id;
        }
        self.selected_stats = None;
        self.scroll_offset = 0;
    }

    pub fn set_workspace_filter(&mut self, workspace: Option<String>) {
        self.workspace_filter.clone_from(&workspace);
        if let Ok(mut ctl) = self.poll_ctl.lock() {
            ctl.workspace = workspace;
        }
    }

    pub fn select_next(&mut self) {
        if self.jobs.is_empty() {
            return;
        }
        let next = self
            .selected_id
            .as_ref()
            .and_then(|id| self.jobs.iter().position(|job| &job.id == id))
            .map_or(0, |i| (i + 1) % self.jobs.len());
        self.set_selected(Some(self.jobs[next].id.clone()));
    }

    pub fn select_previous(&mut self) {
        if self.jobs.is_empty() {
            return;
        }
        let previous = self
            .selected_id
            .as_ref()
            .and_then(|id| self.jobs.iter().position(|job| &job.id == id))
            .map_or(0, |i| (i + self.jobs.len() - 1) % self.jobs.len());
        self.set_selected(Some(self.jobs[previous].id.clone()));
    }

    pub fn apply_daemon_event(&mut self, event: DaemonEvent) {
        match event {
            DaemonEvent::JobsUpdated(jobs) => {
                let had_selection = self.selected_id.is_some();
                self.jobs = jobs;
                let still_exists = self
                    .selected_id
                    .as_ref()
                    .is_some_and(|id| self.jobs.iter().any(|job| &job.id == id));
                if !had_selection || !still_exists {
                    self.set_selected(self.jobs.first().map(|job| job.id.clone()));
                }
            }
            DaemonEvent::StatsUpdated { id, stats } => {
                if self.selected_id.as_ref() == Some(&id) {
                    self.selected_stats = Some(stats);
                }
            }
            DaemonEvent::PtyBytes { id, bytes } => {
                if self.selected_id.as_ref() == Some(&id) {
                    self.parser.process(&bytes);
                }
            }
            DaemonEvent::PtyClosed { id } => {
                if self.selected_id.as_ref() == Some(&id) {
                    if matches!(self.mode, Mode::PtyType { .. }) {
                        self.mode = Mode::Normal;
                    }
                    let name = self
                        .selected_job()
                        .and_then(|job| job.name.as_deref())
                        .unwrap_or(&id);
                    self.toast = Some(format!("{name} exited"));
                }
            }
            DaemonEvent::DigestLoaded(digest) => self.digest_cache = Some(digest),
            DaemonEvent::LogsLoaded(lines) => self.log_lines = lines,
            DaemonEvent::Toast(message) | DaemonEvent::Error(message) => {
                self.toast = Some(message);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn record(id: &str, state: JobState) -> JobRecord {
        JobRecord {
            id: id.into(),
            name: Some(id.into()),
            workspace: None,
            cmd: vec!["true".into()],
            pid: None,
            state,
            started_at: "2026-01-01T00:00:00Z".into(),
            readiness: None,
            restart: None,
            max_retries: None,
            log_max_size: None,
            log_keep: None,
            memory_max: None,
            cpu_quota: None,
            pids_max: None,
            sandbox: false,
            pty: true,
            max_runtime_ms: None,
            env: HashMap::default(),
            max_rss_mb: None,
            cwd: None,
            allocated_port: None,
            health_check: None,
            health_interval_secs: None,
            health_threshold: None,
        }
    }

    #[test]
    fn selection_survives_earlier_jobs_being_cleaned() {
        let mut app = App::new();
        app.apply_daemon_event(DaemonEvent::JobsUpdated(vec![
            record("a", JobState::Running),
            record("b", JobState::Ready),
        ]));
        app.set_selected(Some("b".into()));
        // Cleaning the earlier job must not move the selection.
        app.apply_daemon_event(DaemonEvent::JobsUpdated(vec![record("b", JobState::Ready)]));
        assert_eq!(app.selected_id.as_deref(), Some("b"));
        app.apply_daemon_event(DaemonEvent::JobsUpdated(Vec::new()));
        assert_eq!(app.selected_id, None);
    }

    #[test]
    fn pty_close_returns_to_normal_mode() {
        let mut app = App::new();
        app.apply_daemon_event(DaemonEvent::JobsUpdated(vec![record(
            "a",
            JobState::Running,
        )]));
        app.mode = Mode::PtyType {
            ctrl_p_pending: false,
        };
        app.apply_daemon_event(DaemonEvent::PtyClosed { id: "a".into() });
        assert_eq!(app.mode, Mode::Normal);
        assert_eq!(app.toast.as_deref(), Some("a exited"));
    }
}
