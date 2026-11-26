//! Themed three-zone Ratatui renderer and floating overlays.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Layout, Margin, Position, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, BorderType, Borders, Clear, List, ListItem, Padding, Paragraph},
};
use tui_term::widget::{Cursor, PseudoTerminal};

use super::keys::palette_actions;
use super::settings::{SettingsRow, TuiSettings, settings_path_label};
use super::state::{App, Mode, OverlayKind, PromptKind, is_alive};
use super::theme::{ChromeStyle, Theme};

const HELP_LINES: &[&str] = &[
    "j / ↓  next job          k / ↑  previous job",
    "Enter / i  type into PTY   Ctrl+P Ctrl+Q  detach",
    "Ctrl+K / :  command palette    b  toggle rail",
    "n  new job   r  restart   s  stop   K  kill",
    "l  logs/diff   c  clean terminated   ,  settings",
    "PageUp / PageDown  scrollback     q  quit",
    "",
    "Quitting the TUI leaves the daemon and jobs running.",
];

pub fn render(frame: &mut Frame, app: &mut App) {
    app.expire_toast();
    let theme = Theme::new(app.settings.theme);
    let area = frame.area();
    let outer = if area.width >= 80 && area.height >= 24 {
        area.inner(Margin::new(1, 1))
    } else {
        area
    };
    let gap = u16::from(outer.height >= 16);
    let [body, footer] = Layout::vertical([Constraint::Min(5), Constraint::Length(1)])
        .spacing(gap)
        .areas(outer);

    let (rail_area, stage_area) = if app.show_rail && body.width >= 68 {
        let [rail, stage] = Layout::horizontal([
            Constraint::Length(app.settings.sidebar_width),
            Constraint::Min(28),
        ])
        .spacing(1)
        .areas(body);
        (Some(rail), stage)
    } else {
        (None, body)
    };

    if let Some(rail) = rail_area {
        render_rail(frame, app, rail, &theme);
    } else {
        app.rail_inner_rect = Rect::default();
    }
    render_stage(frame, app, stage_area, &theme);
    render_footer(frame, app, footer, &theme);

    match app.mode {
        Mode::Palette => render_palette(frame, app, area, &theme),
        Mode::Prompt(kind) => render_prompt(frame, app, kind, area, &theme),
        Mode::Overlay(kind) => render_overlay(frame, app, kind, area, &theme),
        Mode::Normal | Mode::PtyType { .. } => {}
    }
}

fn render_rail(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let scope = app.workspace_filter.as_deref().unwrap_or("all");
    let block = match app.settings.chrome {
        ChromeStyle::Rounded => Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(theme.border))
            .title_top(" bgrun · jobs ")
            .title_top(Line::from(format!(" {scope} ")).right_aligned())
            .padding(Padding::new(1, 1, 1, 0)),
        ChromeStyle::Minimal => Block::default()
            .borders(Borders::RIGHT)
            .border_style(Style::default().fg(theme.border))
            .title_top(" bgrun · jobs ")
            .title_top(Line::from(format!(" {scope} ")).right_aligned())
            .padding(Padding::new(1, 2, 1, 0)),
        ChromeStyle::Borderless => Block::default()
            .title_top(" bgrun · jobs ")
            .title_top(Line::from(format!(" {scope} ")).right_aligned())
            .padding(Padding::new(1, 1, 1, 0)),
    }
    .style(Style::default().bg(theme.bg).fg(theme.fg));
    let inner = block.inner(area);
    app.rail_inner_rect = inner;
    frame.render_widget(block, area);

    let [list_area, summary_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(2)]).areas(inner);
    if app.jobs.is_empty() {
        frame.render_widget(
            Paragraph::new(" No jobs yet · press n").style(Style::default().fg(theme.fg_muted)),
            list_area,
        );
    } else {
        let width = list_area.width as usize;
        let items: Vec<ListItem> = app
            .jobs
            .iter()
            .map(|job| ListItem::new(rail_row(job, width, theme)))
            .collect();
        let list = List::new(items)
            .style(Style::default().fg(theme.fg))
            .highlight_style(theme.selection)
            .highlight_symbol("");
        frame.render_stateful_widget(list, list_area, &mut app.rail_state);
    }
    render_rail_summary(frame, app, summary_area, theme);
}

fn rail_row(job: &bgrun_proto::JobRecord, width: usize, theme: &Theme) -> Line<'static> {
    let (glyph, color) = match job.state {
        bgrun_proto::JobState::Ready => ("●", theme.ok),
        bgrun_proto::JobState::Running | bgrun_proto::JobState::Starting => ("●", theme.accent),
        bgrun_proto::JobState::Crashed => ("●", theme.err),
        bgrun_proto::JobState::Exited | bgrun_proto::JobState::Killed => ("○", theme.fg_muted),
    };
    let name = job.name.clone().unwrap_or_else(|| short_id(&job.id));
    let tag = compact_tag(job).filter(|tag| width >= 2 + text_width(tag) + 2 + 4);
    let reserved = 2 + tag.as_ref().map_or(0, |tag| text_width(tag) + 2);
    let name = ellipsize(&name, width.saturating_sub(reserved));
    let mut spans = vec![
        Span::styled(format!("{glyph} "), Style::default().fg(color)),
        Span::raw(name),
    ];
    if let Some(tag) = tag {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(tag, Style::default().fg(theme.fg_subtle)));
    }
    let used = Line::from(spans.clone()).width();
    spans.push(Span::raw(" ".repeat(width.saturating_sub(used))));
    Line::from(spans)
}

fn compact_tag(job: &bgrun_proto::JobRecord) -> Option<String> {
    if let Some(port) = job.allocated_port {
        return Some(format!(":{port}"));
    }
    if job.restart.is_some() {
        return Some(
            job.max_retries
                .map_or_else(|| "↻".into(), |retries| format!("↻{retries}")),
        );
    }
    (!job.pty).then(|| "pipe".into())
}

fn render_rail_summary(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let width = area.width as usize;
    frame.render_widget(
        Paragraph::new("─".repeat(width)).style(Style::default().fg(theme.border)),
        Rect { height: 1, ..area },
    );
    if area.height < 2 {
        return;
    }
    let alive = app.jobs.iter().filter(|job| is_alive(&job.state)).count();
    let errors = app
        .jobs
        .iter()
        .filter(|job| matches!(job.state, bgrun_proto::JobState::Crashed))
        .count();
    let done = app.jobs.len() - alive;
    let summary = format!(" {alive} alive · {errors} err · {done} done ");
    let new_hint = " + n ";
    let left = ellipsize(&summary, width.saturating_sub(text_width(new_hint)));
    let padding = width.saturating_sub(text_width(&left) + text_width(new_hint));
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled(left, Style::default().fg(theme.fg_muted)),
            Span::raw(" ".repeat(padding)),
            Span::styled(new_hint, Style::default().fg(theme.accent)),
        ])),
        Rect {
            y: area.y + 1,
            height: 1,
            ..area
        },
    );
}

fn render_stage(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let is_typing = matches!(app.mode, Mode::PtyType { .. });
    let border_color = if is_typing {
        theme.border_focus
    } else {
        theme.border
    };
    let block = match app.settings.chrome {
        ChromeStyle::Rounded => Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::default().fg(border_color))
            .padding(Padding::new(2, 2, 1, 0)),
        ChromeStyle::Minimal | ChromeStyle::Borderless => Block::default()
            .border_style(Style::default().fg(border_color))
            .padding(if app.settings.chrome == ChromeStyle::Minimal {
                Padding::new(2, 1, 1, 0)
            } else {
                Padding::new(2, 2, 1, 1)
            }),
    }
    .style(Style::default().bg(theme.bg).fg(theme.fg));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let [header, pty_area] =
        Layout::vertical([Constraint::Length(2), Constraint::Min(1)]).areas(inner);

    render_stage_header(frame, app, header, is_typing, theme);
    app.stage_pty_rect = pty_area;
    app.set_stage_size(pty_area.width, pty_area.height);

    if app.selected_job().is_none() {
        frame.render_widget(
            Paragraph::new("No jobs running — press n to start one or Ctrl+K for commands")
                .alignment(Alignment::Center)
                .style(Style::default().fg(theme.fg_muted)),
            pty_area,
        );
        return;
    }

    let cursor = if is_typing {
        Cursor::default()
            .style(Style::default().bg(theme.accent).fg(theme.bg))
            .visibility(true)
    } else if app.settings.view_cursor == super::settings::ViewCursor::Dim
        && app.selected_is_alive_pty()
    {
        Cursor::default()
            .style(Style::default().fg(theme.fg_subtle))
            .visibility(true)
    } else {
        Cursor::default().visibility(false)
    };
    let terminal = PseudoTerminal::new(app.parser.screen())
        .cursor(cursor)
        .style(Style::default().bg(theme.bg).fg(theme.fg));
    frame.render_widget(terminal, pty_area);
}

fn render_stage_header(frame: &mut Frame, app: &App, area: Rect, is_typing: bool, theme: &Theme) {
    if area.height == 0 || area.width == 0 {
        return;
    }
    let [left, badge] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(8)]).areas(area);
    let header = stage_header_line(app, theme);
    frame.render_widget(Paragraph::new(header), left);
    let badge_style = if is_typing {
        Style::default()
            .fg(theme.accent)
            .bg(theme.bg)
            .add_modifier(Modifier::REVERSED | Modifier::BOLD)
    } else {
        Style::default().fg(theme.fg_muted)
    };
    frame.render_widget(
        Paragraph::new(if is_typing { " TYPE " } else { " VIEW " })
            .alignment(Alignment::Center)
            .style(badge_style),
        badge,
    );
    if area.height >= 2 {
        frame.render_widget(
            Paragraph::new("─".repeat(area.width as usize)).style(Style::default().fg(
                if is_typing {
                    theme.border_focus
                } else {
                    theme.border
                },
            )),
            Rect {
                y: area.y + 1,
                height: 1,
                ..area
            },
        );
    }
}

fn stage_header_line(app: &App, theme: &Theme) -> Line<'static> {
    let Some(job) = app.selected_job() else {
        return Line::from(Span::styled(
            " no job selected · press n to start one ",
            Style::default().fg(theme.fg_muted),
        ));
    };
    let name = job.name.clone().unwrap_or_else(|| short_id(&job.id));
    let (glyph, state_color) = match job.state {
        bgrun_proto::JobState::Ready => ("●", theme.ok),
        bgrun_proto::JobState::Running | bgrun_proto::JobState::Starting => ("●", theme.accent),
        bgrun_proto::JobState::Crashed => ("●", theme.err),
        bgrun_proto::JobState::Exited | bgrun_proto::JobState::Killed => ("○", theme.fg_muted),
    };
    let stats = app
        .selected_stats
        .as_ref()
        .map_or_else(String::new, |stats| {
            format!(" · {:.1}% cpu · {} MB", stats.cpu_pct, stats.rss_mb)
        });
    Line::from(vec![
        Span::styled(
            format!(" {name} "),
            Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("{glyph} {} ", job.state),
            Style::default().fg(state_color),
        ),
        Span::styled(
            format!(
                "· pid {} · {} · {}{stats} ",
                job.pid.map_or_else(|| "—".into(), |pid| pid.to_string()),
                if job.pty { "pty" } else { "piped" },
                if is_alive(&job.state) { "live" } else { "done" },
            ),
            Style::default().fg(theme.fg_muted),
        ),
    ])
}

fn render_footer(frame: &mut Frame, app: &App, area: Rect, theme: &Theme) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let inner = area.inner(Margin::new(1, 0));
    let [left, right] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(30)]).areas(inner);
    let left_line = if matches!(app.mode, Mode::PtyType { .. }) {
        Line::from(Span::styled(
            "All keys forwarded to PTY · Ctrl+P Ctrl+Q return to nav",
            Style::default().fg(theme.accent),
        ))
    } else if let Some((toast, _)) = &app.toast {
        Line::from(Span::styled(toast, Style::default().fg(theme.warn)))
    } else {
        let mut spans = Vec::new();
        if app.daemon_idle {
            spans.push(Span::styled(
                "daemon idle · ",
                Style::default().fg(theme.fg_subtle),
            ));
        }
        spans.extend(footer_hints(theme));
        Line::from(spans)
    };
    frame.render_widget(Paragraph::new(left_line), left);
    frame.render_widget(
        Paragraph::new(Line::from(footer_right(theme)).alignment(Alignment::Right)),
        right,
    );
}

fn footer_hints(theme: &Theme) -> Vec<Span<'static>> {
    [
        ("j/k", " jobs"),
        ("enter", " type"),
        ("n", " new"),
        ("r", " restart"),
        ("s", " stop"),
        ("ctrl+k", " commands"),
    ]
    .into_iter()
    .enumerate()
    .map(|(index, (key, label))| {
        let separator = if index == 0 { "" } else { " · " };
        vec![
            Span::styled(separator, Style::default().fg(theme.fg_subtle)),
            Span::styled(
                key,
                Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
            ),
            Span::styled(label, Style::default().fg(theme.fg_muted)),
        ]
    })
    .collect::<Vec<_>>()
    .into_iter()
    .flatten()
    .collect()
}

fn footer_right(theme: &Theme) -> Vec<Span<'static>> {
    [("b", " rail"), (",", " settings"), ("q", " quit")]
        .into_iter()
        .enumerate()
        .map(|(index, (key, label))| {
            let separator = if index == 0 { "" } else { " · " };
            vec![
                Span::styled(separator, Style::default().fg(theme.fg_subtle)),
                Span::styled(
                    key,
                    Style::default().fg(theme.fg).add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, Style::default().fg(theme.fg_muted)),
            ]
        })
        .collect::<Vec<_>>()
        .into_iter()
        .flatten()
        .collect()
}

fn render_palette(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let actions = palette_actions(app);
    let height = (actions.len() as u16 + 7).max(7);
    let rect = modal_rect(64, height, area);
    let block = modal_block(" Command Palette ", theme);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let [input_area, divider, list_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(1),
    ])
    .areas(inner);

    let scroll = app
        .palette_input
        .visual_scroll(input_area.width.saturating_sub(2) as usize) as u16;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::raw(app.palette_input.value()),
        ]))
        .scroll((0, scroll)),
        input_area,
    );
    frame.render_widget(
        Paragraph::new("─".repeat(divider.width as usize)).style(Style::default().fg(theme.border)),
        divider,
    );
    let items: Vec<ListItem> = actions
        .iter()
        .map(|(label, _)| ListItem::new(format!("  {label}  ⏎")))
        .collect();
    if items.is_empty() {
        frame.render_widget(
            Paragraph::new("  no matches").style(Style::default().fg(theme.fg_muted)),
            list_area,
        );
    } else {
        let list = List::new(items)
            .style(Style::default().fg(theme.fg))
            .highlight_style(theme.selection)
            .highlight_symbol("");
        frame.render_stateful_widget(list, list_area, &mut app.palette_state);
    }
    set_input_cursor(frame, inner, input_area, &app.palette_input, scroll, 2);
}

fn render_prompt(frame: &mut Frame, app: &App, kind: PromptKind, area: Rect, theme: &Theme) {
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
    let rect = modal_rect(68, 7, area);
    let block = modal_block(title, theme);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let [input_area, hint_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);
    let scroll = app
        .prompt_input
        .visual_scroll(input_area.width.saturating_sub(2) as usize) as u16;
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("> ", Style::default().fg(theme.accent)),
            Span::raw(app.prompt_input.value()),
        ]))
        .scroll((0, scroll)),
        input_area,
    );
    frame.render_widget(
        Paragraph::new(hint).style(Style::default().fg(theme.fg_muted)),
        hint_area,
    );
    set_input_cursor(frame, inner, input_area, &app.prompt_input, scroll, 2);
}

fn set_input_cursor(
    frame: &mut Frame,
    modal_inner: Rect,
    input_area: Rect,
    input: &tui_input::Input,
    scroll: u16,
    prefix: u16,
) {
    if input_area.width <= prefix || input_area.height == 0 || modal_inner.width <= prefix {
        return;
    }
    let visual = input.visual_cursor() as u16;
    let x = input_area
        .x
        .saturating_add(prefix)
        .saturating_add(visual.saturating_sub(scroll))
        .min(modal_inner.right().saturating_sub(1));
    let y = input_area.y.min(modal_inner.bottom().saturating_sub(1));
    frame.set_cursor_position(Position { x, y });
}

fn render_overlay(frame: &mut Frame, app: &mut App, kind: OverlayKind, area: Rect, theme: &Theme) {
    match kind {
        OverlayKind::Settings => render_settings(frame, app, area, theme),
        OverlayKind::LogsDiff => render_logs(frame, app, area, theme),
        OverlayKind::Digest | OverlayKind::Inspect | OverlayKind::Help => {
            render_static_overlay(frame, app, kind, area, theme);
        }
    }
}

fn render_static_overlay(
    frame: &mut Frame,
    app: &App,
    kind: OverlayKind,
    area: Rect,
    theme: &Theme,
) {
    let (title, lines) = overlay_content(app, kind);
    let rect = modal_rect(76, lines.len() as u16 + 6, area);
    let block = modal_block(title, theme);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);
}

fn render_settings(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let rect = modal_rect(72, 14, area);
    let saved_label = format!(" saved to {} ", settings_path_label());
    let saved_label = ellipsize(&saved_label, (rect.width as usize).saturating_sub(24));
    let block = modal_block(" Settings ", theme).title_top(Line::from(saved_label).right_aligned());
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let [list_area, divider, hint_area] = Layout::vertical([
        Constraint::Min(1),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .areas(inner);
    let items: Vec<ListItem> = SettingsRow::ALL
        .into_iter()
        .map(|row| {
            ListItem::new(settings_row_line(
                &app.settings,
                row,
                list_area.width as usize,
            ))
        })
        .collect();
    let list = List::new(items)
        .style(Style::default().fg(theme.fg))
        .highlight_style(theme.selection)
        .highlight_symbol("");
    frame.render_stateful_widget(list, list_area, &mut app.settings_state);
    frame.render_widget(
        Paragraph::new("─".repeat(divider.width as usize)).style(Style::default().fg(theme.border)),
        divider,
    );
    frame.render_widget(
        Paragraph::new(" ↑↓ move · ←→ / space change · esc close ")
            .style(Style::default().fg(theme.fg_muted)),
        hint_area,
    );
}

fn settings_row_line(settings: &TuiSettings, row: SettingsRow, width: usize) -> Line<'static> {
    let value = format!(" ◀ {} ▶ ", settings.value_label(row));
    let label = row.label();
    let value_width = text_width(&value);
    let label_width = width.saturating_sub(value_width + 2);
    let gap = label_width.saturating_sub(text_width(label));
    Line::from(vec![
        Span::raw(format!(" {label}")),
        Span::raw(" ".repeat(gap)),
        Span::raw(value),
    ])
}

fn render_logs(frame: &mut Frame, app: &mut App, area: Rect, theme: &Theme) {
    let title = if app.log_search_query.is_empty() {
        " Logs (last 200) "
    } else {
        " Logs matching search "
    };
    let rect = modal_rect(82, 24, area);
    let block = modal_block(title, theme);
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    let [list_area, hint_area] =
        Layout::vertical([Constraint::Min(1), Constraint::Length(1)]).areas(inner);
    let viewport = list_area.height as usize;
    app.log_viewport_height = viewport;
    let max_scroll = app.log_lines.len().saturating_sub(viewport);
    app.log_scroll = app.log_scroll.min(max_scroll);
    let end = app.log_lines.len().saturating_sub(app.log_scroll);
    let start = end.saturating_sub(viewport);
    let visible = &app.log_lines[start..end];
    let blank = viewport.saturating_sub(visible.len());
    let mut lines = vec![Line::default(); blank];
    lines.extend(visible.iter().map(|line| Line::from(line.clone())));
    if app.log_lines.is_empty() {
        lines.push(Line::from(Span::styled(
            " no log lines",
            Style::default().fg(theme.fg_muted),
        )));
    }
    frame.render_widget(Paragraph::new(lines), list_area);
    frame.render_widget(
        Paragraph::new(" j/k scroll · PgUp/PgDn · g/G · q close ")
            .style(Style::default().fg(theme.fg_muted)),
        hint_area,
    );
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
            (" Digest ", lines)
        }
        OverlayKind::LogsDiff | OverlayKind::Settings => unreachable!("rendered separately"),
        OverlayKind::Inspect => {
            let Some(job) = app.selected_job() else {
                return (" Inspect ", vec![Line::from(" no job selected")]);
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
            (" Inspect ", lines)
        }
        OverlayKind::Help => (
            " Help ",
            HELP_LINES
                .iter()
                .map(|line| Line::from((*line).to_string()))
                .collect(),
        ),
    }
}

fn field(label: &str, value: impl AsRef<str>) -> Line<'static> {
    Line::from(vec![
        Span::raw(format!(" {label:<18}")),
        Span::raw(value.as_ref().to_owned()),
    ])
}

fn modal_block(title: &'static str, theme: &Theme) -> Block<'static> {
    Block::bordered()
        .border_type(BorderType::Rounded)
        .title_top(title)
        .padding(Padding::new(2, 2, 1, 1))
        .style(Style::default().bg(theme.popup_bg).fg(theme.fg))
        .border_style(Style::default().fg(theme.border_focus))
}

fn modal_rect(width: u16, height: u16, area: Rect) -> Rect {
    let width = width.max(1).min(area.width.saturating_sub(4).max(1));
    let height = height.max(1).min(area.height.saturating_sub(2).max(1));
    Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width,
        height,
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn text_width(text: &str) -> usize {
    Line::from(text).width()
}

fn ellipsize(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if text_width(value) <= max {
        return value.to_string();
    }
    let mut out = String::new();
    for ch in value.chars() {
        let next_len = text_width(&format!("{out}{ch}"));
        if next_len + 1 > max {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::tui::daemon::DaemonEvent;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;
    use ratatui::style::Color;
    use tui_input::Input;

    fn record(name: &str, pty: bool) -> bgrun_proto::JobRecord {
        bgrun_proto::JobRecord {
            id: format!("{name}-12345678"),
            name: Some(name.into()),
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
            pty,
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

    fn app_with_jobs() -> App {
        let mut app = App::new();
        let mut worker = record("worker-queue", false);
        worker.state = bgrun_proto::JobState::Exited;
        app.apply_daemon_event(DaemonEvent::JobsUpdated(vec![
            record("website-dev", true),
            worker,
        ]));
        app
    }

    fn render_app(width: u16, height: u16, app: &mut App) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|frame| render(frame, app)).unwrap();
        terminal
    }

    fn buffer_text(buffer: &Buffer, area: Rect) -> String {
        let mut out = String::new();
        for y in area.y..area.bottom() {
            for x in area.x..area.right() {
                out.push_str(buffer[(x, y)].symbol());
            }
            out.push('\n');
        }
        out
    }

    #[test]
    fn renders_required_sizes_and_overlays_without_panics() {
        for (width, height) in [(120, 36), (80, 24), (50, 15), (100, 30), (70, 20)] {
            let mut app = app_with_jobs();
            render_app(width, height, &mut app);
            for mode in [
                Mode::Palette,
                Mode::Overlay(OverlayKind::Settings),
                Mode::Overlay(OverlayKind::LogsDiff),
                Mode::Overlay(OverlayKind::Help),
            ] {
                app.mode = mode;
                render_app(width, height, &mut app);
            }
        }
    }

    #[test]
    fn collapses_rail_at_50x15() {
        let mut app = app_with_jobs();
        assert!(app.show_rail);
        render_app(50, 15, &mut app);
        assert_eq!(app.rail_inner_rect, Rect::default());
        assert!(app.stage_inner_size.0 >= 2);
    }

    #[test]
    fn terminal_theme_never_paints_rgb_backgrounds() {
        let mut app = app_with_jobs();
        let terminal = render_app(100, 30, &mut app);
        assert!(
            terminal
                .backend()
                .buffer()
                .content
                .iter()
                .all(|cell| { !matches!(cell.style().bg, Some(Color::Rgb(_, _, _))) })
        );
    }

    #[test]
    fn rail_rows_keep_status_and_mode_tags_at_default_width() {
        let mut app = app_with_jobs();
        let terminal = render_app(100, 30, &mut app);
        let text = buffer_text(terminal.backend().buffer(), app.rail_inner_rect);
        assert!(text.contains("● website-dev"));
        assert!(text.contains("○ worker-queue"));
        assert!(text.contains("pipe"));
    }

    #[test]
    fn logs_render_oldest_to_newest_and_anchor_at_bottom() {
        let mut app = app_with_jobs();
        app.log_lines = vec!["oldest".into(), "middle".into(), "newest".into()];
        app.mode = Mode::Overlay(OverlayKind::LogsDiff);
        let terminal = render_app(100, 30, &mut app);
        let text = buffer_text(
            terminal.backend().buffer(),
            terminal.backend().buffer().area,
        );
        let oldest = text.find("oldest").unwrap();
        let newest = text.find("newest").unwrap();
        assert!(oldest < newest);
    }

    #[test]
    fn settings_title_and_prompt_cursor_render_cleanly() {
        let mut app = app_with_jobs();
        app.mode = Mode::Overlay(OverlayKind::Settings);
        let terminal = render_app(100, 30, &mut app);
        let text = buffer_text(
            terminal.backend().buffer(),
            terminal.backend().buffer().area,
        );
        assert!(text.contains("Settings"));

        app.mode = Mode::Prompt(PromptKind::NewJob);
        app.prompt_input = Input::new("echo hi".into());
        let terminal = render_app(100, 30, &mut app);
        assert!(terminal.backend().cursor_visible());
        assert!(terminal.backend().cursor_position().x > 0);
    }

    #[test]
    fn every_theme_renders_and_fits_in_settings() {
        let mut app = app_with_jobs();
        app.mode = Mode::Overlay(OverlayKind::Settings);
        for kind in crate::tui::theme::ThemeKind::ALL {
            app.settings.theme = kind;
            let terminal = render_app(70, 20, &mut app);
            let text = buffer_text(
                terminal.backend().buffer(),
                terminal.backend().buffer().area,
            );
            assert!(
                text.contains(kind.label()),
                "theme {} was clipped",
                kind.label()
            );
        }
    }

    #[test]
    fn every_palette_item_can_be_selected_and_rendered() {
        let mut app = App::new();
        let count = palette_actions(&app).len();
        app.mode = Mode::Palette;
        for index in 0..count {
            app.palette_state.select(Some(index));
            render_app(70, 20, &mut app);
        }
    }
}
