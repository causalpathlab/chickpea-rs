# Chickpea

**CH**romatin **I**nteractions **C**aptured by **K**nitting **P**eaks with
**E**xpression **A**nchors.

Peak-to-gene cis-regulatory linkage from paired single-cell RNA + ATAC.

RNA genes and ATAC peaks share one multiome feature axis, embedded like
`senna bge --multiome`: a wide ATAC modality is module-only, so each peak's row
is its module's row. Inside the phase-1 fit, each gene's score mixes in the
predicted accessibility of its cis peaks, pooled through ReLU gates on
distance, and an alignment term pulls the gene's own profile toward it, so
genes and peaks share one feature space.
Each link is scored by its gate times the data's evidence. The design is in
[`docs/design.md`](docs/design.md).

## Model

```text
λ_u,m = ⟨e_u, μ_m⟩ + b_m + β_u,k(m)                       # module score, β per-unit modality intercept
ρ_ug  = ⟨e_u, μ_{m(g)} + r_g⟩ + b_g                       # the gene's own score
w_gp  = abc_gp · max(0, θ₀ + θ₁ z(log contact_gp))        # the gate: a distance prior
w̃_gp  = w_gp / Σ_q w_gq                                   # the gene's share of each pair
ã_ug  = Σ_p w̃_gp λ_u,m(p)                                 # ATAC-guided gene activity
η_ug  = (1 − α) ρ_ug + α ã_ug                             # the gene's likelihood score, α = --mix
gap   = mean_g var_u(ρ_ug − ã_ug)                         # added to phase 1 × --align-weight

corr_gp  = corr_u(⟨e_u, μ_{m(p)}⟩, ⟨e_u, ρ_g⟩)             # after training: the evidence
score_gp = w̃_gp · corr_gp                                 # the link score
```

`θ` is shared by every gene. The mixture routes part of each cis gene's
likelihood through its peaks, and the written gene rows carry it; the
alignment moves gene rows, peak-module rows and the gates, never the units.
The two need each other: the mixture alone leaves the peaks apart from the
genes, the alignment alone squeezes the cell embedding. `w` is unit-free, and cell context comes in through
`e_u`. `β` keeps each unit's ATAC:RNA count split out of the embedding. The
variance and the correlation run across pseudobulk units.

## Pipeline

`chickpea peak-to-gene` (aliases `p2g`, `peak2gene`):

1. **Cis candidates.** Peaks whose midpoint lies within `--cis-window` of a
   gene's TSS, at most `--max-cis` per gene, nearest first. Each pair carries
   the ABC contact `(d + c)^-γ`, normalised over the gene's candidates. A peak
   near several genes is a candidate for each.
2. **Multiome embedding.** RNA genes and ATAC peaks on one axis, trained with
   the shared two-phase engine: an exact two-level softmax over multilevel
   pseudobulks, then one cell encoder over the joint axis. Modules are
   modality-pure, with a budget per modality (`--feature-modules` for RNA,
   `--peak-modules` for ATAC). Flat features (one rate explains their counts)
   form each modality's background and go module-only; on RNA, singleton
   genes join them. ATAC modules are never set aside by size, so a single
   active peak keeps its own module, and they never cross a 10 Mb genomic
   window.
3. **Gates.** After the partition, pairs of module-only genes and pairs to the
   ATAC background are dropped. The gate scalars train inside phase 1,
   through the alignment term. Afterwards each pair is scored: its gate times
   the correlation across pseudobulks of the peak module's score and the
   gene's.
4. **Per-cluster links.** Cells are clustered (Leiden), and each gene's gate
   shares are re-weighted by each cluster's peak accessibility. Fixed ABC shares
   are reported alongside as the baseline.

Cell QC (on by default, driven by the RNA counts) leaves failed cells in the
embedding but out of clustering, accessibility rates and every cell table.

## Usage

```bash
chickpea peak-to-gene \
  --rna sim.rna.zarr \
  --atac sim.atac.zarr \
  --gene-coords sim.gene_coords.tsv.gz \
  -o out
```

Gene TSS positions come from `--gene-coords` (a `gene<TAB>chr<TAB>tss` TSV with
a header) or `--gff-file` (GFF/GTF); one is required.

Key options (see `chickpea peak-to-gene --help` for all):

| Flag | Default | Meaning |
|------|---------|---------|
| `--rna`, `--atac` | — | paired matrices (zarr/h5) for the same barcodes, one file each per sample (paired by position); with several samples a cell is `{barcode}@{sample}` and each sample is a batch |
| `--data` | — | the count files in any order, in place of `--rna`/`--atac`: each told RNA, ATAC or both (genes and peaks as rows of one file, a 10x multiome matrix) by its rows, and RNA and ATAC files paired by the words of their names |
| `--batch` | — | batch labels, one per barcode |
| `--cis-window` | 500000 | max peak-midpoint distance (bp) to a TSS |
| `--max-cis` | 200 | cap on candidate peaks per gene (nearest) |
| `--contact-gamma`, `--contact-pseudocount` | 1, 5000 | ABC contact `(d + c)^-γ` |
| `--mix` | 0.5 | share of each gene's score taken from its cis peaks (0 = exact likelihood) |
| `--align-weight` | 0.1 | weight of the gene-to-cis-peak alignment per unit (0 = off) |
| `--weight-decay` | 1e-4 | L2 decay on the gene and peak embedding rows, never the pseudobulks (0 = off) |
| `--embedding-dim` | 128 | embedding dimension |
| `--epochs` | 1000 | embedding epochs |
| `--feature-modules` | 1024 | RNA gene modules |
| `--peak-modules` | 10000 | ATAC peak modules (each peak's row is its module's) |
| `--module-only-min-rows` | 100000 | ATAC goes module-only at this many peaks (0 = off) |
| `--device`, `--device-no` | cpu, 0 | compute device (`cpu`, `cuda`, `metal`) |
| `--n-clusters` | Leiden | target number of cell clusters |
| `--groups` | Leiden | cell groups for the per-group tables: a lupin round (`.json`), its argmax table, or a cell, group table |
| `-o`, `--out` | — | output prefix |

Outputs, all `{out}.*.parquet`, and the run's record `{out}.run.json`:

| File | Contents |
|------|----------|
| `links` | one row per kept cis pair: `gene`, `peak`, `distance`, `abc`, `gate` (the trained `w`; `0` for a closed pair), `corr` (the evidence), `score` (`gate × corr`; rank links by this), `gene_idx`, `peak_idx` |
| `links_by_cluster` | `gene_idx`, `peak_idx`, `cluster`, `gate`, `abc`: shares within each cell group (indices into the RNA / peak axes) |
| `groups`, `peak_rates` | each group's `cluster` number, name and cells; each peak's accessibility in all cells and in each group |
| `genes` | `gene`, `chromosome`, `tss` |
| `gene_embedding` | RNA gene rows |
| `peak_embedding`, `peaks` | each peak's module row; `peak`, `chromosome`, `start`, `end` |
| `cell_embedding`, `cell_clusters` | per-cell rows; `cell`, `cluster`, `group` |

### Regroup, run and view

`chickpea regroup --run out/p2g --groups lupin/X.L2.json -o out/p2g.L2`
redoes a finished run's per-group tables for other cell groups (a lupin
round's cell types, say) without training again, and writes
`{out}.regroup.json` naming the run whose links it uses. It reads each
sample's ATAC from the run's record, `{out}.run.json` (the samples' files,
groups and flags, relative to the record and in full), or for a regrouped
run, from its `{out}.regroup.json`, which keeps the samples as it read
them; `--atac` gives a file that has moved, matched to its sample by name.

`chickpea run` sets up a step on a terminal screen and runs it with its log
on screen: the count files on a data pane, a row per sample, picked in a file
browser (space marks several; each is told RNA, ATAC or both and paired by
name), and the other flags as a form, with the mouse as well as the keys.
`chickpea view out/p2g` ranks genes by their links, genome-wide, and draws a
gene's locus (arcs over each group's accessibility); `--pdf`, `--png` and
`--text` write either figure without the screen. Both run as detachable
sessions (`chickpea attach`, `sessions`, `kill`, `rename`).

### Agents (MCP)

`chickpea mcp` serves chickpea's tools to an agent over the Model Context
Protocol on stdio:

| Tool | What it does |
|------|--------------|
| `p2g_flags` | peak-to-gene's flags, read from the command line (help, default, choices) |
| `p2g_run` | runs peak-to-gene as a child `chickpea`, checked first, as a background job by default (follow it with `job_status` / `job_wait`); never writes over a run |
| `p2g_links` | a run's links, highest `score` first, for all genes or the ones named |
| `p2g_regroup` | a finished run's per-group tables for other cell groups, as `chickpea regroup` |
| `view_open` | opens a run to look at: its genes, peaks and cell groups |
| `view_genome` | the genes ranked genome-wide, as a figure (PNG or text) and a table |
| `view_locus` | one gene's locus, as a figure (PNG or text) and its candidates |
| `view_export` | the locus or genome-wide figure as PDF and/or PNG |

Outputs the agent does not name go to `results/<today>/` under the project (the
working directory, or `LEGUME_MCP_PROJECT`), beside a driver script that
repeats the work and a lab notebook.

```bash
claude mcp add chickpea -e LEGUME_MCP_PROJECT="$PWD" -- chickpea mcp
```

### Simulation

Paired ATAC + RNA with ground-truth peak-gene links lives in
`data-beans-sim multiome`:

```bash
data-beans-sim multiome \
  --out ./results/sim \
  --n-genes 2000 --n-peaks 10000 --n-cells 5000 \
  --n-topics 10 \
  --linked-gene-fraction 0.3 --n-causal-per-gene 3 \
  --depth-rna 5000 --depth-atac 2000 \
  --rseed 42
```

It writes `{out}.rna.zarr`, `{out}.atac.zarr`, and `{out}.gene_coords.tsv.gz`
(the `--gene-coords` input above). Optional `--reference-rna`/`--reference-atac`
switch a modality to NB+copula sampling fitted from a real reference.

## Installation

```bash
cargo install chickpea-rs
# or from this repo:
cargo install --path .
```

Binary name remains `chickpea`. GPU / HDF5:

```bash
cargo install chickpea-rs --features metal   # macOS
cargo install chickpea-rs --features cuda    # Linux with CUDA
cargo install chickpea-rs --features hdf5
```

The terminal screens and their sessions (`run`, `view`, `attach`,
`sessions`, `kill`, `rename`) and the `view_*` MCP tools are the default
`tui` feature; `--no-default-features` builds without them (`chickpea mcp`
then serves the `p2g_*` tools alone, and a `p2g_run` job has no screen to
watch).
