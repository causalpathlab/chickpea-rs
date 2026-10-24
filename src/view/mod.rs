//! `chickpea view`: look at a finished run's links, genome-wide and at a
//! gene's locus.
//!
//! [`data`] reads a run; [`State`] is what is being looked at (the run, the
//! gene, the measure, how genes are summarised), kept between calls by the
//! MCP tools as the screen keeps it between keys; [`tui`] is the screen.

pub mod data;
pub mod figure;
pub mod mcp;
pub mod text;
pub mod tui;

use crate::common::mkdir_parent;
use data::{Measure, Run, Summary};
use legume_tui::map::raster::Frame;
use legume_tui::map::text::Font;

#[derive(clap::Args, Debug)]
pub struct ViewArgs {
    /// A run's out prefix, or one of its tables ({out}.links.parquet)
    pub run: Option<String>,

    /// Open at this gene's locus
    #[arg(long)]
    pub gene: Option<String>,

    /// Draw the genome-wide view instead of a gene's locus (with --pdf,
    /// --png or --text)
    #[arg(long)]
    pub genome: bool,

    /// How the genome-wide view sums up a gene's links: its best, or how
    /// many reach --count-cutoff
    #[arg(long, default_value = "best", value_parser = ["best", "count"])]
    pub summary: String,

    /// With --summary count: the least value a link counts at
    #[arg(long, default_value_t = data::COUNT_CUTOFF)]
    pub count_cutoff: f32,

    /// Show only this chromosome of the genome-wide view
    #[arg(long)]
    pub chr: Option<String>,

    /// What a link is measured by
    #[arg(long, default_value = "score", value_parser = ["score", "gate", "corr", "abc"])]
    pub measure: String,

    /// bp each side of the TSS the locus shows [default: its farthest link]
    #[arg(long)]
    pub half_window: Option<i64>,

    /// Links weaker than this share of the strongest shown are left out
    #[arg(long, default_value_t = 0.05)]
    pub cutoff: f32,

    /// A position (bp) to mark with a dashed line; again for more
    #[arg(long = "mark")]
    pub marks: Vec<i64>,

    /// Rows to show, comma-separated: `all` (all the cells) and groups by
    /// name (C0, or the run's cell types), or by number as the locus lists
    /// them (0 all, 1 the first group) [default: all of them]
    #[arg(long, value_delimiter = ',')]
    pub rows: Vec<String>,

    /// Write the locus figure as a PDF here and exit (needs --gene)
    #[arg(long)]
    pub pdf: Option<std::path::PathBuf>,

    /// Write the locus figure as a PNG here and exit (needs --gene)
    #[arg(long)]
    pub png: Option<std::path::PathBuf>,

    /// Print the locus figure as text this many columns wide and exit
    /// (needs --gene)
    #[arg(long, value_name = "COLUMNS")]
    pub text: Option<u16>,

    /// Width of an exported figure, in inches
    #[arg(long, default_value_t = 7.0)]
    pub width_in: f32,

    /// Resolution of an exported figure
    #[arg(long, default_value_t = 300)]
    pub dpi: u32,

    #[command(flatten)]
    pub session: legume_tui::cli::SessionArgs,
}

/// What is being looked at.
pub struct State {
    pub run: Run,
    /// The gene whose locus is shown.
    pub gene: Option<usize>,
    pub measure: Measure,
    pub summary: Summary,
    /// bp each side of the TSS; none: the gene's farthest link.
    pub half_window: Option<i64>,
    pub style: figure::Style,
    /// The part of the genome the genome-wide view shows.
    pub genome_view: figure::GenomeView,
}

impl State {
    pub fn new(run: Run) -> Self {
        Self {
            run,
            gene: None,
            measure: Measure::Score,
            summary: Summary::Best,
            half_window: None,
            style: figure::Style::default(),
            genome_view: figure::GenomeView::default(),
        }
    }

    /// Show `name`'s locus.
    pub fn set_gene(&mut self, name: &str) -> anyhow::Result<()> {
        self.gene = Some(self.run.gene(name).ok_or_else(|| {
            let like = self.run.genes_like(name, 5);
            if like.is_empty() {
                anyhow::anyhow!("{name}: no links in this run")
            } else {
                anyhow::anyhow!(
                    "{name}: no links in this run (closest: {})",
                    like.join(", ")
                )
            }
        })?);
        Ok(())
    }

    /// The genome-wide view, as it is set.
    #[must_use]
    pub fn genome(&self) -> data::Genome {
        self.run.genome(self.measure, self.summary)
    }

    /// Show only chromosome `chr` of the genome-wide view; `""` all of it.
    pub fn show_chr(&mut self, chr: &str) -> anyhow::Result<()> {
        let (from, span) = if chr.is_empty() {
            (0.0, 1.0)
        } else {
            self.genome()
                .span_of(chr)
                .ok_or_else(|| anyhow::anyhow!("{chr}: no linked gene on it"))?
        };
        self.genome_view.from = from;
        self.genome_view.span = span;
        Ok(())
    }

    /// The locus shown, as it is set.
    #[must_use]
    pub fn locus(&self) -> Option<data::Locus> {
        self.run.locus(self.gene?, self.measure, self.half_window)
    }

    /// The genome-wide figure, or the locus one (none without a placed
    /// gene), as it is set.
    #[must_use]
    pub fn figure(&self, genome: bool) -> Option<Figure> {
        Some(if genome {
            Figure::Genome(self.genome())
        } else {
            Figure::Locus(self.locus()?)
        })
    }

    /// `fig`'s height over its width.
    #[must_use]
    pub fn aspect(&self, fig: &Figure) -> f32 {
        match fig {
            Figure::Genome(_) => figure::GENOME_ASPECT,
            Figure::Locus(l) => figure::aspect(self.style.rows_of(l).len()),
        }
    }

    /// `fig` as a PNG `w` pixels wide.
    pub fn png(&self, fig: &Figure, w: usize) -> anyhow::Result<Vec<u8>> {
        let h = (w as f32 * self.aspect(fig)) as usize;
        figure::png_bytes((w, h), w as f32 / 46.0, |f, font, bold| {
            self.draw(fig, f, font, bold);
        })
    }

    /// Draw `fig` on `frame`, as it is set.
    pub fn draw(&self, fig: &Figure, frame: &mut Frame, font: Font, bold: Font) {
        match fig {
            Figure::Genome(g) => {
                figure::draw_genome(frame, font, bold, g, &self.run.genes, &self.genome_view);
            }
            Figure::Locus(l) => figure::draw_locus(frame, font, bold, l, &self.style),
        }
    }
}

/// A figure's data: the genome-wide view or one gene's locus.
pub enum Figure {
    Genome(data::Genome),
    Locus(data::Locus),
}

/// Run the screen, or with `--pdf` or `--png`, write the figure and exit.
pub fn run_view(args: &ViewArgs) -> anyhow::Result<()> {
    let path = args
        .run
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("give a run: chickpea view <out>"))?;
    let mut state = State::new(Run::open(path)?);
    if let Some(g) = &args.gene {
        state.set_gene(g)?;
    }
    state.measure = Measure::parse(&args.measure)?;
    state.summary = match args.summary.as_str() {
        "count" => Summary::Count(args.count_cutoff),
        _ => Summary::Best,
    };
    if let Some(c) = &args.chr {
        state.show_chr(c)?;
    }
    state.genome_view.focus = state.gene;
    state.half_window = args.half_window;
    state.style = figure::Style {
        cutoff: args.cutoff,
        marks: args.marks.clone(),
        rows: args
            .rows
            .iter()
            .map(|r| state.run.row_of(r))
            .collect::<anyhow::Result<_>>()?,
    };
    if args.genome {
        if let Some(width) = args.text {
            let g = state.genome();
            let text =
                text::genome_to_string(&g, &state.run.genes, &state.genome_view, width.max(40), 24);
            println!("{text}");
            return Ok(());
        }
        if args.pdf.is_some() || args.png.is_some() {
            return export(&state, args);
        }
    }
    if let Some(width) = args.text {
        anyhow::ensure!(
            args.gene.is_some(),
            "--text draws a gene's locus: give --gene"
        );
        let locus = state
            .locus()
            .ok_or_else(|| anyhow::anyhow!("the gene has no place on the genome in this run"))?;
        println!("{}", text::to_string(&locus, &state.style, width.max(40)));
        return Ok(());
    }
    if args.pdf.is_some() || args.png.is_some() {
        anyhow::ensure!(
            args.gene.is_some(),
            "--pdf and --png draw a gene's locus: give --gene"
        );
        return export(&state, args);
    }
    crate::tui::need_terminal(
        "add --text COLUMNS to print the figure, or --png FILE / --pdf FILE to write it",
    )?;
    tui::run(state, &args.session.options(true))
}

/// Write the locus figure `state` shows to `--pdf` and `--png`.
fn export(state: &State, args: &ViewArgs) -> anyhow::Result<()> {
    for p in args.pdf.iter().chain(&args.png) {
        anyhow::ensure!(
            !p.exists(),
            "{} exists: a figure is not written over",
            p.display()
        );
        mkdir_parent(&p.to_string_lossy())?;
    }
    let page = legume_tui::export::PageSize {
        width_in: args.width_in,
        dpi: args.dpi,
    };
    let fig = state
        .figure(args.genome)
        .ok_or_else(|| anyhow::anyhow!("the gene has no place on the genome in this run"))?;
    figure::save(
        args.pdf.as_deref(),
        args.png.as_deref(),
        page,
        state.aspect(&fig),
        |frame, font, bold| state.draw(&fig, frame, font, bold),
    )?;
    for p in args.pdf.iter().chain(&args.png) {
        log::info!("Wrote {}", p.display());
    }
    Ok(())
}
