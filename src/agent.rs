//! Acting-agent resolution for provenance stamping.
//!
//! Provenance fields (`inspected_by` on edges, `author` on notes/ignores) record
//! WHO made a change — and now carry the agent *role*, not just human/llm. This
//! is what makes separation-of-duties legible: the schema's `owner` says who
//! *should* fill a field; the provenance says who *did*.
//!
//! Resolution precedence: an explicit `--by`/`--inspected-by`/`--author` flag,
//! then the `LOOM_AGENT` env var (set once per agent session so every command
//! auto-stamps — e.g. `LOOM_AGENT=llm:analyzer`), then a plain `"llm"` fallback.
//! Values are free-form but should carry a role from `db::schema::role`
//! (e.g. `llm:analyzer`, `human:reviewer`).

use std::env;

/// The environment variable an agent sets once to auto-stamp its identity.
pub const ENV_VAR: &str = "LOOM_AGENT";

/// Resolve the acting agent for a provenance field.
pub fn acting(explicit: Option<&str>) -> String {
    if let Some(e) = explicit {
        if !e.is_empty() {
            return e.to_string();
        }
    }
    if let Ok(v) = env::var(ENV_VAR) {
        if !v.is_empty() {
            return v;
        }
    }
    "llm".to_string()
}
