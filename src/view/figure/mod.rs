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
    /// Links weaker than this share of their row's strongest are left
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

mod genome;
mod locus;
pub use genome::{draw_genome, summary_text, GenomeView, GENOME_ASPECT, NAMED};
pub use locus::draw_locus;

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
