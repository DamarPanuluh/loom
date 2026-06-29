//! `loom export` — write the graph's travel format: deterministic JSON meant
//! to be committed to git (diffable in PRs) and rebuilt with `loom import`.

use anyhow::Result;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
#[cfg(unix)]
use std::io::ErrorKind;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::db::{GraphReadHandle, GraphReadRepository};
use crate::output::Printer;

pub fn run(out: &str, check: bool, printer: &Printer) -> Result<()> {
    let cwd = crate::db::resolve_root()?;
    let store = GraphReadHandle::open(&cwd)?;
    run_with_db(&store, &cwd, out, check, printer)
}

pub fn run_with_db(
    db: &dyn GraphReadRepository,
    root: &std::path::Path,
    out: &str,
    check: bool,
    printer: &Printer,
) -> Result<()> {
    let graph = db.export_json()?;
    run_graph(root, out, check, printer, graph)
}

fn run_graph(
    root: &std::path::Path,
    out: &str,
    check: bool,
    printer: &Printer,
    graph: serde_json::Value,
) -> Result<()> {
    let pretty = serde_json::to_string_pretty(&graph)?;

    if check {
        // The commit guard: the export is deterministic (same graph →
        // identical bytes), so freshness is a byte comparison. Non-zero exit
        // on drift makes this hookable (pre-commit / CI) — a graph change can
        // never silently ship without its travel format.
        if out == "-" {
            anyhow::bail!("--check needs a file to compare against (not '-') — use `loom export --check loom.graph.json` or drop --check.");
        }
        let confined_out = crate::repo::confine(root, std::path::Path::new(out))
            .ok_or_else(|| anyhow::anyhow!("export path escapes graph root: {out}"))?;
        let on_disk = fs::read_to_string(root.join(confined_out)).ok();
        let fresh = on_disk.as_deref() == Some(pretty.as_str());
        if printer.json {
            printer.print_json(&serde_json::json!({
                "status": if fresh { "ok" } else if on_disk.is_none() { "missing" } else { "stale" },
                "out": out,
                "next_step": if fresh {
                    format!("commit {out} so the graph travels")
                } else {
                    format!("run `loom export` and commit {out}")
                },
            }));
        } else if fresh {
            println!("{}", crate::output::up_to_date_line(out));
        } else if on_disk.is_none() {
            println!("✗ {out} does not exist — run `loom export` and commit it.");
        } else {
            println!("✗ {out} is STALE — the graph has changed since it was written.");
            println!("  Run `loom export` and commit the result.");
        }
        if !fresh {
            anyhow::bail!(
                "export file is stale or missing — run `loom export` and commit the result."
            );
        }
        return Ok(());
    }

    if out == "-" {
        println!("{pretty}");
        return Ok(());
    }
    let confined_out = crate::repo::confine(root, std::path::Path::new(out))
        .ok_or_else(|| anyhow::anyhow!("export path escapes graph root: {out}"))?;
    let target = root.join(confined_out);
    durable_replace(&target, pretty.as_bytes())?;

    let nodes: usize = graph["nodes"]
        .as_object()
        .map(|m| {
            m.values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len())
                .sum()
        })
        .unwrap_or(0);
    let edges: usize = graph["edges"]
        .as_object()
        .map(|m| {
            m.values()
                .filter_map(|v| v.as_array())
                .map(|a| a.len())
                .sum()
        })
        .unwrap_or(0);
    if printer.json {
        printer.print_json(&serde_json::json!({
            "status": "ok", "out": out, "nodes": nodes, "edges": edges,
            "next_step": format!("commit {out} so the graph travels"),
        }));
    } else {
        println!("✓ Graph exported to {out}  ({nodes} nodes, {edges} edges)");
        println!("  Commit it so the graph travels with the repo; rebuild anywhere with:");
        println!("  loom init . && loom import {out}");
    }
    Ok(())
}

/// Atomically and crash-durably replace `target` with `bytes`.
///
/// `rename` alone gives atomic visibility but not crash durability: after a
/// sudden power loss the directory entry can be lost even though the process saw
/// a successful rename. The export is the graph's committed travel format, so
/// write and fsync the temp file, rename it into place, then fsync the parent
/// directory where the platform supports directory fsync.
fn durable_replace(target: &Path, bytes: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let tmp = temp_path_for(target);
    let write_result = (|| -> Result<()> {
        let mut file = OpenOptions::new().write(true).create_new(true).open(&tmp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&tmp, target)?;
        sync_directory(parent)?;
        Ok(())
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    write_result
}

fn temp_path_for(target: &Path) -> PathBuf {
    let mut tmp = target.as_os_str().to_os_string();
    tmp.push(format!(
        ".tmp.{}.{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    PathBuf::from(tmp)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<()> {
    match File::open(path).and_then(|dir| dir.sync_all()) {
        Ok(()) => Ok(()),
        Err(err) if matches!(err.kind(), ErrorKind::InvalidInput | ErrorKind::Unsupported) => {
            Ok(())
        }
        Err(err) => Err(err.into()),
    }
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durable_replace_persists_final_bytes_and_cleans_temp() {
        let dir = std::env::temp_dir().join(format!(
            "loom-export-durable-{}-{}",
            std::process::id(),
            line!()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join("loom.graph.json");

        durable_replace(&target, b"{\"version\":1}\n").unwrap();
        durable_replace(&target, b"{\"version\":2}\n").unwrap();

        assert_eq!(fs::read_to_string(&target).unwrap(), "{\"version\":2}\n");
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains("loom.graph.json.tmp")
            })
            .collect();
        assert!(
            leftovers.is_empty(),
            "durable export should not leave temp files behind on success: {leftovers:?}"
        );
        let _ = fs::remove_dir_all(&dir);
    }
}
