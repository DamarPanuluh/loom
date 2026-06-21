//! `loom skill` — the OPT-IN lane-skill surface.
//!
//! loom serves each lane's discipline JUST-IN-TIME via `loom guide --role <lane>`
//! — no install, the binary IS the skill server. This command exists only for the
//! power user who wants to PIN the lane-skills as harness skills (model-invocable,
//! persistent, version-controlled). It emits each lane-skill as a `SKILL.md` — a
//! regenerable PROJECTION of the gate's lane table, exactly like `loom wiki` and
//! `loom export`. loom OFFERS; the LLM/user adds them to their own harness, so
//! loom never needs to know every harness's filesystem layout (it does support
//! `--write` for the common `.claude/skills/` case as a convenience).

use anyhow::{Context, Result};

use crate::cli::SkillCmd;
use crate::commands::guide::{lane_skill_manifest, lane_skill_markdown};
use crate::output::Printer;

pub fn run(cmd: SkillCmd, printer: &Printer) -> Result<()> {
    match cmd {
        SkillCmd::List => run_list(printer),
        SkillCmd::Show { role } => run_show(&role, printer),
        SkillCmd::Install { dir, write } => run_install(dir.as_deref(), write, printer),
    }
}

fn run_list(printer: &Printer) -> Result<()> {
    let manifest = lane_skill_manifest();
    if printer.json {
        printer.print_json(&serde_json::json!({
            "skills": manifest.iter().map(|(name, role, desc)| serde_json::json!({
                "skill": name, "role": role, "description": desc,
            })).collect::<Vec<_>>(),
            "note": "loom serves each lane JIT — no install needed (`loom guide --role <lane>`). \
                     `loom skill install` only PINS them as harness skills (opt-in).",
        }));
        return Ok(());
    }
    println!("── loom lane-skills ──────────────────────────────────────────────");
    println!("  Served JIT by the binary (`loom guide --role <lane>`) — no install needed.");
    println!("  Pin them as harness skills only if you want it: `loom skill install`.");
    println!();
    for (name, _role, desc) in &manifest {
        println!("  {name}");
        println!("      {desc}");
    }
    Ok(())
}

fn run_show(role: &str, printer: &Printer) -> Result<()> {
    let md = lane_skill_markdown(role);
    if md.is_empty() {
        anyhow::bail!(
            "Unknown lane '{role}'. Lanes: {}. (`loom skill list`)",
            crate::db::schema::ROLES.join(", "),
        );
    }
    if printer.json {
        printer.print_json(&serde_json::json!({
            "skill": format!("loom-{role}"),
            "path": skill_rel_path(role),
            "markdown": md,
        }));
    } else {
        print!("{md}");
    }
    Ok(())
}

/// The conventional relative path for a pinned lane-skill in a Claude-style harness.
fn skill_rel_path(role: &str) -> String {
    format!(".claude/skills/loom-{role}/SKILL.md")
}

fn run_install(dir: Option<&str>, write: bool, printer: &Printer) -> Result<()> {
    let base = dir.unwrap_or(".claude/skills");
    let files: Vec<(String, String, String)> = lane_skill_manifest()
        .iter()
        .map(|(name, role, _desc)| {
            let path = format!("{base}/loom-{role}/SKILL.md");
            (name.clone(), path, lane_skill_markdown(role))
        })
        .collect();

    let mut written: Vec<String> = Vec::new();
    if write {
        for (_name, path, md) in &files {
            let p = std::path::Path::new(path);
            if let Some(parent) = p.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("creating skills dir {}", parent.display()))?;
            }
            std::fs::write(p, md).with_context(|| format!("writing {path}"))?;
            written.push(path.clone());
        }
    }

    if printer.json {
        printer.print_json(&serde_json::json!({
            "wrote": write,
            "dir": base,
            "files": files.iter().map(|(name, path, md)| serde_json::json!({
                "skill": name, "path": path, "markdown": md,
            })).collect::<Vec<_>>(),
            "written": written,
            "next_step": if write {
                "Pinned. Your harness will auto-load these by description; or adopt one with `loom guide --role <lane>`.".to_string()
            } else {
                "Materialization plan only. Write each `markdown` to its `path` (or re-run with --write), or skip entirely — `loom guide --role <lane>` needs no install.".to_string()
            },
        }));
        return Ok(());
    }

    if write {
        println!("✓ Pinned {} lane-skill(s) under {}/:", written.len(), base);
        for path in &written {
            println!("  {path}");
        }
        println!();
        println!(
            "  These are a generated projection — re-run `loom skill install --write` after a"
        );
        println!(
            "  loom upgrade to refresh. The live charge is always `loom guide --role <lane>`."
        );
    } else {
        println!("── loom skill install (plan — nothing written) ───────────────────");
        println!(
            "  OPTIONAL: loom serves each lane JIT (`loom guide --role <lane>`) with no install."
        );
        println!("  To PIN them, write each file below (or re-run with `--write`):");
        println!();
        for (name, path, _md) in &files {
            println!("  {name}  →  {path}");
        }
        println!();
        println!(
            "  `loom skill show <lane>` prints one file; `loom skill install --json` returns all"
        );
        println!("  bodies + paths for an agent to write into its own harness.");
    }
    Ok(())
}
