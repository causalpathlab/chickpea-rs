use chickpea::common::*;
use chickpea::{mcp, p2g};
use colored::Colorize;

const LOGO: &str = include_str!("../logo.txt");
const ENV_HELP: &str = "Environment variables:\n  RUST_LOG=info  Enable logging to stderr";

fn colorize_logo_line(line: &str) -> String {
    line.replace(':', &":".truecolor(210, 180, 120).to_string())
        .replace('*', &"*".truecolor(240, 210, 140).to_string())
        .replace('o', &"o".truecolor(160, 120, 60).to_string())
        .replace('.', &".".truecolor(180, 160, 110).to_string())
        .replace('^', &"^".truecolor(180, 160, 110).to_string())
        .replace('\'', &"'".truecolor(180, 160, 110).to_string())
        .replace('/', &"/".truecolor(139, 90, 43).to_string())
        .replace('\\', &"\\".truecolor(139, 90, 43).to_string())
        .replace('_', &"_".truecolor(139, 90, 43).to_string())
        .replace('=', &"=".green().to_string())
        .replace('>', &">".bright_green().to_string())
        .replace('-', &"-".green().to_string())
        .replace('|', &"|".green().to_string())
        .replace('▀', &"▀".truecolor(100, 180, 100).to_string())
}

fn print_logo() {
    for line in LOGO.lines() {
        println!("  {}", colorize_logo_line(line));
    }
    println!(" {}", "Multi-Omic Linkage Analysis".bold());
    println!();
}

#[derive(Parser, Debug)]
#[command(
    version,
    about = "chickpea —\n\
             peak-to-gene cis-regulatory linkage for paired single-cell RNA + ATAC",
    long_about = "chickpea — peak-to-gene cis-regulatory linkage\n\
                  \n\
                  Links ATAC peaks to RNA genes in paired single-cell RNA + ATAC data.\n\
                  Peaks become gene features: each gene gets an ATAC track,\n\
                  embedded next to its RNA track against shared pseudobulks.\n\
                  Each gene then attends over its cis peaks, and the shares are the links.\n\
                  \n\
                  Usage:\n\
                  data-beans-sim multiome -o sim && chickpea peak-to-gene --rna sim.rna.zarr \\\n\
                  --atac sim.atac.zarr --gene-coords sim.gene_coords.tsv.gz -o out",
    term_width = 80
)]
struct Cli {
    #[arg(
        short = 'v',
        long,
        global = true,
        help = "Enable verbose logging",
        long_help = "Enable verbose logging to stderr. Equivalent to setting RUST_LOG=info."
    )]
    verbose: bool,

    #[command(subcommand)]
    commands: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Link cis peaks to genes over a multiome embedding (bge-style)
    #[command(
        long_about = "Link ATAC peaks to RNA genes.\n\
                      \n\
                      RNA genes and ATAC peaks are embedded on one multiome axis\n\
                      (ATAC module-only, like senna bge --multiome). Each gene's\n\
                      score mixes in the accessibility of its cis peaks, pooled\n\
                      through ReLU gates on distance (--mix), and its profile is\n\
                      pulled toward it (--align-weight), so genes and peaks share\n\
                      one feature space.\n\
                      A link's score is its gate times the correlation across\n\
                      pseudobulks of the peak module's score and the gene's (so\n\
                      peaks of one ATAC module share a corr; --peak-modules sets\n\
                      how finely).\n\
                      \n\
                      {out}.links.parquet has gene, peak, distance, abc, gate,\n\
                      corr and score (gate × corr).\n\
                      {out}.links_by_cluster.parquet has gene_idx/peak_idx/cluster shares.\n\
                      Gene, peak and cell embeddings and cell clusters are written too.",
        after_long_help = ENV_HELP,
        aliases = ["p2g", "peak2gene"]
    )]
    PeakToGene(Box<p2g::PeakToGeneArgs>),

    /// A finished run's per-group tables again, for other cell groups
    #[command(
        long_about = "A finished run's per-group tables again, for other cell groups.\n\
                      \n\
                      A group's link shares need only the trained gates (in\n\
                      {run}.links.parquet) and the peaks' accessibility in its cells,\n\
                      so the run is not trained again: a lupin round's cell types\n\
                      take the place of the Leiden clusters in minutes.\n\
                      \n\
                      Writes {out}.groups, cell_clusters, peak_rates and\n\
                      links_by_cluster, and {out}.regroup.json, which names the run\n\
                      whose links it uses, so {out} opens as a run of its own\n\
                      (chickpea view {out}) without copying them.\n\
                      \n\
                      Example:\n\
                      \x20 chickpea regroup --run out/p2g --atac atac.zarr \\\n\
                      \x20     --groups lupin/X.L2.json -o out/p2g.L2",
        after_long_help = ENV_HELP
    )]
    Regroup(p2g::RegroupArgs),

    /// Server for agents (Model Context Protocol on stdio)
    #[command(
        long_about = "Serve chickpea's tools to an agent over the Model Context Protocol.\n\
                      \n\
                      stdin and stdout carry the protocol; logs go to stderr. The tools\n\
                      list peak-to-gene's flags, run it in the background, and read the\n\
                      links a run wrote. Outputs, a driver script and a lab notebook are\n\
                      kept under the project (the working directory, or\n\
                      LEGUME_MCP_PROJECT).\n\
                      \n\
                      Any MCP client runs it as the command `chickpea mcp`:\n\
                      \x20 claude mcp add chickpea -e LEGUME_MCP_PROJECT=\"$PWD\" -- chickpea mcp\n\
                      \x20 codex mcp add chickpea -- chickpea mcp"
    )]
    Mcp,

    /// Set up a step in the terminal and run it
    #[cfg(feature = "tui")]
    #[command(long_about = format!(
        "Set up a chickpea step in the terminal and run it.\n\n\
         Every flag of the step is a row of the form, with its help; enter\n\
         changes the row under the cursor, d puts it back to its default.\n\
         The command line the form stands for is shown at the foot, checked\n\
         as chickpea parses it. ctrl+r runs it, with its log and progress on\n\
         screen; a run is never written over an existing one. x stops it.\n\n\
         A step given after `--` runs at once:\n\
         \x20 chickpea run -- peak-to-gene --rna r.zarr --atac a.zarr \\\n\
         \x20     --gene-coords g.tsv -o out/p2g\n\
         This is how `p2g_run` on `chickpea mcp` runs in the background.\n\n\
         {}",
        legume_tui::cli::session_help(
            &chickpea::tui::TOOL,
            legume_tui::cli::SessionHelp::new()
                .what_goes_on("the run goes on")
                .resume_hint(Some("`chickpea run` with no step"))
                .more("The keys in effect are listed at the foot of the screen.")
        )
    ))]
    Run(chickpea::run::RunArgs),

    /// Look at a run's links, genome-wide and at a gene's locus
    #[cfg(feature = "tui")]
    #[command(long_about = format!(
        "Look at a finished peak-to-gene run's links.\n\n\
         Genes are ranked by their links, one number each (their best link,\n\
         or how many reach a cutoff), by score, gate, corr or abc; the gene\n\
         under the cursor shows its cis candidates.\n\n\
         Examples:\n\
         \x20 chickpea view out/p2g\n\
         \x20 chickpea view out/p2g.links.parquet --gene GATA1\n\n\
         {}",
        legume_tui::cli::session_help(
            &chickpea::tui::TOOL,
            legume_tui::cli::SessionHelp::new()
                .more("The keys in effect are listed at the foot of the screen.")
        )
    ))]
    View(Box<chickpea::view::ViewArgs>),

    #[cfg(feature = "tui")]
    #[command(
        about = "Return to a detached `chickpea run` or `chickpea view`",
        long_about = legume_tui::cli::about(&chickpea::tui::TOOL, "q in it").attach
    )]
    Attach(legume_tui::cli::AttachArgs),

    #[cfg(feature = "tui")]
    #[command(
        about = "List the live `chickpea run` and `chickpea view` sessions",
        long_about = legume_tui::cli::about(&chickpea::tui::TOOL, "q in it").sessions
    )]
    Sessions,

    #[cfg(feature = "tui")]
    #[command(
        about = "Stop a `chickpea run` or `chickpea view` session",
        long_about = format!(
            "{}\n\n`chickpea run` stops the run it is running first.",
            legume_tui::cli::about(&chickpea::tui::TOOL, "q in it").kill
        )
    )]
    Kill(legume_tui::cli::KillArgs),

    #[cfg(feature = "tui")]
    #[command(
        about = "Give a `chickpea run` or `chickpea view` session a label",
        long_about = legume_tui::cli::about(&chickpea::tui::TOOL, "q in it").rename
    )]
    Rename(legume_tui::cli::RenameArgs),
}

fn main() -> anyhow::Result<()> {
    // A session's host runs this binary again: it serves the session and
    // exits here, before any of chickpea's own work.
    #[cfg(feature = "tui")]
    legume_tui::serve_if_host();

    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        print_logo();
    }

    let cli = Cli::parse();

    // The shared logger routes log lines above the progress bars (one
    // MultiProgress for the whole workspace), so they do not corrupt each other.
    data_beans::aux::logging::init_logger(cli.verbose);

    match &cli.commands {
        Commands::PeakToGene(args) => p2g::run_peak_to_gene(args),
        Commands::Regroup(args) => p2g::run_regroup(args),
        Commands::Mcp => legume_mcp::serve_in_project(
            legume_mcp::ServerInfo {
                name: mcp::SERVER.into(),
                version: env!("CARGO_PKG_VERSION").into(),
                instructions: Some(mcp::INSTRUCTIONS.into()),
            },
            mcp::Server::default(),
        ),
        #[cfg(feature = "tui")]
        Commands::Run(args) => chickpea::run::app::run(args),
        #[cfg(feature = "tui")]
        Commands::View(args) => chickpea::view::run_view(args),
        #[cfg(feature = "tui")]
        Commands::Attach(args) => Ok(legume_tui::cli::attach(&chickpea::tui::TOOL, args)?),
        #[cfg(feature = "tui")]
        Commands::Sessions => Ok(legume_tui::cli::sessions(&chickpea::tui::TOOL)?),
        #[cfg(feature = "tui")]
        Commands::Kill(args) => Ok(legume_tui::cli::kill(&chickpea::tui::TOOL, args)?),
        #[cfg(feature = "tui")]
        Commands::Rename(args) => Ok(legume_tui::cli::rename(&chickpea::tui::TOOL, args)?),
    }
}
