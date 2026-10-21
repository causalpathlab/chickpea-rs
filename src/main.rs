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
                      pseudobulks of the peak module's score and the gene's.\n\
                      \n\
                      {out}.links.parquet has gene, peak, distance, abc, gate,\n\
                      corr and score (gate × corr).\n\
                      {out}.links_by_cluster.parquet has gene_idx/peak_idx/cluster shares.\n\
                      Gene, peak and cell embeddings and cell clusters are written too.",
        after_long_help = ENV_HELP,
        aliases = ["p2g", "peak2gene"]
    )]
    PeakToGene(Box<p2g::PeakToGeneArgs>),

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
}

fn main() -> anyhow::Result<()> {
    if std::env::args().any(|arg| arg == "--help" || arg == "-h") {
        print_logo();
    }

    let cli = Cli::parse();

    // The shared logger routes log lines above the progress bars (one
    // MultiProgress for the whole workspace), so they do not corrupt each other.
    data_beans::aux::logging::init_logger(cli.verbose);

    match &cli.commands {
        Commands::PeakToGene(args) => p2g::run_peak_to_gene(args),
        Commands::Mcp => {
            // The project's folder, so the paths an agent gives are read
            // from the project, as the records are.
            let root = std::env::var_os(legume_mcp::project::ROOT_ENV).filter(|r| !r.is_empty());
            if let Some(root) = root {
                let root = std::path::absolute(&root)?;
                std::env::set_current_dir(&root)
                    .map_err(|e| anyhow::anyhow!("project {}: {e}", root.display()))?;
                // Absolute, so the records find the project from inside it.
                std::env::set_var(legume_mcp::project::ROOT_ENV, &root);
            }
            legume_mcp::serve(
                legume_mcp::ServerInfo {
                    name: mcp::SERVER.into(),
                    version: env!("CARGO_PKG_VERSION").into(),
                    instructions: Some(mcp::INSTRUCTIONS.into()),
                },
                mcp::Server::default(),
            )
        }
    }
}
