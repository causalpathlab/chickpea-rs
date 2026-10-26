//! The locus figure: one gene's links over its window, a row for all the
//! cells and one for each group. In each row, arcs run from the gene's
//! TSS to its linked peaks, darker where the link is stronger, above the
//! peaks' accessibility in that row; a dashed line marks the TSS and any
//! position asked for, and the genes of the window and a scale run along
//! the foot. Laid out as a bipartite graph instead, each row has the peaks
//! along its top, under their accessibility, and a straight line from each
//! linked one to the TSS at the row's foot.
//!
//! Drawn on a legume-tui frame, as cocoa draws its genome figure, so the
//! same pixels go on screen (through the terminal's image protocol, or
//! block characters), into a PNG, and on a PDF page with the text kept as
//! text.

use super::data::{Genome, Locus, Measure, Summary};
use legume_tui::map::colour::{to_linear, Lin, Srgb};
use legume_tui::map::raster::{edge, ring, stamp, Canvas, Frame, Shape};
use legume_tui::map::text::{self, Font};
use legume_tui::map::theme::Theme;

/// The page the figure is drawn on.
pub const THEME: Theme = Theme::PAPER;
/// Arcs of positive links, and their peaks' marks.
const ARC: Srgb = [92, 30, 140];
/// Arcs of negative links (a negative corr).
const ARC_NEG: Srgb = [217, 119, 87];
/// The accessibility tracks.
const TRACK: Srgb = [52, 101, 164];
/// Bands behind every other row.
const BAND: Srgb = [236, 235, 231];
/// Segments an arc is drawn in.
const ARC_STEPS: usize = 64;

/// How a locus's links are drawn; its names are clap's, `chickpea view
/// --layout`'s and `view_locus`'s.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default, clap::ValueEnum)]
pub enum Layout {
    /// Arcs from the gene's TSS to its peaks, as tall as they reach.
    #[default]
    Arcs,
    /// Peaks along the top, and a straight line from each linked one to
    /// the gene's TSS at the foot, the gene's own bar hanging under it.
    Bipartite,
}

impl Layout {
    #[must_use]
    pub fn name(self) -> String {
        clap::ValueEnum::to_possible_value(&self)
            .expect("no layout is hidden")
            .get_name()
            .to_string()
    }

    /// Every layout's name.
    #[must_use]
    pub fn names() -> Vec<String> {
        <Self as clap::ValueEnum>::value_variants()
            .iter()
            .map(|l| l.name())
            .collect()
    }

    /// What its links are called in a key: `arcs: …`, `lines: …`.
    #[must_use]
    pub fn noun(self) -> &'static str {
        match self {
            Layout::Arcs => "arcs",
            Layout::Bipartite => "lines",
        }
    }

    /// What its links stand for, in a figure's key.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Layout::Arcs => "height: distance",
            Layout::Bipartite => "lines: peak to TSS",
        }
    }

    /// How it draws the links, said to the user who turns to it.
    #[must_use]
    pub fn about(self) -> &'static str {
        match self {
            Layout::Arcs => "links as arcs from the gene's TSS",
            Layout::Bipartite => "links as straight lines from the peaks to the TSS",
        }
    }

    /// The floor and height of a row's track, the row from `y0` to `y1`
    /// (`rh` tall): under the arcs at the row's foot, or over the lines,
    /// which run down to the TSS.
    #[must_use]
    pub fn track(self, y0: f32, y1: f32, rh: f32) -> (f32, f32) {
        match self {
            Layout::Arcs => (y1 - rh * 0.06, rh * 0.32),
            Layout::Bipartite => (y0 + rh * 0.36, rh * 0.26),
        }
    }

    /// The layout after it, round again after the last.
    #[must_use]
    pub fn next(self) -> Self {
        let all = <Self as clap::ValueEnum>::value_variants();
        let i = all.iter().position(|&l| l == self).unwrap_or(0);
        all[(i + 1) % all.len()]
    }

    pub fn parse(s: &str) -> anyhow::Result<Self> {
        <Self as clap::ValueEnum>::from_str(s, false)
            .map_err(|_| anyhow::anyhow!("layout {s:?}: one of {}", Self::names().join(", ")))
    }
}

/// What the figure shows beside the locus.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Links weaker than this share of the strongest one shown are left
    /// out.
    pub cutoff: f32,
    /// Positions (bp) marked with a dashed line, beside the TSS.
    pub marks: Vec<i64>,
    /// Rows to show, by their index in [`Locus::rows`]; empty: all.
    pub rows: Vec<usize>,
    pub layout: Layout,
}

impl Style {
    /// The rows of `locus` it shows, by their index in [`Locus::rows`].
    #[must_use]
    pub fn rows_of(&self, locus: &Locus) -> Vec<usize> {
        if self.rows.is_empty() {
            (0..locus.rows.len()).collect()
        } else {
            self.rows
                .iter()
                .copied()
                .filter(|&r| r < locus.rows.len())
                .collect()
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Self {
            cutoff: 0.05,
            marks: Vec::new(),
            rows: Vec::new(),
            layout: Layout::Arcs,
        }
    }
}

/// A figure `draw` makes, rasterized to fill `area` of the screen through
/// `picker`.
pub fn on_screen(
    area: ratatui::layout::Rect,
    picker: &ratatui_image::picker::Picker,
    draw: impl FnOnce(&mut Frame, Font, Font),
) -> anyhow::Result<ratatui_image::protocol::Protocol> {
    let (cw, ch) = legume_tui::map::display::cell(picker);
    let (w, h) = (
        (f32::from(area.width) * cw) as usize,
        (f32::from(area.height) * ch) as usize,
    );
    let mut frame = Frame::blank(w.max(1), h.max(1), THEME.background);
    let (font, bold) = legume_tui::export::fonts(ch);
    draw(&mut frame, font, bold);
    legume_tui::map::display::fit_protocol(picker, frame.to_image(), area)
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// A figure `draw` makes, `page` wide and `tall` of its width high: as a
/// PDF page at `pdf`, its text kept as text, and as a PNG at `png`, its
/// text in its pixels; those given.
pub fn save(
    pdf: Option<&std::path::Path>,
    png: Option<&std::path::Path>,
    page: legume_tui::export::PageSize,
    tall: f32,
    draw: impl Fn(&mut Frame, Font, Font),
) -> anyhow::Result<()> {
    use legume_tui::export::{write_pdf, Page};
    let px = page.check(tall).map_err(|e| anyhow::anyhow!(e))?;
    if let Some(png) = png {
        let mut frame = Frame::blank(px.0, px.1, THEME.background);
        let (font, bold) = page.fonts();
        draw(&mut frame, font, bold);
        frame.to_image().save(png)?;
    }
    if let Some(pdf) = pdf {
        let (sheet, ()) = Page::draw(px, THEME.background, page.cell_px(), &draw);
        write_pdf(&[sheet], page.dpi as f32, pdf)?;
    }
    Ok(())
}

/// A figure `draw` makes, `px` pixels, as PNG bytes.
pub fn png_bytes(
    (w, h): (usize, usize),
    cell_px: f32,
    draw: impl FnOnce(&mut Frame, Font, Font),
) -> anyhow::Result<Vec<u8>> {
    let mut frame = Frame::blank(w, h, THEME.background);
    let (font, bold) = legume_tui::export::fonts(cell_px);
    draw(&mut frame, font, bold);
    let mut out = std::io::Cursor::new(Vec::new());
    frame
        .to_image()
        .write_to(&mut out, image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

/// The height of a locus figure over its width, for `rows` rows.
#[must_use]
pub fn aspect(rows: usize) -> f32 {
    (0.22 + 0.11 * rows as f32).clamp(0.4, 2.5)
}

/// A rectangle filled, clipped to the frame.
fn fill(frame: &mut Frame, x0: f32, y0: f32, x1: f32, y1: f32, c: Lin) {
    let cl = |v: f32, max: usize| (v.round().max(0.0) as usize).min(max);
    let (x0, x1) = (cl(x0.min(x1), frame.w), cl(x0.max(x1), frame.w));
    let (y0, y1) = (cl(y0.min(y1), frame.h), cl(y0.max(y1), frame.h));
    if x1 > x0 && y1 > y0 {
        frame.fill([x0, y0, x1, y1], c);
    }
}

/// A rectangle blended at `alpha`, clipped to the frame.
fn shade(frame: &mut Frame, x0: f32, y0: f32, x1: f32, y1: f32, c: Lin, alpha: f32) {
    let cl = |v: f32, max: usize| (v.round().max(0.0) as usize).min(max);
    let (x0, x1) = (
        cl(x0.min(x1), frame.w),
        cl(x0.max(x1).max(x0.min(x1) + 1.0), frame.w),
    );
    let (y0, y1) = (cl(y0.min(y1), frame.h), cl(y0.max(y1), frame.h));
    for y in y0..y1 {
        for x in x0..x1 {
            frame.blend(x, y, c, alpha);
        }
    }
}

/// A dashed vertical line at `x` from `y0` to `y1`.
fn dashed(frame: &mut Frame, x: f32, y0: f32, y1: f32, dash: f32, c: Lin) {
    let mut y = y0;
    while y < y1 {
        fill(frame, x - 0.6, y, x + 0.6, (y + dash).min(y1), c);
        y += dash * 2.0;
    }
}

/// Lanes a locus's gene names may take under its gene track.
const GENE_LANES: usize = 3;

/// The lane of each of `spans` (in the order given, the most wanted
/// first): the first of `max` lanes where it is `pad` clear of every span
/// already on it, or none where every lane is taken.
#[must_use]
pub fn lanes(spans: &[(f32, f32)], pad: f32, max: usize) -> Vec<Option<usize>> {
    let mut taken: Vec<Vec<(f32, f32)>> = vec![Vec::new(); max];
    spans
        .iter()
        .map(|&(a, b)| {
            let lane = taken
                .iter()
                .position(|t| t.iter().all(|&(c, d)| b + pad <= c || a >= d + pad))?;
            taken[lane].push((a, b));
            Some(lane)
        })
        .collect()
}

/// A straight line from `a` to `b`, `thick` pixels across.
fn line(frame: &mut Frame, a: (f32, f32), b: (f32, f32), thick: usize, c: Lin, alpha: f32) {
    // Copies side by side across the line, so it is as thick at any slope.
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len = dx.hypot(dy).max(f32::EPSILON);
    let (nx, ny) = (-dy / len, dx / len);
    for t in 0..thick {
        let o = (t as f32 - (thick - 1) as f32 / 2.0) * 0.8;
        let (ox, oy) = (nx * o, ny * o);
        edge(
            frame,
            (a.0 + ox, a.1 + oy),
            (b.0 + ox, b.1 + oy),
            (0.0, 0.0),
            c,
            alpha,
        );
    }
}

/// A half-ellipse from `(a, base)` to `(b, base)`, `height` tall.
fn arc(frame: &mut Frame, a: f32, b: f32, base: f32, height: f32, c: Lin, alpha: f32) {
    let (mid, rx) = ((a + b) / 2.0, (b - a) / 2.0);
    let at = |i: usize| {
        let t = std::f32::consts::PI * i as f32 / ARC_STEPS as f32;
        (mid - rx * t.cos(), base - height * t.sin())
    };
    for i in 0..ARC_STEPS {
        edge(frame, at(i), at(i + 1), (0.0, 0.0), c, alpha);
    }
}

/// Round steps for an axis over `[lo, hi]`: about `n` of them.
fn ticks(lo: f32, hi: f32, n: usize) -> Vec<f32> {
    let raw = (hi - lo) / n.max(1) as f32;
    if !(raw.is_finite() && raw > 0.0) {
        return Vec::new();
    }
    let mag = 10f32.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .into_iter()
        .map(|m| m * mag)
        .find(|s| *s >= raw)
        .unwrap_or(10.0 * mag);
    let mut t = (lo / step).ceil() * step;
    let mut out = Vec::new();
    while t <= hi {
        out.push(t);
        t += step;
    }
    out
}

/// A row's strongest link, by size.
#[must_use]
pub fn row_strongest(values: &[f32]) -> f32 {
    values
        .iter()
        .map(|v| v.abs())
        .filter(|v| v.is_finite())
        .fold(0f32, f32::max)
}

/// The scales `rows` of `locus` share: the strongest link, and the most
/// open peak.
#[must_use]
pub fn scales(locus: &Locus, rows: &[usize]) -> (f32, f32) {
    let top = |v: &mut dyn Iterator<Item = f32>| v.filter(|v| v.is_finite()).fold(0f32, f32::max);
    (
        top(&mut rows
            .iter()
            .flat_map(|&r| locus.rows[r].value.iter())
            .map(|v| v.abs())),
        top(&mut rows
            .iter()
            .flat_map(|&r| locus.rows[r].rate.iter())
            .copied()),
    )
}

/// Whether a link of value `v` is drawn in a row whose strongest is
/// `row_max`: the cutoff is the row's own, so a row's arcs do not change
/// with the other rows shown.
#[must_use]
pub fn drawn(v: f32, row_max: f32, cutoff: f32) -> bool {
    v.is_finite() && v != 0.0 && v.abs() >= cutoff * row_max
}

/// The genes of `locus` named under their ticks, in the order their names
/// are placed: its own first, then those ← and → step to, nearest first;
/// the others keep their ticks only.
#[must_use]
pub fn named(locus: &Locus) -> Vec<&(Box<str>, i64)> {
    let mut order: Vec<&(Box<str>, i64)> = locus
        .genes
        .iter()
        .filter(|(name, _)| {
            **name == *locus.gene || locus.steps.iter().flatten().any(|s| s == name)
        })
        .collect();
    order.sort_by_key(|(name, t)| (**name != *locus.gene, (t - locus.tss.tss).abs()));
    order
}

/// A number for a key: three decimals, or two digits when it is small.
#[must_use]
pub fn short(v: f32) -> String {
    if v == 0.0 {
        "0".into()
    } else if v.abs() >= 0.01 {
        format!("{v:.3}")
    } else {
        format!("{v:.1e}")
    }
}

/// What a locus key says after its arc colours, part by part (a line
/// breaks only between parts): what shade, height and the bars stand for,
/// with the scales of the figure, and the share of its row's strongest a
/// link needs to be drawn.
#[must_use]
pub fn key_parts(v_max: f32, rate_max: f32, cutoff: f32, layout: Layout) -> Vec<String> {
    let mut parts = vec![format!("darker: stronger (top {})", short(v_max))];
    if cutoff > 0.0 {
        parts.push(format!("shown: {} of the row's top", short(cutoff)));
    }
    parts.push(layout.key().into());
    parts.push(format!("bars: accessibility (top {})", short(rate_max)));
    parts
}

/// `parts` laid out from `x` on line 0 within `[left, right)`, ` · ` before
/// each one but at a line's start, a part that would cross `right` starting
/// the next line at `left`: each part's line and start.
pub fn flow(
    parts: &[String],
    width: impl Fn(&str) -> f32,
    x: f32,
    left: f32,
    right: f32,
) -> Vec<(usize, f32)> {
    let sep = width(" · ");
    let (mut line, mut at) = (0usize, x);
    let mut out = Vec::with_capacity(parts.len());
    for p in parts {
        let lead = if at <= left { 0.0 } else { sep };
        if at + lead + width(p) > right && at > left {
            line += 1;
            at = left;
            out.push((line, at));
        } else {
            out.push((line, at + lead));
            at += lead;
        }
        at += width(p);
    }
    out
}

/// A genome position as a scale shows it: `1.25 Mb`, `250 kb`, `800 bp`.
#[must_use]
pub fn bp_text(bp: f32) -> String {
    let a = bp.abs();
    if a >= 1e6 {
        format!("{:.2} Mb", bp / 1e6)
    } else if a >= 1e3 {
        format!("{:.0} kb", bp / 1e3)
    } else {
        format!("{bp:.0} bp")
    }
}

/// Draw `locus` on `frame`.
pub fn draw_locus(frame: &mut Frame, font: Font, bold: Font, locus: &Locus, style: &Style) {
    let lh = font.line_height() as f32;
    let (w, h) = (frame.w as f32, frame.h as f32);
    let bg = frame.background();
    let ink = to_linear(THEME.text);
    let hint = to_linear(THEME.hint);
    let muted = to_linear(THEME.muted);

    let title = format!(
        "{} · {}:{}-{}{}",
        locus.gene,
        locus.tss.chr,
        locus.lo,
        locus.hi,
        if locus.tss.inferred {
            " · TSS inferred"
        } else {
            ""
        }
    );
    text::draw(frame, bold, &title, lh as i32, (lh * 0.3) as i32, ink, bg);
    let drawn_as = style.layout.noun();
    let legend = match locus.measure {
        Measure::Corr => format!("{drawn_as}: {}", Measure::Corr.shown()),
        m => format!("{drawn_as}: {}, in each row", m.shown()),
    };
    let legend = legend.as_str();
    let lw = font.width(legend) as f32;
    // Beside the title where both fit, else first in the key.
    let legend_beside = lh + bold.width(&title) as f32 + lh * 2.0 + lw <= w - lh;
    if legend_beside {
        text::draw(
            frame,
            font,
            legend,
            (w - lh - lw) as i32,
            (lh * 0.3) as i32,
            hint,
            bg,
        );
    }

    let rows = style.rows_of(locus);
    let labels: Vec<(String, String)> = rows
        .iter()
        .map(|&r| {
            let row = &locus.rows[r];
            let cells = row.cells.map_or_else(String::new, |n| format!("{n} cells"));
            (row.label.clone(), cells)
        })
        .collect();
    let label_w = labels
        .iter()
        .map(|(a, b)| font.width(a).max(font.width(b)))
        .max()
        .unwrap_or(0) as f32;
    let x0 = lh + label_w + lh;
    let x1 = w - lh;
    // One scale for every row's links, and one for every row's tracks.
    let (v_max, rate_max) = scales(locus, &rows);

    // The key: the arcs' colours, then the scales, flowing onto more lines
    // where the page is narrow; the rows start below it.
    let ky = lh * 1.55;
    let mut kx = lh;
    let sw = lh * 0.55;
    for (i, (c, what)) in [(ARC, "positive"), (ARC_NEG, "negative")]
        .into_iter()
        .enumerate()
    {
        if i > 0 {
            kx += lh * 0.6;
        }
        fill(
            frame,
            kx,
            ky + (lh - sw) * 0.5,
            kx + sw,
            ky + (lh + sw) * 0.5,
            to_linear(c),
        );
        kx += sw + lh * 0.3;
        text::draw(frame, font, what, kx as i32, ky as i32, hint, bg);
        kx += font.width(what) as f32;
    }
    let mut lead = Vec::new();
    if !legend_beside {
        lead.push(legend.to_string());
    }
    lead.extend(key_parts(v_max, rate_max, style.cutoff, style.layout));
    // The gene's own bar, in the bipartite layout: one scale for its rows.
    let activity_max = rows
        .iter()
        .map(|&r| locus.rows[r].activity)
        .fold(0f32, f32::max);
    if style.layout == Layout::Bipartite {
        let by = if locus.measure == Measure::Abc {
            "ABC prior"
        } else {
            "gate"
        };
        lead.push(format!(
            "under the TSS: its peaks pooled by {by} (top {})",
            short(activity_max)
        ));
    }
    let width = |t: &str| font.width(t) as f32;
    let placed = flow(&lead, width, kx, lh, w - lh);
    for ((line, x), part) in placed.iter().zip(&lead) {
        let y = ky + lh * *line as f32;
        let gap = width(" · ");
        if x - gap >= lh {
            text::draw(frame, font, " · ", (x - gap) as i32, y as i32, hint, bg);
        }
        text::draw(frame, font, part, *x as i32, y as i32, hint, bg);
    }
    let key_lines = placed.last().map_or(0, |p| p.0) + 1;
    let top = lh * (2.0 + key_lines as f32);
    let span = (locus.hi - locus.lo).max(1) as f32;
    let xs = |bp: i64| x0 + (bp - locus.lo) as f32 / span * (x1 - x0);
    // The names under the ticks (see `named`), each on the first lane
    // where it is clear.
    // Each name, its font and colour, and where it starts and ends.
    let names: Vec<(&str, Font, Lin, (f32, f32))> = named(locus)
        .into_iter()
        .map(|(name, t)| {
            let (f, c) = if **name == *locus.gene {
                (bold, ink)
            } else {
                (font, hint)
            };
            let tw = f.width(name) as f32;
            let tx = (xs(*t) - tw / 2.0).clamp(x0, (x1 - tw).max(x0));
            (&**name, f, c, (tx, tx + tw))
        })
        .collect();
    let spans: Vec<(f32, f32)> = names.iter().map(|n| n.3).collect();
    let name_lanes = lanes(&spans, lh * 0.4, GENE_LANES);
    let more = name_lanes.iter().flatten().max().map_or(0, |&l| l) as f32;
    let foot = lh * (4.2 + more);
    let y_end = h - foot;
    if x1 - x0 < lh * 4.0 || y_end - top < lh * 2.0 || rows.is_empty() {
        return;
    }
    let tss_x = xs(locus.tss.tss);
    let rh = (y_end - top) / rows.len() as f32;

    for (i, &r) in rows.iter().enumerate() {
        let row = &locus.rows[r];
        let y0 = top + rh * i as f32;
        let y1 = y0 + rh;
        if i % 2 == 1 {
            fill(frame, 0.0, y0, w, y1, to_linear(BAND));
        }
        let band_bg = if i % 2 == 1 { to_linear(BAND) } else { bg };
        let (floor, track_h) = style.layout.track(y0, y1, rh);

        // The label, centred on the arcs.
        let (name, cells) = &labels[i];
        let ly = (y0 + rh * 0.35 - lh * 0.5) as i32;
        let lx = (lh + label_w - font.width(name) as f32) as i32;
        text::draw(frame, bold, name, lx, ly, ink, band_bg);
        if !cells.is_empty() {
            let cx = (lh + label_w - font.width(cells) as f32) as i32;
            text::draw(frame, font, cells, cx, ly + lh as i32, hint, band_bg);
        }

        // The track: each peak's accessibility as a bar on its span.
        fill(frame, x0, floor, x1, floor + 1.0, muted);
        if rate_max > 0.0 {
            for ((_, p), &v) in locus.peaks.iter().zip(&row.rate) {
                if v > 0.0 {
                    let (a, b) = (xs(p.start), xs(p.end).max(xs(p.start) + 1.5));
                    let top_y = floor - (v / rate_max).min(1.0) * track_h;
                    fill(frame, a, top_y, b, floor, to_linear(TRACK));
                }
            }
        }

        let place = RowPlace {
            y0,
            y1,
            rh,
            floor,
            track_h,
            tss_x,
            lh,
            width: x1 - x0,
        };
        let strokes = strokes(locus, row, style.cutoff, v_max, lh, &xs);
        match style.layout {
            Layout::Arcs => arcs_row(frame, &strokes, &place),
            Layout::Bipartite => {
                let open = if activity_max > 0.0 {
                    row.activity / activity_max
                } else {
                    0.0
                };
                lines_row(frame, &strokes, &place, open);
            }
        }
    }

    // The TSS (where the arcs meet) and the marks, through every row.
    if style.layout == Layout::Arcs {
        dashed(frame, tss_x, top, y_end, lh * 0.25, hint);
    }
    for &m in &style.marks {
        if (locus.lo..=locus.hi).contains(&m) {
            dashed(frame, xs(m), top, y_end, lh * 0.35, ink);
        }
    }

    // The genes of the window: a tick at each TSS, the gene's own in bold.
    let gy = y_end + lh * 0.6;
    fill(frame, x0, gy, x1, gy + 1.0, muted);
    for (name, t) in &locus.genes {
        let x = xs(*t);
        let c = if **name == *locus.gene { ink } else { hint };
        fill(frame, x - 0.8, gy - lh * 0.35, x + 0.8, gy + lh * 0.35, c);
    }
    for (&(name, f, c, (tx, _)), lane) in names.iter().zip(&name_lanes) {
        let Some(lane) = lane else { continue };
        let y = gy + lh * (0.4 + *lane as f32);
        text::draw(frame, f, name, tx as i32, y as i32, c, bg);
    }

    // The scale: positions along the window.
    let ay = y_end + lh * (2.4 + more);
    fill(frame, x0, ay, x1, ay + 1.0, hint);
    for t in ticks(locus.lo as f32, locus.hi as f32, 6) {
        let x = xs(t as i64);
        fill(frame, x - 0.5, ay, x + 0.5, ay + lh * 0.3, hint);
        let s = bp_text(t);
        let tw = font.width(&s) as f32;
        text::draw(
            frame,
            font,
            &s,
            (x - tw / 2.0) as i32,
            (ay + lh * 0.35) as i32,
            hint,
            bg,
        );
    }
}

/// Where a locus row's parts go, in pixels: its top and foot, the
/// track's floor and height, the TSS, the line height that sizes marks,
/// and the window's width.
struct RowPlace {
    y0: f32,
    y1: f32,
    rh: f32,
    floor: f32,
    track_h: f32,
    tss_x: f32,
    lh: f32,
    width: f32,
}

/// A link as a row draws it: its peak's x, its colour, and how strong it
/// looks (the square root of its strength, and the alpha and thickness
/// that come of it).
struct Stroke {
    px: f32,
    c: Lin,
    s: f32,
    alpha: f32,
    thick: usize,
}

/// The links `row` shows, by its own cutoff, on the scale `v_max` of all
/// the rows: the weakest first, so the strong ones lie on top; the strong
/// dark and thick, the weak faint and thin.
fn strokes(
    locus: &Locus,
    row: &super::data::Row,
    cutoff: f32,
    v_max: f32,
    lh: f32,
    xs: &impl Fn(i64) -> f32,
) -> Vec<Stroke> {
    if v_max <= 0.0 {
        return Vec::new();
    }
    let top = row_strongest(&row.value);
    let mut order: Vec<usize> = (0..locus.links.len())
        .filter(|&k| drawn(row.value[k], top, cutoff))
        .collect();
    order.sort_by(|&a, &b| row.value[a].abs().total_cmp(&row.value[b].abs()));
    order
        .into_iter()
        .filter_map(|k| {
            let v = row.value[k];
            let p = locus.links[k].place.as_ref()?;
            let s = (v.abs() / v_max).sqrt();
            Some(Stroke {
                px: xs(p.mid()),
                c: to_linear(if v < 0.0 { ARC_NEG } else { ARC }),
                s,
                alpha: (0.2 + 0.8 * s).min(1.0),
                thick: 1 + (s * lh * 0.18).round() as usize,
            })
        })
        .collect()
}

/// A row's links as arcs from the TSS, standing on its track: as tall as
/// they reach, and a peak at the TSS as a stem.
fn arcs_row(frame: &mut Frame, strokes: &[Stroke], at: &RowPlace) {
    let base = at.floor - at.track_h;
    let arc_h = base - at.y0 - at.rh * 0.08;
    let m = at.lh * 0.12;
    for k in strokes {
        let reach = ((k.px - at.tss_x).abs() / at.width * 2.2).clamp(0.12, 1.0);
        let (a, b) = (k.px.min(at.tss_x), k.px.max(at.tss_x));
        for t in 0..k.thick {
            arc(
                frame,
                a,
                b,
                base - t as f32 * 0.7,
                arc_h * reach,
                k.c,
                k.alpha,
            );
        }
        shade(
            frame,
            k.px - m * 1.5,
            base - m,
            k.px + m * 1.5,
            base + m * 0.5,
            k.c,
            k.alpha,
        );
        // A peak at the TSS has no arc to speak of: a stem as tall as its
        // link is strong says it instead.
        if (k.px - at.tss_x).abs() < at.lh * 0.6 {
            let stem = arc_h * (0.15 + 0.6 * k.s);
            shade(
                frame,
                k.px - m * 0.6,
                base - stem,
                k.px + m * 0.6,
                base,
                k.c,
                k.alpha,
            );
        }
    }
}

/// A row's links as straight lines from the peaks, under their track, to
/// the TSS, the gene's own bar hanging under it, `open` of the deepest.
fn lines_row(frame: &mut Frame, strokes: &[Stroke], at: &RowPlace, open: f32) {
    let tss_y = at.y1 - at.rh * 0.3;
    let (depth, half) = (open.min(1.0) * at.rh * 0.22, at.lh * 0.35);
    fill(
        frame,
        at.tss_x - half,
        tss_y,
        at.tss_x + half,
        tss_y + depth,
        to_linear(TRACK),
    );
    for k in strokes {
        line(
            frame,
            (k.px, at.floor + 1.0),
            (at.tss_x, tss_y),
            k.thick,
            k.c,
            k.alpha,
        );
    }
}

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
    let seen = |t: f32| t >= from && t <= from + span;
    let top = genome
        .points
        .iter()
        .filter(|p| seen(p.1 as f32 / len))
        .map(|p| p.2)
        .fold(0f32, f32::max);
    let lo = genome
        .points
        .iter()
        .map(|p| p.2)
        .fold(0f32, f32::min)
        .min(0.0);
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
    let mut best: Vec<&(usize, u64, f32)> = genome
        .points
        .iter()
        .filter(|p| seen(p.1 as f32 / len) && Some(p.0) != view.focus && p.2 > 0.0)
        .collect();
    best.sort_by(|a, b| b.2.total_cmp(&a.2));
    let mut named = 0;
    for &&(g, pos, v) in best.iter().take(NAMED * 3) {
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
        let at = |t: f32| {
            // The last base in sight, not the first past it.
            let pos = ((genome.len() as f32 * t) as u64).saturating_sub(u64::from(t > from));
            let k = chrom_of(pos);
            let (name, off, _) = &genome.chroms[k];
            (name, bp_text((pos - off) as f32))
        };
        let ((ca, a), (cb, b)) = (at(from), at(from + span));
        let s = if ca == cb {
            format!("{ca}: {a} - {b}")
        } else {
            format!("{ca}: {a} - {cb}: {b}")
        };
        text::draw(frame, font, &s, x0 as i32, (y1 + lh * 0.3) as i32, hint, bg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_view_stays_inside_the_genome() {
        let v = GenomeView {
            from: 0.9,
            span: 0.25,
            focus: None,
        };
        assert_eq!(v.window(), (0.75, 0.25));
    }

    #[test]
    fn ticks_are_round_and_inside() {
        let t = ticks(1_234.0, 98_765.0, 5);
        assert!(t.iter().all(|&v| (1_234.0..=98_765.0).contains(&v)));
        assert!(t.iter().all(|v| v % 10_000.0 == 0.0), "{t:?}");
    }

    /// GENE1 at 50 kb, linked to a peak at 20 kb.
    fn locus() -> Locus {
        Locus::for_tests(&[("p1", 19_750, 0.8)], &[("All cells", &[0.8])])
    }

    fn drawn_as(layout: Layout) -> image::RgbaImage {
        let mut frame = Frame::blank(600, 300, THEME.background);
        let (font, bold) = legume_tui::export::fonts(12.0);
        let style = Style {
            layout,
            ..Style::default()
        };
        draw_locus(&mut frame, font, bold, &locus(), &style);
        frame.to_image()
    }

    /// The lowest row of `img`, below its key, with a pixel tinted toward
    /// `c` from the page, more than toward the figure's other colours: away
    /// from the page by half the way to `c` at least (antialiased lines are
    /// never quite `c`).
    fn lowest_of(img: &image::RgbaImage, c: Srgb) -> Option<u32> {
        let bg = THEME.background.map(f32::from);
        let away = |c: Srgb| -> [f32; 3] { std::array::from_fn(|i| f32::from(c[i]) - bg[i]) };
        let norm = |v: &[f32; 3]| v.iter().map(|x| x * x).sum::<f32>().sqrt();
        let cos = |a: &[f32; 3], b: &[f32; 3]| {
            a.iter().zip(b).map(|(x, y)| x * y).sum::<f32>() / (norm(a) * norm(b))
        };
        let e = away(c);
        let rest: Vec<[f32; 3]> = [ARC, ARC_NEG, TRACK]
            .into_iter()
            .filter(|&o| o != c)
            .map(away)
            .collect();
        img.enumerate_pixels()
            .filter(|(_, y, _)| *y > img.height() / 3)
            .filter(|(_, _, p)| {
                let d = away([p[0], p[1], p[2]]);
                let to_c = cos(&d, &e);
                norm(&d) > 0.5 * norm(&e) && to_c > 0.97 && rest.iter().all(|o| cos(&d, o) < to_c)
            })
            .map(|(_, y, _)| y)
            .max()
    }

    #[test]
    fn bipartite_lines_run_down_to_the_tss_below_the_peaks() {
        let arcs = lowest_of(&drawn_as(Layout::Arcs), ARC).expect("an arc");
        let lines = lowest_of(&drawn_as(Layout::Bipartite), ARC).expect("a line");
        assert!(
            lines > arcs + 10,
            "arcs stand on the track at {arcs}, lines end at {lines}"
        );
    }

    #[test]
    fn the_genes_own_bar_hangs_under_its_tss_in_the_bipartite_layout() {
        let bip = drawn_as(Layout::Bipartite);
        let bar = lowest_of(&bip, TRACK).expect("the gene's bar");
        let lines = lowest_of(&bip, ARC).expect("a line");
        assert!(
            bar > lines,
            "the bar to {bar}, under the lines' end at {lines}"
        );
        assert_eq!(
            lowest_of(&drawn_as(Layout::Arcs), TRACK),
            None,
            "arcs have none"
        );
    }

    #[test]
    fn only_the_gene_and_its_steps_are_named() {
        let mut l = locus();
        l.genes = ["GENE0", "GENE1", "GENE2", "GENE3"]
            .iter()
            .zip([10_000, 50_000, 60_000, 90_000])
            .map(|(n, t)| ((*n).into(), t))
            .collect();
        l.steps = [Some("GENE0".into()), Some("GENE2".into())];
        let named: Vec<&str> = named(&l).iter().map(|(n, _)| &**n).collect();
        assert_eq!(
            named,
            ["GENE1", "GENE2", "GENE0"],
            "the gene, then nearest first"
        );
    }

    #[test]
    fn names_that_would_overlap_go_on_lanes_below() {
        // In the order they are placed: three crowded together, then one
        // apart, then one with no lane left.
        let spans = [
            (10.0, 30.0),
            (20.0, 40.0),
            (100.0, 120.0),
            (25.0, 35.0),
            (15.0, 45.0),
        ];
        assert_eq!(
            lanes(&spans, 2.0, GENE_LANES),
            [Some(0), Some(1), Some(0), Some(2), None]
        );
    }

    #[test]
    fn a_thick_line_is_thick_at_any_slope() {
        // Pixels inked down column 300 of a nearly flat line, and along
        // row 150 of a steep one.
        let ink = |a: (f32, f32), b: (f32, f32), across: &dyn Fn(usize) -> (usize, usize)| {
            let mut frame = Frame::blank(600, 300, THEME.background);
            line(&mut frame, a, b, 5, to_linear(ARC), 1.0);
            let img = frame.to_image();
            let bg = image::Rgba([
                THEME.background[0],
                THEME.background[1],
                THEME.background[2],
                255,
            ]);
            (0..300)
                .map(across)
                .filter(|&(x, y)| *img.get_pixel(x as u32, y as u32) != bg)
                .count()
        };
        let flat = ink((10.0, 140.0), (590.0, 160.0), &|y| (300, y));
        let steep = ink((290.0, 10.0), (310.0, 290.0), &|x| (x, 150));
        assert!(flat >= 4 && steep >= 3, "flat {flat}, steep {steep}");
    }

    #[test]
    fn a_layout_is_parsed_by_name() {
        assert_eq!(Layout::parse("bipartite").unwrap(), Layout::Bipartite);
        assert_eq!(Layout::parse(&Layout::Arcs.name()).unwrap(), Layout::Arcs);
        assert!(Layout::parse("tree").is_err());
        assert_eq!(Layout::Arcs.next(), Layout::Bipartite);
        assert_eq!(Layout::Bipartite.next(), Layout::Arcs);
    }

    #[test]
    fn positions_read_in_their_unit() {
        assert_eq!(bp_text(1_250_000.0), "1.25 Mb");
        assert_eq!(bp_text(250_000.0), "250 kb");
        assert_eq!(bp_text(800.0), "800 bp");
    }
}
