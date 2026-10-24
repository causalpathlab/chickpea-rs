//! Drawing the run screen: the count files, the form, or the run and its
//! log, with the command line and the keys at the foot, and the file
//! browser over them when it is open.

use super::app::{App, Drawn, Pane};
use super::worker::Ended;
use crate::tui::{key_line, ListHit, RUN_KEY};
use legume_tui::browse::first_visible;
use legume_tui::child::gauge;
use legume_tui::chrome::tail;
use legume_tui::form::{row_line, Kind};
use legume_tui::look::Look;
use ratatui::layout::{Constraint, Layout, Margin, Rect};
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
    let mut drawn = Drawn::default();
    tabs(f, app, head, &look, &mut drawn);
    match app.pane {
        Pane::Data => data(f, app, body, &look, &mut drawn),
        Pane::Form => form(f, app, body, &look, &mut drawn),
        Pane::Run => run(f, app, body, &look),
    }
    *app.drawn.borrow_mut() = drawn;
    command(f, app, cmd, &look);
    f.render_widget(Paragraph::new(Span::styled(keys(app), look.hint)), foot);
    if let Some(pick) = &app.pick {
        pick.draw(f, f.area(), &look);
    }
}

/// The head line: the step, and a tab per pane (the one shown bold).
fn tabs(f: &mut Frame, app: &App, area: Rect, look: &Look, drawn: &mut Drawn) {
    let title = format!("chickpea run · {}  ", app.step.name);
    let mut x = area.x + title.chars().count() as u16;
    let mut spans = vec![Span::styled(title, look.bold)];
    let samples = app.data.pairs.len();
    let names = [
        (Pane::Data, format!(" data ({samples}) ")),
        (
            Pane::Form,
            format!(" flags ({} changed) ", app.form.changed()),
        ),
        (Pane::Run, " run ".to_string()),
    ];
    for (pane, name) in names {
        if pane == Pane::Run && app.job.is_none() {
            continue;
        }
        let w = name.chars().count() as u16;
        drawn.tabs.push((pane, x..x + w));
        x += w + 1;
        let style = if pane == app.pane {
            look.selected
        } else {
            look.hint
        };
        spans.push(Span::styled(name, style));
        spans.push(Span::raw(" "));
    }
    spans.push(Span::styled(format!(" {}", app.step.about), look.hint));
    f.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// The count files: a row per sample, its name and each side's file.
fn data(f: &mut Frame, app: &App, area: Rect, look: &Look, drawn: &mut Drawn) {
    let [rows, help] = Layout::vertical([Constraint::Min(3), Constraint::Length(3)]).areas(area);
    let d = &app.data;
    // Files of one folder by name, the folder in the title.
    let mut folders = d
        .pairs
        .iter()
        .flat_map(super::data::Pair::files)
        .map(|p| p.parent().unwrap_or(std::path::Path::new("")));
    let first_folder = folders.next();
    let common = first_folder.filter(|f| folders.all(|g| g == *f));
    let title = match common {
        Some(dir) => format!(
            " count files, a row per sample · in {} ",
            legume_tui::browse::tilde(&legume_tui::browse::normalize(dir))
        ),
        None => " count files, a row per sample ".into(),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border)
        .title(title);
    let inner = block.inner(rows);
    f.render_widget(block, rows);
    let names = d.names();
    let name_w = names
        .iter()
        .flatten()
        .map(|n| n.chars().count())
        .max()
        .unwrap_or(0)
        .max(6)
        + 2;
    let lead = 4 + name_w as u16;
    let n_sides = d.sides.len() as u16;
    let col_w = inner.width.saturating_sub(lead) / n_sides.max(1);
    drawn.cols = (0..n_sides).map(|k| inner.x + lead + k * col_w).collect();
    // A head line, then the rows.
    let [titles, list] = Layout::vertical([Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    let mut head = vec![Span::styled(
        format!("{:<w$}", "  # sample", w = lead as usize),
        look.hint,
    )];
    for s in d.sides {
        head.push(Span::styled(
            format!("{:<w$}", s.title(), w = col_w as usize),
            look.hint,
        ));
    }
    f.render_widget(Paragraph::new(Line::from(head)), titles);
    let shown = list.height as usize;
    let total = d.pairs.len() + 1;
    let first = first_visible(d.at, total, shown);
    drawn.list = ListHit { rows: list, first };
    let cut = |p: &std::path::Path, w: usize| -> String {
        let s = match (common, p.file_name()) {
            (Some(_), Some(name)) => name.to_string_lossy().into_owned(),
            _ => crate::tui::shown(p),
        };
        tail(&s, w)
    };
    let lines: Vec<Line> = (first..total.min(first + shown))
        .map(|i| {
            let mark = if i == d.at { "▸ " } else { "  " };
            let Some(p) = d.pairs.get(i) else {
                let style = if i == d.at { look.selected } else { look.hint };
                return Line::from(vec![
                    Span::styled(mark, look.bold),
                    Span::styled("+ add files (a): space marks several in the browser", style),
                ]);
            };
            let name = names[i].clone().unwrap_or_else(|| "—".into());
            let mut spans = vec![
                Span::styled(mark, look.bold),
                Span::raw(format!("{:<2}", i + 1)),
                Span::raw(format!("{name:<name_w$}")),
            ];
            for (c, &side) in d.sides.iter().enumerate() {
                let w = col_w.saturating_sub(1) as usize;
                let (text, style) = match p.get(side) {
                    Some(path) => (cut(path, w), look.page),
                    None => ("(none: enter browses)".into(), look.hint),
                };
                let style = if i == d.at && c == d.col {
                    look.selected
                } else {
                    style
                };
                spans.push(Span::styled(format!("{text:<w$}"), style));
                spans.push(Span::raw(" "));
            }
            Line::from(spans)
        })
        .collect();
    f.render_widget(Paragraph::new(lines), list);
    let about = match d.sides {
        [_] => "Each sample's ATAC, in the order peak-to-gene was given them.",
        _ => {
            "Each sample's RNA and ATAC, for the same cells (one file of both is in both \
             columns). With several samples a cell is {barcode}@{sample}, the sample named \
             after its files, and each sample is a batch."
        }
    };
    let text = vec![
        Line::from(Span::styled(
            "a adds files · enter changes the file under the cursor · ←→ column · x removes",
            look.hint,
        )),
        Line::from(about),
    ];
    f.render_widget(Paragraph::new(text).wrap(Wrap { trim: true }), help);
}

fn form(f: &mut Frame, app: &App, area: Rect, look: &Look, drawn: &mut Drawn) {
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
    let inner = rows.inner(Margin::new(1, 1));
    let shown_rows = inner.height as usize;
    let first = first_visible(app.at, app.form.fields.len(), shown_rows);
    drawn.list = ListHit { rows: inner, first };
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
        .title(" flags ");
    f.render_widget(Paragraph::new(lines).block(block), rows);
    let Some(field) = app.form.fields.get(app.at) else {
        return;
    };
    let how = match field.kind {
        Kind::Flag { .. } => "enter toggles",
        Kind::Choice(_) => "←/→ or enter changes",
        Kind::Text if app.files.contains(&field.long) => {
            "enter browses for the file, e types it (empty: default)"
        }
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
        Pane::Data => vec![
            ("a".into(), "add files".into()),
            ("enter".into(), "change".into()),
            (RUN_KEY.into(), "run".into()),
        ],
        Pane::Form => vec![
            ("↑↓".into(), "move".into()),
            ("enter".into(), "change".into()),
            (RUN_KEY.into(), "run".into()),
        ],
        Pane::Run => vec![("↑↓".into(), "scroll".into()), ("x".into(), "stop".into())],
    };
    keys.push(("tab".into(), "next pane".into()));
    keys.push(("q".into(), "quit".into()));
    key_line(keys)
}
