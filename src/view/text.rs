//! The figures in characters, for terminals that show no pictures (or only
//! half blocks, where thin lines and small text blur away) and for agents
//! that read text. A locus is a grid: a line for each linked peak, in
//! genome order, and a column for each row of the figure (all the cells,
//! each group), the link's value there in eighth blocks. The genome-wide
//! view is a scatter in braille, two dots across and four down a cell.

use super::data::{Genome, Locus};
use super::figure::{bp_text, drawn, row_strongest, scales, short, summary_text};
use super::figure::{GenomeView, Style as FigureStyle};
use legume_tui::chrome;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Positive links, at their strongest.
const ARC: (u8, u8, u8) = (178, 120, 230);
/// Negative links.
const ARC_NEG: (u8, u8, u8) = (230, 140, 100);
/// The weakest link shown.
const FAINT: (u8, u8, u8) = (90, 90, 100);
const HINT: Color = Color::Rgb(120, 120, 128);
/// Eighth blocks, empty to full.
const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The braille bit of dot `(x, y)` of a cell, `x` in 0..2, `y` in 0..4.
const DOT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Lines of a grid besides its peaks: the title and the header, then the
/// key (two).
const GRID_FRAME: u16 = 4;
/// The most characters of a peak's name, and of a group's, shown.
const PEAK_W: usize = 24;
const GROUP_W: usize = 10;
/// Spaces between the grid's columns.
const GAP: usize = 2;
/// Peaks a grid written out lists at most, the strongest.
const PEAKS_LISTED: usize = 40;

/// A grid of braille dots.
struct Dots {
    w: usize,
    h: usize,
    bits: Vec<u8>,
}

impl Dots {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![0; w * h],
        }
    }

    /// Set dot `(x, y)` (in dots: `2w × 4h`).
    fn set(&mut self, x: f32, y: f32) {
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let (cx, cy) = (x / 2, y / 4);
        if cx >= self.w || cy >= self.h {
            return;
        }
        self.bits[cy * self.w + cx] |= DOT[x % 2][y % 4];
    }
}

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::Rgb(c.0, c.1, c.2)
}

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t.clamp(0.0, 1.0)) as u8;
    Color::Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// `n` and `what`, plural past one: `1 peak`, `3 peaks`.
fn count(n: usize, what: &str) -> String {
    format!("{n} {what}{}", if n == 1 { "" } else { "s" })
}

/// The links of `locus` the grid lists: those shown in some of `rows`,
/// by each row's own cutoff, in genome order.
fn listed(locus: &Locus, style: &FigureStyle, rows: &[usize]) -> Vec<usize> {
    let tops: Vec<f32> = rows
        .iter()
        .map(|&r| row_strongest(&locus.rows[r].value))
        .collect();
    let mut ks: Vec<usize> = (0..locus.links.len())
        .filter(|&k| {
            rows.iter()
                .zip(&tops)
                .any(|(&r, &top)| drawn(locus.rows[r].value[k], top, style.cutoff))
        })
        .collect();
    ks.sort_by_key(|&k| start_of(locus, k));
    ks
}

/// Lines the grid of `locus` takes with every peak it lists, up to
/// [`PEAKS_LISTED`] of them.
#[must_use]
pub fn grid_height(locus: &Locus, style: &FigureStyle) -> u16 {
    height_for(listed(locus, style, &style.rows_of(locus)).len())
}

/// Lines a grid listing `n` peaks takes, up to [`PEAKS_LISTED`] of them.
fn height_for(n: usize) -> u16 {
    GRID_FRAME + n.clamp(1, PEAKS_LISTED) as u16
}

/// Draw `locus` into `area` of `buf`, as a grid of its peaks by its rows.
pub fn render(locus: &Locus, style: &FigureStyle, area: Rect, buf: &mut Buffer) {
    let ks = listed(locus, style, &style.rows_of(locus));
    render_listed(locus, style, ks, area, buf);
}

/// [`render`], the links it lists (by [`listed`]) given.
fn render_listed(
    locus: &Locus,
    style: &FigureStyle,
    mut ks: Vec<usize>,
    area: Rect,
    buf: &mut Buffer,
) {
    let rows = style.rows_of(locus);
    let room = area.height.saturating_sub(GRID_FRAME) as usize;
    if area.width < 40 || room == 0 || rows.is_empty() {
        buf.set_string(
            area.x,
            area.y,
            "too small for the locus",
            Style::default().fg(HINT),
        );
        return;
    }
    let (v_max, _) = scales(locus, &rows);
    // Too many to list: the strongest, put back in genome order.
    let hidden = ks.len().saturating_sub(room);
    if hidden > 0 {
        let strength = |k: usize| {
            rows.iter()
                .map(|&r| locus.rows[r].value[k].abs())
                .fold(0f32, f32::max)
        };
        let mut by: Vec<(f32, usize)> = ks.iter().map(|&k| (strength(k), k)).collect();
        by.sort_by(|a, b| b.0.total_cmp(&a.0));
        ks = by.into_iter().take(room).map(|(_, k)| k).collect();
        ks.sort_by_key(|&k| start_of(locus, k));
    }

    let bold = Style::default().add_modifier(Modifier::BOLD);
    let hint = Style::default().fg(HINT);
    let title = format!(
        "{} {}:{}-{} · blocks: {}{}",
        locus.gene,
        locus.tss.chr,
        locus.lo,
        locus.hi,
        locus.measure.shown(),
        if locus.tss.inferred {
            " · TSS inferred"
        } else {
            ""
        }
    );
    let width = area.width as usize;
    buf.set_stringn(area.x, area.y, &title, width, bold);

    // The peak, its distance from the TSS and its value: as many columns
    // of groups as fit after them.
    let m = locus.measure;
    let dist: Vec<String> = ks
        .iter()
        .map(|&k| {
            locus.links[k].place.as_ref().map_or_else(String::new, |p| {
                let d = p.mid() - locus.tss.tss;
                format!("{}{}", if d > 0 { "+" } else { "" }, bp_text(d as f32))
            })
        })
        .collect();
    let value: Vec<String> = ks
        .iter()
        .map(|&k| format!("{:.3}", locus.links[k].get(m)))
        .collect();
    let wide = |v: &[String], head: &str| {
        v.iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(0)
            .max(head.len())
    };
    let (dist_w, value_w) = (wide(&dist, "dist"), wide(&value, m.name()));
    // The peak's name gives way to its numbers where it is narrow.
    let peak_w = ks
        .iter()
        .map(|&k| locus.links[k].peak.chars().count())
        .max()
        .unwrap_or(0)
        .clamp(4, PEAK_W)
        .min(width.saturating_sub(GAP + dist_w + GAP + value_w));
    if peak_w < 4 {
        buf.set_string(area.x, area.y + 1, "too narrow for the grid", hint);
        return;
    }
    let dist_x = peak_w + GAP;
    let value_x = dist_x + dist_w + GAP;
    let columns: Vec<Column> = rows
        .iter()
        .scan(value_x + value_w, |x, &r| {
            let label = chrome::short(&locus.rows[r].label, GROUP_W);
            let w = label.chars().count().max(3);
            let col = Column {
                x: *x + GAP,
                w,
                top: row_strongest(&locus.rows[r].value),
                r,
                label,
            };
            *x += GAP + w;
            Some(col)
        })
        .take_while(|c| c.x + c.w <= width)
        .collect();
    let dropped = rows.len() - columns.len();

    let y = area.y + 1;
    let right = |s: &str, w: usize| format!("{s:>w$}");
    let at = |x: usize| area.x + x as u16;
    buf.set_string(at(0), y, "peak", bold);
    buf.set_string(at(dist_x), y, right("dist", dist_w), bold);
    buf.set_string(at(value_x), y, right(m.name(), value_w), bold);
    for c in &columns {
        buf.set_string(at(c.x), y, &c.label, bold);
    }
    for (i, &k) in ks.iter().enumerate() {
        let y = y + 1 + i as u16;
        let peak = chrome::middle(&locus.links[k].peak, peak_w);
        buf.set_string(at(0), y, peak, Style::default());
        buf.set_string(at(dist_x), y, right(&dist[i], dist_w), hint);
        buf.set_string(at(value_x), y, right(&value[i], value_w), Style::default());
        for c in &columns {
            let v = locus.rows[c.r].value[k];
            if drawn(v, c.top, style.cutoff) && v_max > 0.0 {
                let s = v.abs() / v_max;
                let level = ((s * 8.0).ceil() as usize).clamp(1, 8);
                let mut st =
                    Style::default().fg(mix(FAINT, if v < 0.0 { ARC_NEG } else { ARC }, s.sqrt()));
                if s > 0.5 {
                    st = st.add_modifier(Modifier::BOLD);
                }
                let block: String = std::iter::repeat_n(BLOCKS[level], c.w).collect();
                buf.set_string(at(c.x), y, block, st);
            } else if v.is_finite() && v != 0.0 {
                buf.set_string(at(c.x), y, "·", hint);
            }
        }
    }

    // The key, flowing onto a second line where it is narrow (a part that
    // does not fit there is left out); what is left out first, so it is
    // never the part cut off.
    let ky = y + 1 + ks.len() as u16;
    let mut texts: Vec<String> = Vec::new();
    let mut styles: Vec<Style> = Vec::new();
    let mut part = |text: String, st: Style| {
        texts.push(text);
        styles.push(st);
    };
    if hidden > 0 {
        part(
            format!("+{} not listed (the weakest)", count(hidden, "peak")),
            hint,
        );
    }
    if dropped > 0 {
        part(format!("+{} past the edge", count(dropped, "group")), hint);
    }
    part("█ positive".into(), Style::default().fg(rgb(ARC)));
    part("█ negative".into(), Style::default().fg(rgb(ARC_NEG)));
    part(format!("taller: stronger (top {})", short(v_max)), hint);
    if style.cutoff > 0.0 {
        part(
            format!("·: under {} of the group's top", short(style.cutoff)),
            hint,
        );
    }
    let chars = |t: &str| t.chars().count() as f32;
    let placed = super::figure::flow(&texts, chars, 0.0, 0.0, width as f32);
    for (((line, x), text), st) in placed.iter().zip(&texts).zip(&styles) {
        if *line > 1 {
            break;
        }
        let (x, y) = (*x as usize, ky + *line as u16);
        if x > 0 {
            buf.set_string(at(x - 3), y, " · ", hint);
        }
        buf.set_stringn(at(x), y, text, width - x, *st);
    }
}

/// A column of the grid: a row of the locus, where it starts and how wide
/// it is, its label, and its strongest link (its cutoff is a share of it).
struct Column {
    r: usize,
    x: usize,
    w: usize,
    label: String,
    top: f32,
}

/// Link `k`'s peak's start, for genome order (unplaced last).
fn start_of(locus: &Locus, k: usize) -> i64 {
    locus.links[k].place.as_ref().map_or(i64::MAX, |p| p.start)
}

/// Points of the genome-wide view, on alternate chromosomes.
const POINT: [Color; 2] = [Color::Rgb(90, 140, 210), Color::Rgb(150, 160, 175)];
/// The focus gene.
const FOCUS: Color = Color::Rgb(178, 120, 230);
/// Genes listed under the genome-wide view at most.
const LISTED: usize = 8;

/// Draw `genome` into `area` of `buf`, as text; `names` the run's genes.
pub fn render_genome(
    genome: &Genome,
    names: &[Box<str>],
    view: &GenomeView,
    area: Rect,
    buf: &mut Buffer,
) {
    let axis_w: u16 = 8;
    if area.width < axis_w + 20 || area.height < 7 || genome.chroms.is_empty() {
        buf.set_string(
            area.x,
            area.y,
            "too small for the genome",
            Style::default().fg(HINT),
        );
        return;
    }
    let what = summary_text(genome.measure, genome.summary);
    buf.set_stringn(
        area.x,
        area.y,
        format!("Genome-wide · {what}"),
        area.width as usize,
        Style::default().add_modifier(Modifier::BOLD),
    );
    let (x0, y0) = (area.x + axis_w, area.y + 1);
    let w = (area.width - axis_w) as usize;
    let h = (area.height - 4) as usize;
    let (from, span) = view.window();
    let len = genome.len().max(1) as f32;
    let seen = |t: f32| t >= from && t <= from + span;
    // A place's dot column, and a value's dot row (0 at the top).
    let dx = |t: f32| ((t - from) / span * (2 * w - 1) as f32).round();
    let in_sight: Vec<&(usize, u64, f32)> = genome
        .points
        .iter()
        .filter(|p| seen(p.1 as f32 / len))
        .collect();
    let hi = in_sight
        .iter()
        .map(|p| p.2)
        .fold(0f32, f32::max)
        .max(f32::MIN_POSITIVE)
        * 1.05;
    let lo = in_sight.iter().map(|p| p.2).fold(0f32, f32::min).min(0.0);
    let dy = |v: f32| ((hi - v) / (hi - lo) * (4 * h - 1) as f32).round();
    let chrom_of = |pos: u64| genome.chrom_at(pos);

    // Dots, each cell coloured by the chromosome of the best gene in it.
    let mut dots = Dots::new(w, h);
    let mut cell_chrom = vec![usize::MAX; w * h];
    let mut cell_best = vec![f32::NEG_INFINITY; w * h];
    for &&(_, pos, v) in &in_sight {
        let (x, y) = (dx(pos as f32 / len), dy(v));
        dots.set(x, y);
        let i = (y as usize / 4).min(h - 1) * w + (x as usize / 2).min(w - 1);
        if v > cell_best[i] {
            cell_best[i] = v;
            cell_chrom[i] = chrom_of(pos);
        }
    }
    let focus = view
        .focus
        .and_then(|f| in_sight.iter().find(|p| p.0 == f))
        .map(|&&(g, pos, v)| {
            (
                g,
                (dx(pos as f32 / len) as usize / 2).min(w - 1),
                (dy(v) as usize / 4).min(h - 1),
            )
        });
    for cy in 0..h {
        for cx in 0..w {
            let i = cy * w + cx;
            if dots.bits[i] == 0 {
                continue;
            }
            let ch = char::from_u32(0x2800 + u32::from(dots.bits[i])).unwrap_or(' ');
            let mut st = Style::default().fg(POINT[cell_chrom[i] % 2]);
            if focus.is_some_and(|(_, fx, fy)| fx == cx && fy == cy) {
                st = Style::default().fg(FOCUS).add_modifier(Modifier::BOLD);
            }
            buf[(x0 + cx as u16, y0 + cy as u16)]
                .set_char(ch)
                .set_style(st);
        }
    }
    // The axis: its top, its middle and its floor.
    for cy in 0..h {
        buf[(x0 - 1, y0 + cy as u16)]
            .set_char('│')
            .set_style(Style::default().fg(HINT));
    }
    let counts = matches!(genome.summary, super::data::Summary::Count(_));
    let num = |v: f32| {
        if counts || hi - lo >= 10.0 {
            format!("{v:.0}")
        } else {
            format!("{v:.3}")
        }
    };
    for (cy, v) in [(0, hi), (h / 2, (hi + lo) / 2.0), (h - 1, lo)] {
        let s = num(v);
        let s: String = s.chars().take((axis_w - 2) as usize).collect();
        buf.set_string(
            x0 - 2 - s.chars().count() as u16,
            y0 + cy as u16,
            &s,
            Style::default().fg(HINT),
        );
    }
    // The focus gene's name beside it.
    if let Some((g, fx, fy)) = focus {
        let name = &names[g];
        let n = name.chars().count();
        let at = if fx + 2 + n <= w {
            fx + 2
        } else {
            fx.saturating_sub(n + 1)
        };
        buf.set_stringn(
            x0 + at as u16,
            y0 + fy as u16,
            name.as_ref(),
            w - at,
            Style::default().fg(FOCUS).add_modifier(Modifier::BOLD),
        );
    }
    // The chromosomes under it.
    let cy = y0 + h as u16;
    for x in 0..w {
        buf[(x0 + x as u16, cy)]
            .set_char('─')
            .set_style(Style::default().fg(HINT));
    }
    let mut used_to = 0usize;
    for (k, (name, off, l)) in genome.chroms.iter().enumerate() {
        let (a, b) = (*off as f32 / len, (off + l) as f32 / len);
        // Only the chromosomes with some of their length in sight.
        if b <= from + 1e-6 || a >= from + span - 1e-6 {
            continue;
        }
        let ca = (dx(a.max(from)) / 2.0) as usize;
        let cb = ((dx(b.min(from + span)) / 2.0) as usize).min(w - 1);
        buf[(x0 + ca.min(w - 1) as u16, cy)]
            .set_char('┴')
            .set_style(Style::default().fg(HINT));
        let n = name.chars().count();
        let mid = (ca + cb) / 2;
        let start = mid.saturating_sub(n / 2);
        if start >= used_to && start + n <= w && cb.saturating_sub(ca) + 1 >= n {
            buf.set_string(
                x0 + start as u16,
                cy + 1,
                name.as_ref(),
                Style::default().fg(POINT[k % 2]),
            );
            used_to = start + n + 1;
        }
    }
    // The best genes in sight with something to show, listed whole while
    // they fit.
    let mut best: Vec<&&(usize, u64, f32)> = in_sight.iter().filter(|p| p.2 > 0.0).collect();
    best.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut list = String::from("top:");
    for p in best.iter().take(LISTED) {
        let entry = format!("{} {}", names[p.0], num(p.2));
        let sep = if list.len() > 4 { " · " } else { " " };
        if list.chars().count() + sep.chars().count() + entry.chars().count() > area.width as usize
        {
            break;
        }
        list.push_str(sep);
        list.push_str(&entry);
    }
    if best.is_empty() {
        list.push_str(" no gene in sight above 0");
    }
    buf.set_stringn(
        area.x,
        cy + 2,
        list,
        area.width as usize,
        Style::default().fg(HINT),
    );
}

/// `genome` drawn as text `width` × `height`, for an agent to read.
#[must_use]
pub fn genome_to_string(
    genome: &Genome,
    names: &[Box<str>],
    view: &GenomeView,
    width: u16,
    height: u16,
) -> String {
    let area = Rect::new(0, 0, width, height);
    let mut buf = Buffer::empty(area);
    render_genome(genome, names, view, area, &mut buf);
    lines_of(&buf)
}

/// A buffer's text, line by line, without trailing blanks.
fn lines_of(buf: &Buffer) -> String {
    let area = buf.area;
    (0..area.height)
        .map(|y| {
            let line: String = (0..area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect();
            line.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// `locus` drawn as text `width` columns wide, for an agent to read.
#[must_use]
pub fn to_string(locus: &Locus, style: &FigureStyle, width: u16) -> String {
    let ks = listed(locus, style, &style.rows_of(locus));
    let area = Rect::new(0, 0, width, height_for(ks.len()));
    let mut buf = Buffer::empty(area);
    render_listed(locus, style, ks, area, &mut buf);
    lines_of(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dot_lands_in_its_cell_and_bit() {
        let mut d = Dots::new(2, 1);
        d.set(3.0, 2.0);
        assert_eq!(d.bits, [0, 0x20]);
        d.set(9.0, 0.0);
        assert_eq!(d.bits, [0, 0x20], "a dot off the grid is dropped");
    }

    /// GENE1 at 50 kb with three links, listed out of genome order, in all
    /// the cells and two groups: p_far under CT1's cutoff, p_near negative
    /// there.
    fn locus() -> Locus {
        Locus::for_tests(
            &[
                ("p_mid", 49_750, 0.3),
                ("p_far", 89_750, 0.1),
                ("p_near", 9_750, 0.5),
            ],
            &[
                ("All cells", &[0.3, 0.1, 0.5]),
                ("CT1", &[0.8, 0.01, -0.4]),
                ("CT2", &[0.0, 0.2, 0.1]),
            ],
        )
    }

    /// The grid's header line.
    fn header(text: &str) -> &str {
        text.lines().find(|l| l.starts_with("peak")).unwrap()
    }

    /// The grid's line for `peak`.
    fn line_of<'a>(text: &'a str, peak: &str) -> &'a str {
        text.lines()
            .find(|l| l.starts_with(peak))
            .unwrap_or_else(|| panic!("no line for {peak} in\n{text}"))
    }

    /// The cell of `line` under the header's `column`.
    fn cell(text: &str, line: &str, column: &str) -> char {
        let header = header(text);
        let at = header[..header.find(column).unwrap()].chars().count();
        // A blank cell at the end of a line is trimmed away with it.
        line.chars().nth(at).unwrap_or(' ')
    }

    #[test]
    fn a_locus_reads_as_a_grid_of_peaks_by_group() {
        let text = to_string(&locus(), &FigureStyle::default(), 100);
        let peaks: Vec<&str> = text
            .lines()
            .filter_map(|l| l.split_whitespace().next())
            .filter(|w| w.starts_with("p_"))
            .collect();
        assert_eq!(peaks, ["p_near", "p_mid", "p_far"], "genome order\n{text}");
        let header = header(&text);
        for col in ["dist", "score", "All cells", "CT1", "CT2"] {
            assert!(header.contains(col), "{col} in {header}");
        }
        let near = line_of(&text, "p_near");
        assert!(near.contains("-40 kb") && near.contains("0.500"), "{near}");
        // The strongest share of the shown rows is full; others in eighths.
        assert_eq!(cell(&text, line_of(&text, "p_mid"), "CT1"), '█');
        assert_eq!(cell(&text, near, "All cells"), '▅');
        assert_eq!(cell(&text, near, "CT1"), '▄', "a negative by its size");
        // Under the group's own cutoff, and nothing at all.
        assert_eq!(cell(&text, line_of(&text, "p_far"), "CT1"), '·');
        assert_eq!(cell(&text, line_of(&text, "p_mid"), "CT2"), ' ');
    }

    #[test]
    fn a_short_grid_keeps_the_strongest_peaks() {
        let locus = locus();
        let style = FigureStyle::default();
        let area = Rect::new(0, 0, 100, grid_height(&locus, &style) - 1);
        let mut buf = Buffer::empty(area);
        render(&locus, &style, area, &mut buf);
        let text = lines_of(&buf);
        assert!(!text.contains("p_far"), "the weakest goes\n{text}");
        assert!(text.contains("p_near") && text.contains("p_mid"));
        assert!(text.contains("+1 peak"), "{text}");
    }

    #[test]
    fn a_long_peak_name_gives_way_to_the_values() {
        let mut l = locus();
        l.links[2].peak = "chr1:9750-10250_a_long_peak_name".into();
        l.lo = -2_000_000;
        l.tss.tss = 1_500_000;
        let text = to_string(&l, &FigureStyle::default(), 40);
        let near = text.lines().find(|t| t.starts_with("chr1:")).unwrap();
        assert!(near.contains('…'), "the name cut: {near}");
        assert!(
            near.contains("-1.49 Mb") && near.contains("0.500"),
            "{near}"
        );
    }

    #[test]
    fn groups_that_do_not_fit_are_counted() {
        let text = to_string(&locus(), &FigureStyle::default(), 40);
        let header = header(&text);
        assert!(!header.contains("CT2"), "{header}");
        assert!(text.contains("+1 group"), "{text}");
    }
}
