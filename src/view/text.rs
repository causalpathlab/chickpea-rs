//! The locus figure in characters, for terminals that show no pictures (or
//! only half blocks, where thin arcs and small text blur away) and for
//! agents that read text: arcs in braille, two dots across and four down a
//! cell, stronger ones brighter; each row's accessibility in eighth blocks;
//! the TSS and the marks as dotted columns; the window's genes and a scale
//! as text.

use super::data::{Genome, Locus};
use super::figure::{bp_text, summary_text, GenomeView, Style as FigureStyle};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

/// Arcs of positive links, at their strongest.
const ARC: (u8, u8, u8) = (178, 120, 230);
/// Arcs of negative links.
const ARC_NEG: (u8, u8, u8) = (230, 140, 100);
/// The weakest arc drawn.
const FAINT: (u8, u8, u8) = (90, 90, 100);
const TRACK: Color = Color::Rgb(90, 140, 210);
const HINT: Color = Color::Rgb(120, 120, 128);
/// Eighth blocks, empty to full.
const BLOCKS: [char; 9] = [' ', '▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// The braille bit of dot `(x, y)` of a cell, `x` in 0..2, `y` in 0..4.
const DOT: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

/// Rows a locus row takes at most and at least: arcs, then its track.
const ROW_MAX: u16 = 6;
const ROW_MIN: u16 = 3;
/// Lines at the foot: the genes, their names, the scale, the key (two).
const FOOT: u16 = 5;

/// A grid of braille dots, each cell keeping the strongest arc through it.
struct Dots {
    w: usize,
    h: usize,
    bits: Vec<u8>,
    /// The strongest arc through each cell, signed.
    strength: Vec<f32>,
}

impl Dots {
    fn new(w: usize, h: usize) -> Self {
        Self {
            w,
            h,
            bits: vec![0; w * h],
            strength: vec![0.0; w * h],
        }
    }

    /// Set dot `(x, y)` (in dots: `2w × 4h`) for an arc of `s`.
    fn set(&mut self, x: f32, y: f32, s: f32) {
        if x < 0.0 || y < 0.0 {
            return;
        }
        let (x, y) = (x as usize, y as usize);
        let (cx, cy) = (x / 2, y / 4);
        if cx >= self.w || cy >= self.h {
            return;
        }
        let i = cy * self.w + cx;
        self.bits[i] |= DOT[x % 2][y % 4];
        if s.abs() > self.strength[i].abs() {
            self.strength[i] = s;
        }
    }

    /// A half-ellipse from dot column `a` to `b` on dot row `base`,
    /// `height` dots tall.
    fn arc(&mut self, a: f32, b: f32, base: f32, height: f32, s: f32) {
        let (mid, rx) = ((a + b) / 2.0, (b - a).abs() / 2.0);
        let steps = ((rx + height) * 3.0).ceil().max(8.0) as usize;
        for i in 0..=steps {
            let t = std::f32::consts::PI * i as f32 / steps as f32;
            self.set(mid - rx * t.cos(), base - height * t.sin(), s);
        }
    }
}

fn mix(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> Color {
    let l = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t.clamp(0.0, 1.0)) as u8;
    Color::Rgb(l(a.0, b.0), l(a.1, b.1), l(a.2, b.2))
}

/// Lines a locus of `rows` rows takes at its best.
#[must_use]
pub fn height(rows: usize) -> u16 {
    1 + rows as u16 * ROW_MAX + FOOT
}

/// Draw `locus` into `area` of `buf`, as text.
pub fn render(locus: &Locus, style: &FigureStyle, area: Rect, buf: &mut Buffer) {
    let rows = style.rows_of(locus);
    let label_w = rows
        .iter()
        .map(|&r| locus.rows[r].label.chars().count())
        .max()
        .unwrap_or(0)
        .max(9) as u16
        + 1;
    if area.width < label_w + 20 || area.height < 1 + ROW_MIN + FOOT || rows.is_empty() {
        buf.set_string(
            area.x,
            area.y,
            "too small for the locus",
            Style::default().fg(HINT),
        );
        return;
    }
    let what = locus.measure.arcs();
    let title = format!(
        "{} {}:{}-{} · {what}",
        locus.gene, locus.tss.chr, locus.lo, locus.hi
    );
    buf.set_stringn(
        area.x,
        area.y,
        &title,
        area.width as usize,
        Style::default().add_modifier(Modifier::BOLD),
    );

    // Rows as tall as fit, as many as fit.
    let room = area.height - 1 - FOOT;
    let rh = (room / rows.len() as u16).clamp(ROW_MIN, ROW_MAX);
    let shown = ((room / rh) as usize).min(rows.len());
    let x0 = area.x + label_w;
    let w = (area.width - label_w) as usize;
    let span = (locus.hi - locus.lo).max(1) as f32;
    // A position's column, in dots (two to a cell) and in cells.
    let dx = |bp: i64| (bp - locus.lo) as f32 / span * (2 * w - 1) as f32;
    let col = |bp: i64| ((dx(bp) / 2.0) as usize).min(w - 1);
    let tss_col = col(locus.tss.tss);
    let mark_cols: Vec<usize> = style
        .marks
        .iter()
        .filter(|m| (locus.lo..=locus.hi).contains(m))
        .map(|&m| col(m))
        .collect();

    let (v_max, rate_max) = super::figure::scales(locus, &rows);

    for (i, &r) in rows.iter().take(shown).enumerate() {
        let row = &locus.rows[r];
        let y = area.y + 1 + i as u16 * rh;
        let arc_rows = (rh - 1) as usize;
        let ty = y + rh - 1;

        buf.set_stringn(
            area.x,
            y,
            &row.label,
            (label_w - 1) as usize,
            Style::default().add_modifier(Modifier::BOLD),
        );
        if let Some(n) = row.cells.filter(|_| arc_rows > 1) {
            buf.set_stringn(
                area.x,
                y + 1,
                format!("{n} cells"),
                (label_w - 1) as usize,
                Style::default().fg(HINT),
            );
        }

        // The arcs, in braille; the cutoff is the row's own.
        let row_max = super::figure::row_strongest(&row.value);
        let mut dots = Dots::new(w, arc_rows);
        let base = (arc_rows * 4) as f32 - 1.0;
        if v_max > 0.0 {
            for (k, l) in locus.links.iter().enumerate() {
                let v = row.value[k];
                if !super::figure::drawn(v, row_max, style.cutoff) {
                    continue;
                }
                let s = v.abs() / v_max;
                let Some(p) = &l.place else { continue };
                let (a, b) = (dx(locus.tss.tss), dx(p.mid()));
                let reach = ((a - b).abs() / (2 * w) as f32 * 2.2).clamp(0.25, 1.0);
                let signed = if v < 0.0 { -s } else { s };
                if (a - b).abs() < 2.0 {
                    // A peak at the TSS: a stem as tall as the link is strong.
                    let stem = (base * (0.2 + 0.7 * s.sqrt())).max(1.0);
                    for d in 0..stem as usize {
                        dots.set(b, base - d as f32, signed);
                    }
                } else {
                    dots.arc(a.min(b), a.max(b), base, base * reach, signed);
                }
            }
        }
        for cy in 0..arc_rows {
            for cx in 0..w {
                let i = cy * w + cx;
                let (x, yy) = (x0 + cx as u16, y + cy as u16);
                if dots.bits[i] != 0 {
                    let s = dots.strength[i];
                    let c = mix(FAINT, if s < 0.0 { ARC_NEG } else { ARC }, s.abs().sqrt());
                    let mut st = Style::default().fg(c);
                    if s.abs() > 0.5 {
                        st = st.add_modifier(Modifier::BOLD);
                    }
                    let ch = char::from_u32(0x2800 + u32::from(dots.bits[i])).unwrap_or(' ');
                    buf[(x, yy)].set_char(ch).set_style(st);
                } else if cx == tss_col || mark_cols.contains(&cx) {
                    buf[(x, yy)]
                        .set_char('┊')
                        .set_style(Style::default().fg(HINT));
                }
            }
        }

        // The track: the most open peak under each column.
        let mut cols = vec![0f32; w];
        for ((_, p), &v) in locus.peaks.iter().zip(&row.rate) {
            for c in &mut cols[col(p.start)..=col(p.end)] {
                *c = c.max(v);
            }
        }
        for (cx, &v) in cols.iter().enumerate() {
            let level = if rate_max > 0.0 && v > 0.0 {
                ((v / rate_max * 8.0).ceil() as usize).clamp(1, 8)
            } else {
                0
            };
            let (ch, st) = match level {
                0 => ('▁', Style::default().fg(Color::Rgb(60, 60, 66))),
                l => (BLOCKS[l], Style::default().fg(TRACK)),
            };
            buf[(x0 + cx as u16, ty)].set_char(ch).set_style(st);
        }
    }
    if shown < rows.len() {
        let y = area.y + 1 + shown as u16 * rh - 1;
        let more = format!("+{} rows", rows.len() - shown);
        buf.set_stringn(
            area.x,
            y,
            more,
            (label_w - 1) as usize,
            Style::default().fg(HINT),
        );
    }

    // The genes of the window, then the scale.
    let gy = area.y + 1 + shown as u16 * rh;
    for x in 0..w {
        buf[(x0 + x as u16, gy)]
            .set_char('─')
            .set_style(Style::default().fg(HINT));
    }
    for (name, t) in &locus.genes {
        let own = **name == *locus.gene;
        buf[(x0 + col(*t) as u16, gy)]
            .set_char(if own { '┃' } else { '│' })
            .set_style(Style::default().fg(if own { Color::Reset } else { HINT }));
    }
    let order = super::figure::label_order(locus);
    let mut taken: Vec<(usize, usize)> = Vec::new();
    for (name, t) in order {
        let own = **name == *locus.gene;
        let n = name.chars().count();
        let start = col(*t).saturating_sub(n / 2).min(w.saturating_sub(n));
        if taken
            .iter()
            .any(|&(a, b)| start < b + 1 && start + n + 1 > a)
        {
            continue;
        }
        taken.push((start, start + n));
        let st = if own {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(HINT)
        };
        buf.set_stringn(x0 + start as u16, gy + 1, name.as_ref(), w - start, st);
    }
    let sy = gy + 2;
    let ticks = [0.0f32, 0.25, 0.5, 0.75, 1.0];
    let mut used_to = 0usize;
    for t in ticks {
        let bp = locus.lo as f32 + t * span;
        let s = bp_text(bp);
        let c = ((t * (w - 1) as f32) as usize).saturating_sub(if t >= 1.0 {
            s.len() - 1
        } else {
            s.len() / 2 * usize::from(t > 0.0)
        });
        if c < used_to || c + s.len() > w {
            continue;
        }
        buf.set_string(x0 + c as u16, sy, &s, Style::default().fg(HINT));
        used_to = c + s.len() + 1;
    }

    // The key: the arcs' colours, then the scales, flowing onto a second
    // line where it is narrow (a part that does not fit there is left out).
    let hint = Style::default().fg(HINT);
    let ky = sy + 1;
    let mut x = area.x;
    let end = area.x + area.width;
    for (text, st) in [
        ("key: ", hint),
        ("⠉⠉ positive", Style::default().fg(mix(ARC, ARC, 1.0))),
        ("  ", hint),
        (
            "⠉⠉ negative",
            Style::default().fg(mix(ARC_NEG, ARC_NEG, 1.0)),
        ),
    ] {
        let (nx, _) = buf.set_stringn(x, ky, text, end.saturating_sub(x) as usize, st);
        x = nx;
    }
    let parts: Vec<String> = super::figure::key_parts(v_max, rate_max, style.cutoff)
        .into_iter()
        .map(|p| p.replace("bars:", "▁▂▃█:"))
        .collect();
    let width = |t: &str| t.chars().count() as f32;
    let placed = super::figure::flow(
        &parts,
        width,
        f32::from(x),
        f32::from(area.x) + 5.0,
        f32::from(end),
    );
    for ((line, at), part) in placed.iter().zip(&parts) {
        if *line > 1 {
            break;
        }
        let y = ky + *line as u16;
        let at = *at as u16;
        if *line == 0 || at > area.x + 5 {
            buf.set_string(at.saturating_sub(3), y, " · ", hint);
        }
        buf.set_stringn(at, y, part, end.saturating_sub(at) as usize, hint);
    }
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
        dots.set(x, y, 1.0);
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
    let area = Rect::new(0, 0, width, height(style.rows_of(locus).len()));
    let mut buf = Buffer::empty(area);
    render(locus, style, area, &mut buf);
    lines_of(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_dot_lands_in_its_cell_and_bit() {
        let mut d = Dots::new(2, 1);
        d.set(3.0, 2.0, 0.5);
        assert_eq!(d.bits, [0, 0x20]);
        d.set(9.0, 0.0, 1.0);
        assert_eq!(d.bits, [0, 0x20], "a dot off the grid is dropped");
        assert_eq!(d.strength[1], 0.5);
    }
}
