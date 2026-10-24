//! Drawing the run screen: the form, or the run and its log, with the
//! command line and the keys at the foot.

use super::app::{App, Pane};
use super::worker::Ended;
use crate::tui::{session_keys, RUN_KEY};
use legume_tui::child::gauge;
use legume_tui::form::{row_line, Kind};
use legume_tui::look::Look;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Wrap};
use ratatui::Frame;

pub fn draw(f: &mut Frame, app: &App) {
    let look = Look::terminal();
    let [head, body, cmd, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(4),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let title = format!("chickpea run · {} — {}", app.step.name, app.step.about);
    f.render_widget(Paragraph::new(Span::styled(title, look.bold)), head);
    match app.pane {
        Pane::Form => form(f, app, body, &look),
        Pane::Run => run(f, app, body, &look),
    }
    command(f, app, cmd, &look);
    f.render_widget(Paragraph::new(Span::styled(keys(app), look.hint)), foot);
}

fn form(f: &mut Frame, app: &App, area: Rect, look: &Look) {
    let [rows, help] = Layout::vertical([Constraint::Min(3), Constraint::Length(4)]).areas(area);
    let width = app
        .form
        .fields
        .iter()
        .map(|f| f.long.len())
        .max()
        .unwrap_or(0)
        + 2;
    let complaint = app.check.as_ref().err();
    let blamed = complaint.and_then(|c| legume_tui::form::blamed(c, &app.form.fields));
    let shown_rows = rows.height.saturating_sub(2) as usize;
    let first = app.at.saturating_sub(shown_rows.saturating_sub(1));
    let lines: Vec<Line> = app
        .form
        .fields
        .iter()
        .enumerate()
        .skip(first)
        .take(shown_rows)
        .map(|(i, field)| {
            let at = i == app.at;
            let shown = match (&app.editing, at) {
                (Some(text), true) => format!("{text}▏"),
                _ => field.shown(),
            };
            let style = if at { look.selected } else { look.page };
            let blame = (blamed == Some(field.long.as_str())).then_some("← here");
            row_line(field, at, width, look, style, shown, blame)
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border)
        .title(format!(" flags ({} changed) ", app.form.changed()));
    f.render_widget(Paragraph::new(lines).block(block), rows);
    let field = &app.form.fields[app.at];
    let how = match field.kind {
        Kind::Flag { .. } => "enter toggles",
        Kind::Choice(_) => "←/→ or enter changes",
        Kind::Text => "enter types a value (empty: default)",
    };
    let text = vec![
        Line::from(vec![
            Span::styled(format!("--{}", field.long), look.bold),
            Span::styled(format!("  {how}; d resets"), look.hint),
        ]),
        Line::from(field.long_help.clone()),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), help);
}

fn run(f: &mut Frame, app: &App, area: Rect, look: &Look) {
    let Some(job) = &app.job else {
        return;
    };
    let log = job.log();
    let [bar, rest] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(area);
    let state = match &log.end {
        None => match log.bar.as_ref().filter(|b| b.visible()) {
            Some(b) => {
                let w = bar.width as usize;
                gauge(
                    &b.now,
                    (w / 3).max(10),
                    &b.trend,
                    w.saturating_sub(w / 3 + 20),
                )
            }
            None => "running …".into(),
        },
        Some(Ok(())) => "done".into(),
        Some(Err(Ended::Stopped)) => "stopped".into(),
        Some(Err(Ended::Failed(why))) => format!("failed: {why}"),
    };
    let style = if matches!(log.end, Some(Err(_))) {
        look.bold
    } else {
        look.page
    };
    f.render_widget(Paragraph::new(Span::styled(state, style)), bar);
    let room = rest.height.saturating_sub(2) as usize;
    let n = log.lines.len();
    let end = n.saturating_sub(app.scroll.min(n));
    let start = end.saturating_sub(room);
    let lines: Vec<Line> = log
        .lines
        .range(start..end)
        .map(|l| Line::from(l.as_str()))
        .collect();
    let title = if app.scroll > 0 {
        format!(" log ({} lines back) ", app.scroll)
    } else {
        " log ".into()
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border)
        .title(title);
    f.render_widget(Paragraph::new(lines).block(block), rest);
}

fn command(f: &mut Frame, app: &App, area: Rect, look: &Look) {
    let mut text = vec![Line::from(vec![
        Span::styled("$ chickpea ", look.hint),
        Span::raw(legume_mcp::command_line(&app.command())),
    ])];
    match (&app.note, &app.check) {
        (Some(note), _) => text.push(Line::from(Span::styled(note.clone(), look.bold))),
        (None, Err(why)) if !why.is_empty() => {
            text.push(Line::from(Span::styled(
                format!("not yet: {why}"),
                look.hint,
            )));
        }
        (None, Ok(made)) => {
            let first = made
                .first()
                .map(|p| crate::tui::shown(p))
                .unwrap_or_default();
            text.push(Line::from(Span::styled(
                format!("writes {first} and {} more", made.len().saturating_sub(1)),
                look.hint,
            )));
        }
        _ => {}
    }
    let block = Block::default()
        .borders(Borders::TOP)
        .border_style(look.border);
    f.render_widget(
        Paragraph::new(text)
            .wrap(Wrap { trim: false })
            .block(block)
            .style(Style::default().remove_modifier(Modifier::REVERSED)),
        area,
    );
}

fn keys(app: &App) -> String {
    let mut keys: Vec<(String, String)> = match app.pane {
        Pane::Form => vec![
            ("↑↓".into(), "move".into()),
            ("enter".into(), "change".into()),
            (RUN_KEY.into(), "run".into()),
        ],
        Pane::Run => vec![
            ("↑↓".into(), "scroll".into()),
            ("x".into(), "stop".into()),
            ("e".into(), "flags".into()),
        ],
    };
    if app.pane == Pane::Form && app.job.is_some() {
        keys.push(("tab".into(), "run".into()));
    }
    keys.push(("q".into(), "quit".into()));
    keys.extend(session_keys());
    keys.iter()
        .map(|(k, what)| format!("{k} {what}"))
        .collect::<Vec<_>>()
        .join(" · ")
}
