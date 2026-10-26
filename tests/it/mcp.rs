//! `chickpea mcp` as an agent drives it: JSON-RPC over the child's stdio.

use crate::common::{self, write_fixture, write_gene_coords};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

/// A running `chickpea mcp`, keeping its records under `project`. It runs
/// in a folder of its own, so a relative path that resolved against its
/// working directory rather than the project would land elsewhere.
pub(crate) struct Session {
    _cwd: Option<tempfile::TempDir>,
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next: u64,
}

impl Session {
    pub(crate) fn start(project: &Path) -> Self {
        let cwd = tempfile::tempdir().unwrap();
        let path = cwd.path().to_path_buf();
        Self::start_in(&path, project, Some(cwd))
    }

    /// A server started in `cwd`, with `LEGUME_MCP_PROJECT` set to `project`
    /// as given (relative to `cwd`, maybe).
    /// `keep` is a folder to remove when the session ends.
    fn start_in(cwd: &Path, project: &Path, keep: Option<tempfile::TempDir>) -> Self {
        let mut child = Command::new(env!("CARGO_BIN_EXE_chickpea"))
            .arg("mcp")
            .current_dir(cwd)
            .env("LEGUME_MCP_PROJECT", project)
            .env("LEGUME_MCP_WATCH", "off")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        let mut s = Session {
            _cwd: keep,
            child,
            stdin,
            stdout,
            next: 0,
        };
        let init = s.request(
            "initialize",
            json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "0"},
            }),
        );
        assert_eq!(init["serverInfo"]["name"], "chickpea", "{init}");
        s.send(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
        s
    }

    fn send(&mut self, msg: &Value) {
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// The result of one request, skipping notifications.
    fn request(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next;
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        loop {
            let mut line = String::new();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "server closed"
            );
            let msg: Value = serde_json::from_str(&line).unwrap();
            if msg["id"] == id {
                assert!(msg["error"].is_null(), "{msg}");
                return msg["result"].clone();
            }
        }
    }

    /// A background job's end state, failing the test if it runs on.
    fn wait(&mut self, job: &str) -> Value {
        for _ in 0..20 {
            let st = self
                .call("job_wait", json!({"id": job, "seconds": 30}))
                .unwrap();
            if st["state"] != "running" {
                return st;
            }
        }
        panic!("{job} still running after 10 minutes");
    }

    /// A tool's structured result, or its error text.
    pub(crate) fn call(&mut self, name: &str, args: Value) -> Result<Value, String> {
        let r = self.request("tools/call", json!({"name": name, "arguments": args}));
        if r["isError"] == true {
            Err(r["content"][0]["text"].as_str().unwrap_or_default().into())
        } else {
            Ok(r["structuredContent"].clone())
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// [`common::SMALL`] as `p2g_run`'s flags, with a window to fit the fixture.
fn small() -> Value {
    let mut flags: serde_json::Map<String, Value> = common::SMALL
        .iter()
        .map(|(flag, value)| (flag.to_string(), json!(value)))
        .collect();
    flags.insert("cis-window".into(), json!(10000));
    flags.into()
}

#[test]
fn lists_its_tools() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::start(dir.path());
    let tools = s.request("tools/list", json!({}));
    let names: Vec<&str> = tools["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in [
        "p2g_flags",
        "p2g_run",
        "p2g_links",
        "job_status",
        "job_wait",
    ] {
        assert!(names.contains(&want), "{want} missing from {names:?}");
    }
    let viewer = names.contains(&"view_open");
    assert_eq!(viewer, cfg!(feature = "tui"), "view tools with tui alone");
}

#[test]
fn flags_come_from_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Session::start(dir.path());
    let out = s.call("p2g_flags", json!({})).unwrap();
    let flags = out["flags"].as_array().unwrap();
    let flag = |name: &str| {
        flags
            .iter()
            .find(|f| f["flag"] == name)
            .unwrap_or_else(|| panic!("{name} missing: {out}"))
            .clone()
    };
    assert_eq!(flag("--cis-window")["default"], "500000");
    assert_eq!(flag("--no-qc")["switch"], true);
    let devices = flag("--device")["choices"].clone();
    assert!(
        devices.as_array().unwrap().contains(&json!("cpu")),
        "{devices}"
    );
    // The inputs and the output are the run's own arguments, not flags.
    for named in [
        "--rna",
        "--atac",
        "--gene-coords",
        "--gff-file",
        "--batch",
        "--out",
    ] {
        assert!(flags.iter().all(|f| f["flag"] != named), "{named} listed");
    }
}

#[test]
fn a_bad_run_is_refused_before_it_starts() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let out = dir.path().join("run").to_string_lossy().into_owned();
    let mut s = Session::start(dir.path());
    let base = json!({"rna": rna, "atac": atac, "gene_coords": genes, "out": out});
    let with = |extra: Value| {
        let mut a = base.clone();
        a.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        a
    };

    let err = s
        .call("p2g_run", with(json!({"flags": {"no-such-flag": 1}})))
        .unwrap_err();
    assert!(
        err.contains("--no-such-flag") && err.contains("p2g_flags"),
        "{err}"
    );

    let err = s
        .call("p2g_run", with(json!({"flags": {"out": "elsewhere"}})))
        .unwrap_err();
    assert!(err.contains("--out"), "{err}");

    let err = s
        .call("p2g_run", with(json!({"flags": {"cis-window": "far"}})))
        .unwrap_err();
    // legume-mcp's one line of clap's refusal, under the command as
    // p2g_flags names it, without a pointer to a --help the agent cannot give.
    assert!(
        err.starts_with("chickpea peak-to-gene: ")
            && err.contains("--cis-window")
            && err.contains("far"),
        "{err}"
    );
    assert!(!err.contains('\n') && !err.contains("--help"), "{err}");

    let err = s
        .call("p2g_run", with(json!({"flags": {"no-qc": "yes"}})))
        .unwrap_err();
    assert!(err.contains("--no-qc") && err.contains("switch"), "{err}");

    let mut missing = base.clone();
    missing["rna"] = json!(dir.path().join("no.rna.zarr"));
    let err = s.call("p2g_run", missing).unwrap_err();
    assert!(err.contains("no.rna.zarr"), "{err}");

    let mut none = base.clone();
    none.as_object_mut().unwrap().remove("gene_coords");
    let err = s.call("p2g_run", none).unwrap_err();
    assert!(
        err.contains("gene_coords") && err.contains("gff_file"),
        "{err}"
    );

    assert!(!std::path::Path::new(&format!("{out}.links.parquet")).exists());
}

#[test]
fn a_run_writes_links_the_server_then_reads() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let out = dir.path().join("run").to_string_lossy().into_owned();
    let mut s = Session::start(dir.path());
    // A hidden flag that takes `=value`, and an output named relative to
    // the project.
    let mut flags = small();
    flags["qc-mad-on-genes"] = json!(false);
    flags["qc-report"] = json!("qc.tsv");
    let args = json!({
        "rna": rna, "atac": atac, "gene_coords": genes, "out": out, "flags": flags,
    });

    // In the background by default: a job to wait on.
    let started = s.call("p2g_run", args.clone()).unwrap();
    let job = started["job"].as_str().expect("a job id").to_string();
    let command = started["command"].as_str().unwrap();
    assert!(command.starts_with("chickpea peak-to-gene "), "{command}");
    assert!(command.contains("--qc-mad-on-genes=false"), "{command}");
    let state = s.wait(&job);
    assert_eq!(state["state"], "done", "{state}");
    let report = dir.path().join("qc.tsv");
    assert!(report.exists(), "the QC report under the project");
    let report = report.canonicalize().unwrap();
    let writes = started["writes"].as_array().unwrap();
    assert!(
        writes
            .iter()
            .any(|w| Path::new(w.as_str().unwrap()).canonicalize().ok() == Some(report.clone())),
        "{started}"
    );

    // The tables the run wrote are the ones the server guards.
    let mut written: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| {
            let name = e.unwrap().file_name().to_string_lossy().into_owned();
            Some(
                name.strip_prefix("run.")?
                    .strip_suffix(".parquet")?
                    .to_string(),
            )
        })
        .collect();
    written.sort();
    let mut stems = chickpea::p2g::workflow::OUTPUT_STEMS
        .map(String::from)
        .to_vec();
    stems.sort();
    assert_eq!(written, stems);

    // A run is never written over.
    let err = s.call("p2g_run", args.clone()).unwrap_err();
    assert!(err.contains("exists"), "{err}");
    // Nor is an output a flag names.
    let mut other = args;
    other["out"] = json!(dir.path().join("other"));
    let err = s.call("p2g_run", other).unwrap_err();
    assert!(err.contains("qc.tsv") && err.contains("exists"), "{err}");

    let top = s.call("p2g_links", json!({"out": out, "top": 5})).unwrap();
    let rows = top["links"].as_array().unwrap();
    assert!(!rows.is_empty() && rows.len() <= 5, "{top}");
    assert!(top["pairs"].as_u64().unwrap() >= rows.len() as u64);
    let scores: Vec<f64> = rows.iter().map(|r| r["score"].as_f64().unwrap()).collect();
    assert!(scores.windows(2).all(|w| w[0] >= w[1]), "{scores:?}");
    for col in ["gene", "peak", "distance", "abc", "gate", "corr", "score"] {
        assert!(!rows[0][col].is_null(), "{col} missing: {}", rows[0]);
    }

    let gene = rows[0]["gene"].as_str().unwrap().to_string();
    let one = s
        .call(
            "p2g_links",
            json!({"out": out, "genes": [gene], "top": 100}),
        )
        .unwrap();
    let rows = one["links"].as_array().unwrap();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r["gene"] == gene.as_str()), "{one}");

    let err = s
        .call("p2g_links", json!({"out": dir.path().join("nothing")}))
        .unwrap_err();
    assert!(err.contains("links.parquet"), "{err}");
}

#[test]
fn runs_without_an_out_each_get_their_own() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let mut s = Session::start(dir.path());
    let args = json!({
        "rna": rna, "atac": atac, "gene_coords": genes, "flags": small(), "background": false,
    });
    let links = |r: &Value| -> String {
        let written = r["written"].as_array().unwrap();
        let f = written.iter().find_map(|p| {
            let p = p.as_str()?;
            p.ends_with(".links.parquet").then(|| p.to_string())
        });
        f.unwrap_or_else(|| panic!("no links in {r}"))
    };
    let first = links(&s.call("p2g_run", args.clone()).unwrap());
    let second = links(&s.call("p2g_run", args).unwrap());
    assert_ne!(first, second);
    for f in [&first, &second] {
        assert!(Path::new(f).starts_with(dir.path().join("results")), "{f}");
        assert!(Path::new(f).exists(), "{f}");
    }
}

#[test]
fn links_rank_best_first_with_undefined_scores_last() {
    let scores = [0.2, f64::NAN, 0.9, -0.5, -f64::NAN];
    let mut rows: Vec<usize> = (0..scores.len()).collect();
    chickpea::mcp::best_first(&scores, &mut rows, 10);
    assert_eq!(&rows[..3], &[2, 0, 3]);
    assert!(rows[3..].iter().all(|&i| scores[i].is_nan()));

    let mut rows: Vec<usize> = (0..scores.len()).collect();
    chickpea::mcp::best_first(&scores, &mut rows, 2);
    assert_eq!(rows, [2, 0]);
}

#[test]
fn background_runs_without_an_out_do_not_collide() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let mut s = Session::start(dir.path());
    // A null flag is left out, not passed as "null".
    let mut flags = small();
    flags["n-clusters"] = Value::Null;
    let args = json!({"rna": rna, "atac": atac, "gene_coords": genes, "flags": flags});
    let first = s.call("p2g_run", args.clone()).unwrap();
    let second = s.call("p2g_run", args).unwrap();
    assert!(
        !first["command"].as_str().unwrap().contains("n-clusters"),
        "{first}"
    );
    assert_ne!(first["writes"], second["writes"]);
    for started in [&first, &second] {
        let state = s.wait(started["job"].as_str().unwrap());
        assert_eq!(state["state"], "done", "{state}");
    }
}

#[test]
fn a_relative_project_is_taken_from_where_the_server_starts() {
    let dir = tempfile::tempdir().unwrap();
    let project = dir.path().join("proj");
    std::fs::create_dir(&project).unwrap();
    write_fixture(&project);
    let genes = write_gene_coords(&project);
    let mut s = Session::start_in(dir.path(), Path::new("proj"), None);
    let args = json!({
        "rna": "rna.zarr", "atac": "atac.zarr", "gene_coords": genes, "flags": small(),
        "background": false,
    });
    let ran = s.call("p2g_run", args).unwrap();
    let results = project.join("results").canonicalize().unwrap();
    for f in ran["written"].as_array().unwrap() {
        let f = Path::new(f.as_str().unwrap()).canonicalize().unwrap();
        assert!(f.starts_with(&results), "{}", f.display());
    }
    assert!(!project.join("proj").exists());
}

#[test]
fn without_qc_no_report_is_expected() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let genes = write_gene_coords(dir.path());
    let out = dir.path().join("run");
    let mut s = Session::start(dir.path());
    let mut flags = small();
    flags["no-qc"] = json!(true);
    flags["qc-report"] = json!("qc.tsv");
    let args = json!({
        "rna": rna, "atac": atac, "gene_coords": genes, "out": out, "flags": flags,
        "background": false,
    });
    let ran = s.call("p2g_run", args).unwrap();
    let written = ran["written"].as_array().unwrap();
    assert!(
        !written
            .iter()
            .any(|w| w.as_str().unwrap().ends_with("qc.tsv")),
        "{ran}"
    );
}

#[test]
fn an_input_spelled_as_a_flag_is_pointed_to_its_argument() {
    let dir = tempfile::tempdir().unwrap();
    let (rna, atac) = write_fixture(dir.path());
    let mut s = Session::start(dir.path());
    let args = json!({"rna": rna, "atac": atac, "flags": {"gene_coords": "g.tsv"}});
    let err = s.call("p2g_run", args).unwrap_err();
    assert!(err.contains("as `gene_coords`, not as a flag"), "{err}");
}
