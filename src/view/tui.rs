//! The view screen: genes ranked by their links on the left; on the right
//! the chosen gene's locus, or (tab) the genome-wide view with that gene
//! ringed.

use super::data::{Measure, Summary};
use super::State;
use crate::tui::{click, key_line, wheel, ListHit, TOOL};
use legume_tui::look::Look;
use legume_tui::Round;
use ratatui::crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, MouseEvent};
use ratatui::layout::{Constraint, Layout, Margin, Rect};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;
use std::time::Duration;

const TICK: Duration = Duration::from_millis(200);

/// The screen's own state beside what is looked at.
/// Where the last draw put the gene list and the figure, for the mouse.
#[derive(Clone, Copy, Debug, Default)]
struct Drawn {
    genes: ListHit,
    figure: Rect,
}

struct Screen {
    /// The right pane shows the genome-wide view (else the locus).
    genome: bool,
    /// How the attached terminal shows pictures.
    picker: Option<ratatui_image::picker::Picker>,
    /// The locus as a figure (else as a table).
    figure: bool,
    /// The figure in characters, not as a picture: on by itself where the
    /// terminal shows only half blocks.
    text: bool,
    /// The figure as last drawn, for the area and settings it was drawn for.
    shown: Option<(Rect, Key, ratatui_image::protocol::Protocol)>,
    /// The figure in characters as last computed, for the settings it was
    /// computed for.
    text_shown: Option<(Key, TextFigure)>,
    state: State,
    /// The ranked genes, as the list shows them.
    ranked: Vec<(usize, f32)>,
    /// The row under the cursor.
    at: usize,
    /// The search being typed.
    search: Option<String>,
    note: Option<String>,
    quit: bool,
    /// Where the last draw put the gene list's rows (and the first gene
    /// shown) and the right pane, for the mouse.
    drawn: Drawn,
}

impl Screen {
    fn new(state: State) -> Self {
        let mut s = Self {
            genome: false,
            picker: None,
            figure: true,
            text: false,
            shown: None,
            text_shown: None,
            ranked: Vec::new(),
            at: 0,
            search: None,
            note: None,
            quit: false,
            drawn: Drawn::default(),
            state,
        };
        s.rank();
        s.follow();
        s
    }

    fn rank(&mut self) {
        let keep = self.state.gene;
        self.ranked = self
            .state
            .run
            .ranked(self.state.measure, self.state.summary);
        if let Some(g) = keep {
            self.at = self.ranked.iter().position(|&(i, _)| i == g).unwrap_or(0);
        }
    }

    /// Ask the terminal `session` draws on how it shows pictures.
    fn repick(&mut self, session: &legume_tui::Session) {
        use legume_tui::graphics::{Fallback, Graphics};
        self.picker = Some(legume_tui::graphics::pick(
            session,
            Graphics::Auto,
            Fallback::Probe,
        ));
        self.text = half_blocks(self.picker.as_ref());
    }

    /// The locus, and the gene ringed genome-wide, follow the cursor.
    fn follow(&mut self) {
        self.state.gene = self.ranked.get(self.at).map(|&(g, _)| g);
        self.state.genome_view.focus = self.state.gene;
    }

    /// Zoom the genome-wide view by `by` (< 1 in), about the gene in focus
    /// when it is in sight, else about the middle.
    fn zoom_genome(&mut self, by: f32) {
        let (from, span) = self.state.genome_view.window();
        let g = self.state.genome();
        let len = g.len().max(1) as f32;
        let at = self
            .state
            .gene
            .and_then(|f| g.points.iter().find(|p| p.0 == f))
            .map(|p| p.1 as f32 / len)
            .filter(|t| *t >= from && *t <= from + span)
            .unwrap_or(from + span / 2.0);
        let span = (span * by).clamp(1e-4, 1.0);
        self.state.genome_view.span = span;
        self.state.genome_view.from = (at - span / 2.0).clamp(0.0, 1.0 - span);
    }

    /// Move the genome-wide view by `by` of its width.
    fn pan_genome(&mut self, by: f32) {
        let (from, span) = self.state.genome_view.window();
        self.state.genome_view.from = (from + by * span).clamp(0.0, 1.0 - span);
    }

    /// The wheel moves through the genes over their list and zooms over
    /// the figure; a click on a gene goes to it, and on the gene the
    /// cursor is on, to its locus.
    fn mouse(&mut self, m: MouseEvent) {
        if self.search.is_some() {
            return;
        }
        let on_list = self.drawn.genes.row_at(m.column, m.row);
        let on_figure = self.drawn.figure.contains((m.column, m.row).into());
        match (wheel(&m), on_list) {
            // Over the list, as the arrow keys.
            (Some(d), Some(_)) => {
                self.key(KeyEvent::from(if d < 0 {
                    KeyCode::Up
                } else {
                    KeyCode::Down
                }));
                return;
            }
            (Some(d), None) if on_figure => {
                let by = if d < 0 { 0.8 } else { 1.25 };
                if self.genome {
                    self.zoom_genome(by);
                } else {
                    self.zoom(by);
                }
            }
            (None, Some(i)) if click(&m) => {
                if i == self.at {
                    self.genome = false;
                } else if i < self.ranked.len() {
                    self.at = i;
                }
            }
            _ => return,
        }
        self.note = None;
        self.follow();
    }

    fn key(&mut self, k: KeyEvent) {
        if let Some(text) = self.search.as_mut() {
            match k.code {
                KeyCode::Enter => {
                    let text = std::mem::take(text);
                    self.search = None;
                    match self.state.run.gene(text.trim()) {
                        Some(g) => {
                            self.at = self.ranked.iter().position(|&(i, _)| i == g).unwrap_or(0);
                            self.follow();
                            self.note = None;
                        }
                        None => self.note = Some(format!("{text}: no links")),
                    }
                }
                KeyCode::Esc => self.search = None,
                KeyCode::Backspace => {
                    text.pop();
                }
                KeyCode::Char(c) => text.push(c),
                _ => {}
            }
            return;
        }
        let n = self.ranked.len();
        match k.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit = true,
            KeyCode::Down | KeyCode::Char('j') => self.at = (self.at + 1).min(n.saturating_sub(1)),
            KeyCode::Up | KeyCode::Char('k') => self.at = self.at.saturating_sub(1),
            KeyCode::PageDown => self.at = (self.at + 20).min(n.saturating_sub(1)),
            KeyCode::PageUp => self.at = self.at.saturating_sub(20),
            KeyCode::Home => self.at = 0,
            KeyCode::Char('/') => self.search = Some(String::new()),
            KeyCode::Char('m') => {
                let all = Measure::ALL;
                let i = all
                    .iter()
                    .position(|&m| m == self.state.measure)
                    .unwrap_or(0);
                self.state.measure = all[(i + 1) % all.len()];
                self.rank();
            }
            KeyCode::Char('f') => self.figure = !self.figure,
            KeyCode::Char('g') => {
                self.text = !self.text;
                self.note = Some(
                    if self.text {
                        "the figure in characters"
                    } else {
                        "the figure as a picture"
                    }
                    .into(),
                );
            }
            KeyCode::Tab => self.genome = !self.genome,
            KeyCode::Enter if self.genome => self.genome = false,
            KeyCode::Char('[') if self.genome => self.zoom_genome(0.5),
            KeyCode::Char(']') if self.genome => self.zoom_genome(2.0),
            KeyCode::Left | KeyCode::Char('h') if self.genome => self.pan_genome(-0.5),
            KeyCode::Right | KeyCode::Char('l') if self.genome => self.pan_genome(0.5),
            KeyCode::Char('0') if self.genome => {
                self.state.genome_view.from = 0.0;
                self.state.genome_view.span = 1.0;
            }
            KeyCode::Char('[') => self.zoom(0.5),
            KeyCode::Char(']') => self.zoom(2.0),
            KeyCode::Char('0') => self.state.half_window = None,
            KeyCode::Char('c') => {
                let c = &mut self.state.style.cutoff;
                *c = match *c {
                    x if x < 0.01 => 0.05,
                    x if x < 0.1 => 0.2,
                    _ => 0.0,
                };
                self.note = Some(format!(
                    "links ≥ {} of the strongest",
                    self.state.style.cutoff
                ));
            }
            KeyCode::Char('e') => self.export(),
            KeyCode::Char('s') => {
                self.state.summary = match self.state.summary {
                    Summary::Best => Summary::Count(super::data::COUNT_CUTOFF),
                    Summary::Count(_) => Summary::Best,
                };
                self.rank();
            }
            _ => {}
        }
        self.follow();
    }
}

/// What a drawn figure depends on beside its area.
#[derive(Clone, Debug, PartialEq)]
struct Key {
    genome: bool,
    gene: Option<usize>,
    measure: Measure,
    summary: Summary,
    half: Option<i64>,
    style: super::figure::Style,
    view: super::figure::GenomeView,
}

impl Screen {
    fn key_now(&self) -> Key {
        Key {
            genome: self.genome,
            gene: self.state.gene,
            measure: self.state.measure,
            summary: self.state.summary,
            half: self.state.half_window,
            style: self.state.style.clone(),
            view: self.state.genome_view.clone(),
        }
    }

    /// Narrow (`by` < 1) or widen the window about the TSS.
    fn zoom(&mut self, by: f32) {
        let now = self
            .state
            .half_window
            .or_else(|| self.state.locus().map(|l| (l.hi - l.lo) / 2))
            .unwrap_or(100_000);
        self.state.half_window = Some(((now as f32 * by) as i64).clamp(1_000, 5_000_000));
    }

    /// The figure shown as a PDF beside the run: `{run}.{gene}.locus.pdf`
    /// or `{run}.genome.pdf`, numbered when there is one already.
    fn export(&mut self) {
        let stem = match (self.genome, self.state.gene) {
            (true, _) => format!("{}.genome", self.state.run.prefix),
            (false, Some(g)) => format!(
                "{}.{}.locus",
                self.state.run.prefix, self.state.run.genes[g]
            ),
            (false, None) => return,
        };
        use legume_tui::save::{free_stem, Numbering};
        let path =
            std::path::PathBuf::from(free_stem(&stem, &[".pdf"], Numbering::Suffix) + ".pdf");
        let page = legume_tui::export::PageSize {
            width_in: 7.0,
            dpi: 300,
        };
        let Some(fig) = self.state.figure(self.genome) else {
            return;
        };
        let state = &self.state;
        let saved = super::figure::save(
            Some(&path),
            None,
            page,
            state.aspect(&fig),
            |f, font, bold| {
                state.draw(&fig, f, font, bold);
            },
        );
        self.note = Some(match saved {
            Ok(()) => format!("wrote {}", crate::tui::shown(&path)),
            Err(e) => format!("the PDF: {e:#}"),
        });
    }
}

/// Run the screen on `state`.
pub fn run(state: State, opts: &legume_tui::Options) -> anyhow::Result<()> {
    let session = TOOL.session("view", opts)?;
    let mut screen = Screen::new(state);
    crate::tui::with_terminal(&session, |terminal| {
        // The terminal is fresh: its answer about pictures is this one's.
        session.take_repaint();
        screen.repick(&session);
        loop {
            let gene = screen
                .state
                .gene
                .map_or_else(String::new, |g| screen.state.run.genes[g].to_string());
            session.status(&format!("{} {gene}", screen.state.run.prefix));
            session.draw(terminal, |f| draw(f, &mut screen))?;
            // Everything that has come in, then one draw: a turn of the
            // wheel is many events, and the figure is drawn once for them.
            let mut wait = TICK;
            loop {
                match session.poll(terminal, wait)? {
                    Round::Stopped => return Ok(()),
                    // Another terminal may be attached now: ask it about
                    // pictures, and draw the figure for it.
                    Round::Repainted => {
                        screen.repick(&session);
                        screen.shown = None;
                    }
                    Round::Event(Event::Key(k)) if k.kind != KeyEventKind::Release => {
                        screen.key(k);
                    }
                    Round::Event(Event::Mouse(m)) => screen.mouse(m),
                    Round::Event(_) => {}
                    _ => break,
                }
                if screen.quit {
                    return Ok(());
                }
                wait = Duration::ZERO;
            }
        }
    })
}

fn draw(f: &mut Frame, s: &mut Screen) {
    let look = Look::terminal();
    let [head, body, foot] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(5),
        Constraint::Length(1),
    ])
    .areas(f.area());
    let run = &s.state.run;
    let how = match s.state.summary {
        Summary::Best => "best".to_string(),
        Summary::Count(c) => format!("links ≥ {c}"),
    };
    let title = format!(
        "chickpea view · {} — {} genes, {} pairs · {} by {how}",
        run.prefix,
        run.genes.len(),
        run.n_pairs(),
        s.state.measure.name(),
    );
    f.render_widget(Paragraph::new(Span::styled(title, look.bold)), head);
    let [left, right] =
        Layout::horizontal([Constraint::Length(32), Constraint::Min(20)]).areas(body);
    s.drawn = Drawn {
        genes: genes(f, s, left, &look),
        figure: right,
    };
    if (s.figure || s.genome) && s.text {
        text_figure(f, s, right, &look);
    } else if (s.figure || s.genome) && s.picker.is_some() {
        figure(f, s, right, &look);
    } else {
        locus(f, s, right, &look);
    }
    let foot_text = match (&s.search, &s.note) {
        (Some(text), _) => format!("gene: {text}▏  (enter goes, esc cancels)"),
        (None, Some(note)) => note.clone(),
        (None, None) => {
            let mut keys = vec![
                ("↑↓".to_string(), "gene".to_string()),
                (
                    "tab".into(),
                    if s.genome { "locus" } else { "genome" }.into(),
                ),
                ("/".into(), "find".into()),
                ("m".into(), "measure".into()),
                ("s".into(), "summary".into()),
                ("f".into(), "figure/table".into()),
                ("g".into(), "picture/text".into()),
                (
                    "[ ]".into(),
                    if s.genome { "zoom" } else { "window" }.into(),
                ),
                ("←→".into(), if s.genome { "pan" } else { "" }.into()),
                ("c".into(), "cutoff".into()),
                ("e".into(), "PDF".into()),
                ("q".into(), "quit".into()),
            ];
            keys.retain(|(_, w)| !w.is_empty());
            key_line(keys)
        }
    };
    f.render_widget(Paragraph::new(Span::styled(foot_text, look.hint)), foot);
}

/// The ranked genes, and where they were drawn.
fn genes(f: &mut Frame, s: &Screen, area: Rect, look: &Look) -> ListHit {
    let inner = area.inner(Margin::new(1, 1));
    let rows = inner.height as usize;
    let first = legume_tui::browse::first_visible(s.at, s.ranked.len(), rows);
    let lines: Vec<Line> = s
        .ranked
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
        .map(|(i, &(g, v))| {
            let style = if i == s.at { look.selected } else { look.page };
            let name: String = s.state.run.genes[g].chars().take(18).collect();
            Line::styled(format!("{name:<18} {v:>9.3}"), style)
        })
        .collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border)
        .title(" genes ");
    f.render_widget(Paragraph::new(lines).block(block), area);
    ListHit { rows: inner, first }
}

fn locus(f: &mut Frame, s: &Screen, area: Rect, look: &Look) {
    let Some(g) = s.state.gene else {
        return;
    };
    let run = &s.state.run;
    let m = s.state.measure;
    let mut links = run.links(g);
    links.sort_by(|a, b| b.get(m).total_cmp(&a.get(m)));
    let top = links
        .iter()
        .map(|l| l.get(m).abs())
        .fold(0f32, f32::max)
        .max(f32::MIN_POSITIVE);
    let bar_w = (area.width as usize).saturating_sub(70).clamp(4, 40);
    let mut lines = vec![Line::styled(
        format!(
            "{:<28} {:>9} {:>7} {:>7} {:>7} {:>7}",
            "peak", "distance", "score", "gate", "corr", "abc"
        ),
        look.bold,
    )];
    lines.extend(links.iter().map(|l| {
        let filled = ((l.get(m).abs() / top) * bar_w as f32).round() as usize;
        Line::from(vec![
            Span::raw(format!(
                "{:<28} {:>9} {:>7.3} {:>7.3} {:>7.3} {:>7.3} ",
                l.peak.chars().take(28).collect::<String>(),
                l.distance,
                l.score,
                l.gate,
                l.corr,
                l.abc
            )),
            Span::styled("█".repeat(filled.min(bar_w)), look.bold),
        ])
    }));
    let place = match &run.tss[g] {
        Some(t) => format!(
            " {}:{}{}",
            t.chr,
            t.tss,
            if t.inferred { " (inferred)" } else { "" }
        ),
        None => String::new(),
    };
    let title = format!(
        " {} —{place} · {} candidates, by {} ",
        run.genes[g],
        links.len(),
        m.name()
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border)
        .title(title);
    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// The locus figure in `area`, drawn again only when it or the area
/// changed.
fn figure(f: &mut Frame, s: &mut Screen, area: Rect, look: &Look) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border);
    let inner = block.inner(area);
    f.render_widget(block, area);
    if inner.width < 20 || inner.height < 6 {
        return;
    }
    let key = s.key_now();
    let stale = s
        .shown
        .as_ref()
        .is_none_or(|(a, k, _)| *a != inner || *k != key);
    if stale {
        let Some(picker) = &s.picker else { return };
        let Some(fig) = s.state.figure(s.genome) else {
            s.shown = None;
            return;
        };
        let state = &s.state;
        let drawn = super::figure::on_screen(inner, picker, |frame, font, bold| {
            state.draw(&fig, frame, font, bold);
        });
        match drawn {
            Ok(p) => s.shown = Some((inner, key, p)),
            Err(e) => {
                s.note = Some(format!("the figure: {e}"));
                s.figure = false;
                return;
            }
        }
    }
    if let Some((_, _, p)) = &s.shown {
        f.render_widget(ratatui_image::Image::new(p), inner);
    }
}

/// Whether `picker` shows pictures only as half blocks, too coarse for
/// thin arcs and small text.
fn half_blocks(picker: Option<&ratatui_image::picker::Picker>) -> bool {
    picker.is_none_or(|p| p.protocol_type() == ratatui_image::picker::ProtocolType::Halfblocks)
}

/// What the figure in characters shows.
enum TextFigure {
    Genome(super::data::Genome),
    Locus(Option<super::data::Locus>),
}

/// The locus figure in characters, computed again only when its settings
/// change.
fn text_figure(f: &mut Frame, s: &mut Screen, area: Rect, look: &Look) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(look.border);
    let inner = block.inner(area);
    f.render_widget(block, area);
    let key = s.key_now();
    if s.text_shown.as_ref().is_none_or(|(k, _)| *k != key) {
        let shown = if s.genome {
            TextFigure::Genome(s.state.genome())
        } else {
            TextFigure::Locus(s.state.locus())
        };
        s.text_shown = Some((key, shown));
    }
    match &s.text_shown {
        Some((_, TextFigure::Genome(g))) => super::text::render_genome(
            g,
            &s.state.run.genes,
            &s.state.genome_view,
            inner,
            f.buffer_mut(),
        ),
        Some((_, TextFigure::Locus(Some(locus)))) => {
            super::text::render(locus, &s.state.style, inner, f.buffer_mut());
        }
        _ => {}
    }
}
