//! Minimal three-zone Ratatui renderer plus floating overlays.

use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, Paragraph},
};
use tui_term::widget::{Cursor, PseudoTerminal};

use super::keys::palette_actions;
use super::state::{App, Mode, OverlayKind, PromptKind, is_alive};

pub const BG_STAGE: Color = Color::Rgb(12, 13, 16);
pub const BG_RAIL: Color = Color::Rgb(17, 19, 24);
pub const BG_SELECT: Color = Color::Rgb(26, 31, 41);
pub const BG_PALETTE: Color = Color::Rgb(19, 22, 29);
pub const DIVIDER: Color = Color::Rgb(30, 34, 43);
pub const FG_PRIMARY: Color = Color::Rgb(230, 232, 236);
pub const FG_MUTED: Color = Color::Rgb(122, 130, 145);
pub const FG_DIM: Color = Color::Rgb(75, 82, 96);
pub const ACCENT: Color = Color::Rgb(95, 215, 255);
pub const GREEN: Color = Color::Rgb(74, 222, 128);
pub const YELLOW: Color = Color::Rgb(250, 204, 21);
pub const RED: Color = Color::Rgb(248, 113, 113);

const HELP_LINES: &[&str] = &[
    "j / ↓  next job          k / ↑  previous job",
    "Enter / i  type into PTY   Ctrl+P Ctrl+Q  detach",
    "Ctrl+K / :  command palette    b  toggle sidebar",
    "n  new job   r  restart   s  stop   K  kill",
    "l  logs/diff   c  clean terminated   ?  help",
    "PageUp / PageDown  scrollback     q  quit",
    "",
    "Quitting the TUI leaves the daemon and jobs running.",
];

pub fn render(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    let [body, footer] = Layout::vertical([Constraint::Min(3), Constraint::Length(1)]).areas(area);

    let (rail_area, stage_area) = if app.show_rail && body.width >= 60 {
        let [rail, stage] =
            Layout::horizontal([Constraint::Length(24), Constraint::Min(20)]).areas(body);
        (Some(rail), stage)
    } else {
        (None, body)
    };

    if let Some(rail) = rail_area {
        render_rail(frame, app, rail);
    }
    render_stage(frame, app, stage_area);
    render_footer(frame, app, footer);

    match app.mode {
        Mode::Palette => render_palette(frame, app, area),
        Mode::Prompt(kind) => render_prompt(frame, app, kind, area),
        Mode::Overlay(kind) => render_overlay(frame, app, kind, area),
        Mode::Normal | Mode::PtyType { .. } => {}
    }
}

fn render_rail(frame: &mut Frame, app: &App, area: Rect) {
    let title = match &app.workspace_filter {
        Some(ws) => format!(" JOBS · {ws} "),
        None => " JOBS ".to_string(),
    };
    let block = Block::default()
        .borders(Borders::RIGHT)
        .border_style(Style::default().fg(DIVIDER))
        .title(title)
        .style(Style::default().bg(BG_RAIL));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let lines: Vec<Line> = app
        .jobs
        .iter()
        .map(|job| {
            let selected = app.selected_id.as_deref() == Some(job.id.as_str());
            let name = job.name.clone().unwrap_or_else(|| short_id(&job.id));
            let state = job.state.to_string();
            let state_color = match job.state {
                bgrun_proto::JobState::Ready => GREEN,
                bgrun_proto::JobState::Running | bgrun_proto::JobState::Starting => ACCENT,
                bgrun_proto::JobState::Crashed => RED,
                bgrun_proto::JobState::Exited | bgrun_proto::JobState::Killed => FG_DIM,
            };
            let style = if selected {
                Style::default()
                    .bg(BG_SELECT)
                    .fg(FG_PRIMARY)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(FG_PRIMARY)
            };
            let marker = if selected { "▸" } else { " " };
            Line::from(vec![
                Span::styled(format!("{marker} "), style),
                Span::styled(ellipsize(&name, 11), style),
                Span::styled(" ", style),
                Span::styled(ellipsize(&state, 8), Style::default().fg(state_color)),
                Span::styled(
                    if job.pty { " PTY" } else { " PIPE" },
                    Style::default().fg(FG_DIM),
                ),
            ])
        })
        .collect();

    let paragraph = if lines.is_empty() {
        Paragraph::new(Line::from(Span::styled(
            " No jobs yet · press n",
            Style::default().fg(FG_MUTED),
        )))
    } else {
        Paragraph::new(lines)
    };
    frame.render_widget(paragraph, inner);
}

fn render_stage(frame: &mut Frame, app: &mut App, area: Rect) {
    let [header, pty_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(area);

    let is_typing = matches!(app.mode, Mode::PtyType { .. });
    let hairline = if is_typing { ACCENT } else { DIVIDER };
    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(hairline))
        .style(Style::default().bg(BG_STAGE));
    frame.render_widget(
        Paragraph::new(build_stage_header(app, header.width as usize, is_typing)).block(block),
        header,
    );

    app.stage_inner_size = (pty_area.width.max(2), pty_area.height.max(2));

    let cursor_style = if is_typing {
        Style::default().bg(ACCENT).fg(BG_STAGE)
    } else {
        Style::default().bg(FG_DIM)
    };
    let cursor = Cursor::default().style(cursor_style);
    let pseudo_term = PseudoTerminal::new(app.parser.screen())
        .cursor(cursor)
        .style(Style::default().bg(BG_STAGE).fg(FG_PRIMARY));
    frame.render_widget(pseudo_term, pty_area);
}

fn build_stage_header(app: &App, width: usize, is_typing: bool) -> String {
    let Some(job) = app.selected_job() else {
        return ellipsize(" no job selected · press n to start one ", width);
    };
    let name = job.name.clone().unwrap_or_else(|| short_id(&job.id));
    let stats = app
        .selected_stats
        .as_ref()
        .map(|stats| format!(" · {:.1}% cpu · {}MB", stats.cpu_pct, stats.rss_mb))
        .unwrap_or_default();
    let pipe = if is_typing { "TYPE" } else { "VIEW" };
    let header = format!(
        " {name} · {} · pid {} · {}{stats}  |  {} · {pipe} ",
        job.state,
        job.pid.map_or_else(|| "—".into(), |pid| pid.to_string()),
        if job.pty { "pty" } else { "piped" },
        if is_alive(&job.state) { "live" } else { "done" },
    );
    ellipsize(&header, width)
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect) {
    let text = match app.mode {
        Mode::PtyType { .. } => {
            " Ctrl+P Ctrl+Q detach · Esc not forwarded · type freely".to_string()
        }
        _ => app.toast.clone().unwrap_or_else(|| {
            " j/k jobs · Enter type · Ctrl+K palette · n new · r restart · s stop · K kill · l logs · c clean · ? help · q quit".into()
        }),
    };
    let style = if matches!(app.mode, Mode::PtyType { .. }) {
        Style::default().fg(ACCENT).bg(BG_STAGE)
    } else if app.toast.is_some() && !matches!(app.mode, Mode::PtyType { .. }) {
        Style::default().fg(YELLOW).bg(BG_STAGE)
    } else {
        Style::default().fg(FG_MUTED).bg(BG_STAGE)
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            ellipsize(&text, area.width as usize),
            style,
        )))
        .style(Style::default().bg(BG_STAGE)),
        area,
    );
}

fn render_palette(frame: &mut Frame, app: &App, area: Rect) {
    let rect = centered(56, 12, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(" Command Palette ")
        .style(Style::default().bg(BG_PALETTE).fg(FG_PRIMARY))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    let actions = palette_actions(app);
    let mut lines = vec![Line::from(vec![
        Span::styled("> ", Style::default().fg(ACCENT)),
        Span::raw(app.palette_input.value().to_string()),
    ])];
    lines.push(Line::from(Span::styled(
        "─".repeat(inner.width as usize),
        Style::default().fg(DIVIDER),
    )));
    for (index, (label, _)) in actions.iter().enumerate() {
        let selected = index == app.palette_selected;
        let style = if selected {
            Style::default()
                .bg(BG_SELECT)
                .fg(ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(FG_PRIMARY)
        };
        lines.push(Line::from(vec![
            Span::styled(format!("  {label}"), style),
            Span::styled("  ⏎", Style::default().fg(FG_DIM)),
        ]));
    }
    if actions.is_empty() {
        lines.push(Line::from(Span::styled(
            "  no matches",
            Style::default().fg(FG_MUTED),
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_prompt(frame: &mut Frame, app: &App, kind: PromptKind, area: Rect) {
    let (title, hint) = match kind {
        PromptKind::NewJob => (
            " New job ",
            "command, e.g. npm run dev · Enter starts · Esc cancels",
        ),
        PromptKind::ExportPath => (
            " Export log ",
            "destination path · Enter exports · Esc cancels",
        ),
        PromptKind::WorkspaceFilter => (
            " Workspace filter ",
            "workspace name, or '-' for all · Enter applies · Esc cancels",
        ),
        PromptKind::LogSearch => (" Search logs ", "substring · Enter searches · Esc cancels"),
    };
    let rect = centered(64, 5, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(title)
        .style(Style::default().bg(BG_PALETTE).fg(FG_PRIMARY))
        .border_style(Style::default().fg(ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);

    let lines = vec![
        Line::from(vec![
            Span::styled("> ", Style::default().fg(ACCENT)),
            Span::raw(app.prompt_input.value().to_string()),
        ]),
        Line::from(Span::styled(hint, Style::default().fg(FG_MUTED))),
    ];
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_overlay(frame: &mut Frame, app: &App, kind: OverlayKind, area: Rect) {
    let (title, lines) = overlay_content(app, kind);
    let height = (lines.len() as u16 + 3).min(area.height.max(4) - 1);
    let rect = centered(72, height.max(5), area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .title(format!(" {title} "))
        .style(Style::default().bg(BG_PALETTE).fg(FG_PRIMARY))
        .border_style(Style::default().fg(DIVIDER));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn overlay_content(app: &App, kind: OverlayKind) -> (&'static str, Vec<Line<'static>>) {
    match kind {
        OverlayKind::Digest => {
            let digest = app.digest_cache.as_ref();
            let lines = vec![
                field(
                    "Lines",
                    digest.map_or_else(|| "—".into(), |d| d.total_lines.to_string()),
                ),
                field(
                    "Errors",
                    digest.map_or_else(|| "—".into(), |d| d.errors.to_string()),
                ),
                field(
                    "Warnings",
                    digest.map_or_else(|| "—".into(), |d| d.warnings.to_string()),
                ),
                field(
                    "Last error",
                    digest
                        .and_then(|d| d.last_error.clone())
                        .unwrap_or_else(|| "—".into()),
                ),
            ];
            ("Digest", lines)
        }
        OverlayKind::LogsDiff => {
            let title = if app.log_search_query.is_empty() {
                "Logs (last 200)"
            } else {
                "Logs matching search"
            };
            let lines: Vec<Line> = app
                .log_lines
                .iter()
                .rev()
                .take(20)
                .map(|line| Line::from(ellipsize(line, 68)))
                .collect();
            let lines = if lines.is_empty() {
                vec![Line::from(Span::styled(
                    " no log lines",
                    Style::default().fg(FG_MUTED),
                ))]
            } else {
                lines
            };
            (title, lines)
        }
        OverlayKind::Inspect => {
            let Some(job) = app.selected_job() else {
                return ("Inspect", vec![Line::from(" no job selected")]);
            };
            let stats = app.selected_stats.as_ref();
            let lines = vec![
                field("id", job.id.clone()),
                field("name", job.name.clone().unwrap_or_else(|| "—".into())),
                field("state", job.state.to_string()),
                field("cmd", job.cmd.join(" ")),
                field(
                    "workspace",
                    job.workspace.clone().unwrap_or_else(|| "—".into()),
                ),
                field("mode", if job.pty { "pty" } else { "piped" }),
                field(
                    "cpu / rss / uptime",
                    stats.map_or_else(
                        || "—".into(),
                        |s| format!("{:.1}% / {}MB / {}s", s.cpu_pct, s.rss_mb, s.uptime_secs),
                    ),
                ),
                field("cwd", job.cwd.clone().unwrap_or_else(|| "—".into())),
            ];
            ("Inspect", lines)
        }
        OverlayKind::Help => (
            "Help",
            HELP_LINES
                .iter()
                .map(|line| Line::from((*line).to_string()))
                .collect(),
        ),
    }
}

fn field(label: &str, value: impl AsRef<str>) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{label:<18}"), Style::default().fg(FG_MUTED)),
        Span::raw(ellipsize(value.as_ref(), 48)),
    ])
}

fn centered(width: u16, height: u16, area: Rect) -> Rect {
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn ellipsize(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render_at(width: u16, height: u16) {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();
        app.apply_daemon_event(super::super::daemon::DaemonEvent::JobsUpdated(vec![
            bgrun_proto::JobRecord {
                id: "12345678-abcd".into(),
                name: Some("api-server".into()),
                workspace: None,
                cmd: vec!["cargo".into(), "run".into()],
                pid: Some(42),
                state: bgrun_proto::JobState::Ready,
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
            },
        ]));
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        assert_eq!(app.stage_inner_size.0.max(2), app.stage_inner_size.0);
    }

    #[test]
    fn renders_rail_stage_and_footer_at_100x30() {
        render_at(100, 30);
    }

    #[test]
    fn collapses_rail_at_50x15() {
        let backend = TestBackend::new(50, 15);
        let mut terminal = Terminal::new(backend).unwrap();
        let mut app = App::new();
        assert!(app.show_rail);
        terminal.draw(|frame| render(frame, &mut app)).unwrap();
        assert!(app.stage_inner_size.0 >= 2);
    }
}
