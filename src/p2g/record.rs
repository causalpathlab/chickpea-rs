//! `{out}.run.json`: what a peak-to-gene run read, so later steps find it
//! without being told again. Each sample's name and files (relative to the
//! record's folder, and in full for when the folder has moved without
//! them), the groups and the flags.
//!
//! `regroup` reads each sample's ATAC from it; an `--atac` given there
//! stands in for the recorded file of the sample its name names.
//!
//! A recorded file is looked for where it was first, and only then near the
//! record: a record copied into another tree must not quietly read that
//! tree's data while its own is still there.

use crate::p2g::samples::{words, Sample};
use crate::p2g::workflow::tidy;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// What peak-to-gene writes beside its tables.
pub const RUN_JSON: &str = "run.json";

/// Where `prefix`'s run record is.
#[must_use]
pub fn run_record(prefix: &str) -> String {
    format!("{prefix}.{RUN_JSON}")
}

/// The folder a record at `record` is in (`.` for a bare name).
#[must_use]
pub fn folder(record: &str) -> &Path {
    match Path::new(record).parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}

/// `path` absolute, its `.` and `..` worked out.
#[must_use]
pub fn full(path: &Path) -> PathBuf {
    tidy(&std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// `path` relative to the folder `dir`.
#[must_use]
pub fn relative_to(path: &str, dir: &Path) -> String {
    let (p, d) = (full(Path::new(path)), full(dir));
    let (p, d): (Vec<_>, Vec<_>) = (p.components().collect(), d.components().collect());
    let same = p.iter().zip(&d).take_while(|(a, b)| a == b).count();
    let mut out = PathBuf::new();
    for _ in same..d.len() {
        out.push("..");
    }
    for c in &p[same..] {
        out.push(c);
    }
    out.to_string_lossy().into_owned()
}

/// `path` as a record in `dir` keeps it: relative to `dir`, and in full.
#[must_use]
pub fn recorded(path: &str, dir: &Path) -> (String, String) {
    (
        relative_to(path, dir),
        full(Path::new(path)).to_string_lossy().into_owned(),
    )
}

/// A path a record in `dir` keeps as `rel` and in full as `abs`: `abs`
/// where `found` takes it, else `rel` from the record.
#[must_use]
pub fn located(dir: &Path, rel: &str, abs: Option<&str>, found: impl Fn(&str) -> bool) -> String {
    match abs {
        Some(abs) if found(abs) => abs.to_string(),
        _ => tidy(&dir.join(rel)).to_string_lossy().into_owned(),
    }
}

/// The JSON at `path`.
pub fn read_json(path: &str) -> anyhow::Result<Value> {
    let text = std::fs::read_to_string(path).map_err(|e| anyhow::anyhow!("reading {path}: {e}"))?;
    serde_json::from_str(&text).map_err(|e| anyhow::anyhow!("{path} is not JSON: {e}"))
}

/// Write `v` to `path`, pretty.
pub fn write_json(path: &str, v: &Value) -> anyhow::Result<()> {
    std::fs::write(path, serde_json::to_string_pretty(v)? + "\n")
        .map_err(|e| anyhow::anyhow!("writing {path}: {e}"))
}

/// Write `out`'s run record: `samples`, `groups` and `flags`.
pub fn write_run_record(
    out: &str,
    samples: &[Sample],
    groups: Option<&str>,
    flags: Value,
) -> anyhow::Result<()> {
    let record = run_record(out);
    let dir = folder(&record);
    let file = |p: &str| recorded(p, dir);
    let samples: Vec<Value> = samples
        .iter()
        .map(|s| {
            let ((rna, rna_abs), (atac, atac_abs)) = (file(&s.rna), file(&s.atac));
            json!({
                "name": s.name.as_deref(),
                "rna": rna, "rna_abs": rna_abs,
                "atac": atac, "atac_abs": atac_abs,
            })
        })
        .collect();
    let groups = groups.map(file);
    let v = json!({
        "samples": samples,
        "groups": groups.as_ref().map(|g| &g.0),
        "groups_abs": groups.as_ref().map(|g| &g.1),
        "flags": flags,
    });
    write_json(&record, &v)
}

/// The samples `prefix`'s run record names, their files found; `None` for
/// a run written before records.
pub fn read_samples(prefix: &str) -> anyhow::Result<Option<Vec<Sample>>> {
    let record = run_record(prefix);
    if !Path::new(&record).exists() {
        return Ok(None);
    }
    let v = read_json(&record)?;
    let dir = folder(&record);
    let list = v["samples"]
        .as_array()
        .filter(|l| !l.is_empty())
        .ok_or_else(|| anyhow::anyhow!("{record} names no samples"))?;
    let samples = list
        .iter()
        .map(|s| {
            let at = |side: &str| -> anyhow::Result<Box<str>> {
                let rel = s[side]
                    .as_str()
                    .ok_or_else(|| anyhow::anyhow!("{record}: a sample without its {side}"))?;
                let abs = s[format!("{side}_abs")].as_str();
                Ok(located(dir, rel, abs, |p| Path::new(p).exists()).into())
            };
            Ok(Sample {
                name: s["name"].as_str().map(Into::into),
                rna: at("rna")?,
                atac: at("atac")?,
            })
        })
        .collect::<anyhow::Result<_>>()?;
    Ok(Some(samples))
}

/// `samples` with each of `atac` in place of the ATAC of the sample it
/// names: the sample with the most words of its name among the file's
/// words, else the one whose recorded file has its file name; for a run of
/// one sample, that sample's. When no file names any sample and every
/// sample's is given, they are taken in the samples' order.
pub fn with_atac(mut samples: Vec<Sample>, atac: &[Box<str>]) -> anyhow::Result<Vec<Sample>> {
    let of: Vec<Vec<usize>> = atac.iter().map(|f| named(&samples, f)).collect();
    let in_order = atac.len() == samples.len() && of.iter().all(Vec::is_empty);
    if in_order {
        log::warn!("No --atac file names a sample: taking them in the samples' order");
    }
    let mut given: Vec<Option<&str>> = vec![None; samples.len()];
    for (i, (f, of)) in atac.iter().zip(of).enumerate() {
        let k = match of[..] {
            _ if in_order => i,
            [k] => k,
            _ => {
                let names: Vec<&str> = samples.iter().filter_map(|s| s.name.as_deref()).collect();
                anyhow::bail!(
                    "{f} names {} of the run's samples ({}): an --atac file's name has its \
                     sample's name, or is the recorded file's",
                    if of.is_empty() { "none" } else { "several" },
                    names.join(", ")
                );
            }
        };
        if let Some(was) = given[k] {
            anyhow::bail!("{was} and {f} are both the ATAC of one sample: give one");
        }
        given[k] = Some(f);
    }
    for (s, f) in samples.iter_mut().zip(given) {
        if let Some(f) = f {
            s.atac = f.into();
        }
    }
    Ok(samples)
}

/// The samples file `f` names, by [`with_atac`]'s rule: one for a match.
fn named(samples: &[Sample], f: &str) -> Vec<usize> {
    let mine: Vec<String> = words(f).iter().map(|w| w.to_lowercase()).collect();
    // How many of each sample's name words the file has, all or none.
    let scores: Vec<Option<usize>> = samples
        .iter()
        .map(|s| {
            let Some(n) = &s.name else { return Some(0) };
            let theirs: Vec<String> = n.split('_').map(str::to_lowercase).collect();
            theirs
                .iter()
                .all(|w| mine.contains(w))
                .then_some(theirs.len())
        })
        .collect();
    let base = Path::new(f).file_name();
    match scores.iter().flatten().max() {
        Some(&best) => (0..samples.len())
            .filter(|&k| scores[k] == Some(best))
            .collect(),
        None => (0..samples.len())
            .filter(|&k| base.is_some() && base == Path::new(&*samples[k].atac).file_name())
            .collect(),
    }
}

/// Refuse samples whose ATAC is not there, saying how to give it.
pub fn ensure_atac(samples: &[Sample]) -> anyhow::Result<()> {
    for s in samples {
        if !Path::new(&*s.atac).exists() {
            let whose = match &s.name {
                Some(n) => format!("sample {n}'s ATAC"),
                None => "the run's ATAC".into(),
            };
            anyhow::bail!(
                "{whose} {} is not there: give where it is now with --atac",
                s.atac
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(name: Option<&str>, rna: &str, atac: &str) -> Sample {
        Sample {
            name: name.map(Into::into),
            rna: rna.into(),
            atac: atac.into(),
        }
    }

    fn b(v: &[&str]) -> Vec<Box<str>> {
        v.iter().map(|&s| s.into()).collect()
    }

    #[test]
    fn a_record_finds_its_files_near_it_or_where_they_were() {
        let dir = tempfile::tempdir().unwrap();
        let at = |p: &str| dir.path().join(p).to_string_lossy().into_owned();
        for f in ["d/r.zarr", "d/a.zarr"] {
            std::fs::create_dir_all(at(f)).unwrap();
        }
        let out = at("runs/p2g");
        std::fs::create_dir_all(at("runs")).unwrap();
        let s = [sample(None, &at("d/r.zarr"), &at("d/a.zarr"))];
        write_run_record(&out, &s, None, json!({})).unwrap();
        let read = read_samples(&out).unwrap().unwrap();
        assert_eq!(read[0].atac, s[0].atac);

        // The record moved alone: the files are where they were.
        std::fs::create_dir_all(at("far/away")).unwrap();
        std::fs::copy(run_record(&out), run_record(&at("far/away/p2g"))).unwrap();
        let read = read_samples(&at("far/away/p2g")).unwrap().unwrap();
        assert_eq!(read[0].atac, s[0].atac);

        // The record and its files moved together: they are near it.
        let moved = dir.path().with_extension("moved");
        std::fs::rename(dir.path(), &moved).unwrap();
        let read = read_samples(&moved.join("runs/p2g").to_string_lossy())
            .unwrap()
            .unwrap();
        assert_eq!(Path::new(&*read[0].atac), moved.join("d/a.zarr"));
        std::fs::remove_dir_all(moved).unwrap();
        assert!(read_samples("no/such/run").unwrap().is_none());
    }

    #[test]
    fn an_atac_given_stands_in_for_its_samples() {
        let run = vec![
            sample(Some("s1"), "r1", "x/s1.atac.zarr"),
            sample(Some("s2"), "r2", "x/s2.atac.zarr"),
            sample(Some("s2_b"), "r3", "x/s2_b.atac.zarr"),
        ];
        let s = with_atac(run.clone(), &b(&["new/S2_atac.h5"])).unwrap();
        assert_eq!(&*s[0].atac, "x/s1.atac.zarr");
        assert_eq!(&*s[1].atac, "new/S2_atac.h5");
        let s = with_atac(run.clone(), &b(&["new/s2_b.atac.h5"])).unwrap();
        assert_eq!(&*s[2].atac, "new/s2_b.atac.h5", "the name of most words");
        assert!(with_atac(run.clone(), &b(&["s3.atac.h5"])).is_err());
        assert!(with_atac(run.clone(), &b(&["a/s1.h5", "b/s1.h5"])).is_err());

        // A name of words that say what a file holds still matches.
        let held = vec![
            sample(Some("s1_multiome"), "x/s1_multiome.h5", "x/s1_multiome.h5"),
            sample(Some("s2"), "x/s2_gex.h5", "x/s2_atac.h5"),
        ];
        let s = with_atac(held, &b(&["y/s1_multiome_v2.h5"])).unwrap();
        assert_eq!(&*s[0].atac, "y/s1_multiome_v2.h5");

        // Named g0, g1 (their files alike): by the recorded file's name,
        // else, every sample's given, in the record's order.
        let alike = vec![
            sample(Some("g0"), "x/rna.zarr", "x/atac.zarr"),
            sample(Some("g1"), "y/rna.zarr", "y/atac.h5"),
        ];
        let s = with_atac(alike.clone(), &b(&["z/atac.h5"])).unwrap();
        assert_eq!(&*s[1].atac, "z/atac.h5");
        let s = with_atac(alike.clone(), &b(&["n/x/m.h5", "n/y/m.h5"])).unwrap();
        assert_eq!((&*s[0].atac, &*s[1].atac), ("n/x/m.h5", "n/y/m.h5"));
        assert!(with_atac(alike.clone(), &b(&["n/y/m.h5"])).is_err());
        assert!(
            with_atac(alike, &b(&["z/atac.h5", "n/y/atac.h5"])).is_err(),
            "two files of one sample are refused, not taken in order"
        );

        let one = vec![sample(None, "r", "a")];
        assert_eq!(&*with_atac(one.clone(), &b(&["b"])).unwrap()[0].atac, "b");
        assert!(with_atac(one, &b(&["b", "c"])).is_err());
    }
}
