//! The locus figure: a gene's links in each row, as arcs or lines.

use super::*;

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
    row: &crate::view::data::Row,
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
