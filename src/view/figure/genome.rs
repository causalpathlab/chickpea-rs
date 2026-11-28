//! The genome-wide figure: every placed gene's summary along the genome.

use super::*;

/// Points of the genome-wide figure, on alternate chromosomes.
const POINT: [Srgb; 2] = [[52, 101, 164], [120, 132, 150]];
/// Genes named on the genome-wide figure at most.
pub const NAMED: usize = 12;

/// What part of the genome the genome-wide figure shows, and which gene
/// it rings.
#[derive(Clone, Debug, PartialEq)]
pub struct GenomeView {
    /// Where the view starts along the genome, and how much of it it
    /// shows, as fractions of it (0 and 1: all of it).
    pub from: f32,
    pub span: f32,
    /// The gene ringed, by its index in the run's genes.
    pub focus: Option<usize>,
}

impl Default for GenomeView {
    fn default() -> Self {
        Self {
            from: 0.0,
            span: 1.0,
            focus: None,
        }
    }
}

impl GenomeView {
    /// The span shown, kept inside the genome.
    #[must_use]
    pub fn window(&self) -> (f32, f32) {
        let span = self.span.clamp(1e-4, 1.0);
        (self.from.clamp(0.0, 1.0 - span), span)
    }

    /// Whether `t` (a fraction of the genome) is in sight.
    #[must_use]
    pub fn sees(&self, t: f32) -> bool {
        let (from, span) = self.window();
        t >= from && t <= from + span
    }
}

/// How a genome-wide figure's values read: `best score`, `links ≥ 0.1 by corr`.
#[must_use]
pub fn summary_text(m: Measure, how: Summary) -> String {
    match how {
        Summary::Best => format!("best {}", m.name()),
        Summary::Count(c) => format!("links with {} ≥ {c}", m.name()),
    }
}

/// The height of a genome-wide figure over its width.
pub const GENOME_ASPECT: f32 = 0.42;

/// Draw `genome` on `frame`, `names` the run's genes.
pub fn draw_genome(
    frame: &mut Frame,
    font: Font,
    bold: Font,
    genome: &Genome,
    names: &[Box<str>],
    view: &GenomeView,
) {
    let lh = font.line_height() as f32;
    let adv = font.advance() as f32;
    let (w, h) = (frame.w as f32, frame.h as f32);
    let bg = frame.background();
    let ink = to_linear(THEME.text);
    let hint = to_linear(THEME.hint);
    let muted = to_linear(THEME.muted);

    // The raster's font has no ≥ or en dash.
    let what = summary_text(genome.measure, genome.summary).replace('≥', ">=");
    text::draw(
        frame,
        bold,
        &format!("Genome-wide · {what}"),
        lh as i32,
        (lh * 0.3) as i32,
        ink,
        bg,
    );
    let x0 = lh + adv * 7.0;
    let x1 = w - lh;
    let y0 = lh * 3.4;
    let y1 = h - lh * 1.6;
    if x1 - x0 < lh * 4.0 || y1 - y0 < lh * 3.0 || genome.chroms.is_empty() {
        return;
    }
    let (from, span) = view.window();
    let len = genome.len().max(1) as f32;
    let xs = |t: f32| x0 + (t - from) / span * (x1 - x0);
    let seen = |t: f32| view.sees(t);
    // The axis spans the values in sight, as the text figure's does.
    let in_sight: Vec<&(usize, u64, f32)> = genome.in_view(view).collect();
    let (lo, top) = in_sight
        .iter()
        .fold((0f32, 0f32), |(lo, top), p| (lo.min(p.2), top.max(p.2)));
    let hi = if top > 0.0 { top * 1.08 } else { 1.0 };
    let ys = |v: f32| y1 - (v - lo) / (hi - lo) * (y1 - y0);

    // Chromosome bands and names.
    for (k, (name, off, l)) in genome.chroms.iter().enumerate() {
        let (a, b) = (
            xs(*off as f32 / len).max(x0),
            xs((off + l) as f32 / len).min(x1),
        );
        if b <= a {
            continue;
        }
        if k % 2 == 1 {
            fill(frame, a, y0, b, y1, to_linear(BAND));
        }
        let tw = font.width(name) as f32;
        if b - a > tw * 1.1 {
            text::draw(
                frame,
                font,
                name,
                ((a + b - tw) / 2.0) as i32,
                (y0 - lh * 1.2) as i32,
                hint,
                bg,
            );
        }
    }
    // The axis and its ticks.
    fill(frame, x0 - 1.0, y0, x0, y1, hint);
    let counts = matches!(genome.summary, Summary::Count(_));
    for t in ticks(lo, hi, 4) {
        // A count has whole ticks only.
        if counts && t.fract() != 0.0 {
            continue;
        }
        let y = ys(t);
        fill(frame, x0 - adv * 0.6, y, x0, y + 1.0, hint);
        let s = if counts || hi - lo >= 10.0 {
            format!("{t:.0}")
        } else {
            format!("{t:.2}")
        };
        let tw = font.width(&s) as f32;
        text::draw(
            frame,
            font,
            &s,
            (x0 - adv - tw) as i32,
            (y - lh * 0.5) as i32,
            hint,
            bg,
        );
    }
    if lo < 0.0 {
        fill(frame, x0, ys(0.0), x1, ys(0.0) + 1.0, muted);
    }

    // The genes, by chromosome colour.
    let chrom_of = |pos: u64| genome.chrom_at(pos);
    let r = (lh * 0.16).max(1.5);
    for &(_, pos, v) in &genome.points {
        let t = pos as f32 / len;
        if !seen(t) {
            continue;
        }
        let c = to_linear(POINT[chrom_of(pos) % 2]);
        stamp(frame, xs(t), ys(v), r, Shape::Circle, c, 0.85);
    }

    // The gene in focus, ringed and named first; then the best genes in
    // sight, named where there is room.
    let mut taken: Vec<[f32; 4]> = Vec::new();
    let label = |frame: &mut Frame,
                 f: Font,
                 name: &str,
                 x: f32,
                 y: f32,
                 c: Lin,
                 taken: &mut Vec<[f32; 4]>| {
        let tw = f.width(name) as f32;
        let (lx, ly) = ((x + r * 1.8).min(x1 - tw), y - lh * 1.05);
        let rect = [lx, ly, lx + tw, ly + lh];
        let clash =
            |b: &[f32; 4]| rect[0] < b[2] && rect[2] > b[0] && rect[1] < b[3] && rect[3] > b[1];
        if ly < y0 - lh * 0.2 || taken.iter().any(clash) {
            return false;
        }
        taken.push(rect);
        text::draw(frame, f, name, lx as i32, ly as i32, c, bg);
        true
    };
    let focus = view
        .focus
        .and_then(|f| genome.points.iter().find(|p| p.0 == f));
    if let Some(&(g, pos, v)) = focus {
        let t = pos as f32 / len;
        if seen(t) {
            let (x, y) = (xs(t), ys(v));
            ring(frame, x, y, r * 2.4, to_linear(ARC));
            label(frame, bold, &names[g], x, y, to_linear(ARC), &mut taken);
        }
    }
    let best = Genome::best_first(&in_sight);
    let mut named = 0;
    for &&(g, pos, v) in best
        .iter()
        .filter(|p| Some(p.0) != view.focus)
        .take(NAMED * 3)
    {
        if named >= NAMED {
            break;
        }
        if label(
            frame,
            font,
            &names[g],
            xs(pos as f32 / len),
            ys(v),
            ink,
            &mut taken,
        ) {
            named += 1;
        }
    }
    // Where the view is along the genome, when zoomed in.
    if span < 0.999 {
        // On the chromosomes it covers: `17: 0 bp - 131 kb`, or across
        // several, `3: 10 Mb - 5: 2 Mb`.
        let at = |t: f32, end: bool| {
            let (name, bp) = genome.place_at(t, end);
            (name, bp_text(bp as f32))
        };
        let ((ca, a), (cb, b)) = (at(from, false), at(from + span, true));
        let s = if ca == cb {
            format!("{ca}: {a} - {b}")
        } else {
            format!("{ca}: {a} - {cb}: {b}")
        };
        text::draw(frame, font, &s, x0 as i32, (y1 + lh * 0.3) as i32, hint, bg);
    }
}
