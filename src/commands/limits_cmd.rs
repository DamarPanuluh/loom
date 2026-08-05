//! `loom limits` — the discoverable half of named limits.
//!
//! Plane: operability. Renders `crate::limits::all()` in human and JSON form.
//! The same names appear in violation errors ("killed: exceeded
//! timeout_secs=300"), so a worker can go from an error to its threshold and
//! remedy without bisecting. No store access: the registry is compile-time.

use crate::Result;

pub fn limits_cmd(json: bool) -> Result<()> {
    let limits = crate::limits::all();
    if json {
        println!("{}", serde_json::to_string_pretty(&limits)?);
        return Ok(());
    }
    let width = limits.iter().map(|l| l.name.len()).max().unwrap_or(0);
    for l in &limits {
        println!("{:<width$} {} {}", l.name, l.value, l.unit);
        println!("  scope:  {}", l.scope);
        println!("  remedy: {}", l.remedy);
    }
    println!(
        "\n{} limits enforced. Violations name the limit that fired.",
        limits.len()
    );
    Ok(())
}
