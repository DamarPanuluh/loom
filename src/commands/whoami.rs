use anyhow::Result;

use crate::output::Printer;

/// `loom whoami` — report the acting identity ($LOOM_AGENT), the resolved role,
/// and whether lane enforcement is ON (a role is set) or OFF (solo). The pulse
/// footer's `as <role>` / `solo` stamp answers this in passing; `whoami` is the
/// direct ask, so a driver can confirm its separation-of-duties context.
pub fn run(printer: &Printer) -> Result<()> {
    let env_set = std::env::var(crate::agent::ENV_VAR)
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    let acting = crate::agent::acting(None);
    let role = crate::agent::session_role();
    let enforced = role.is_some();

    if printer.json {
        printer.print_json(&serde_json::json!({
            "acting": acting,
            "role": role,
            "solo": !enforced,
            "lane_enforcement": enforced,
            "env_var": crate::agent::ENV_VAR,
            "env_set": env_set,
        }));
    } else {
        let src = if env_set {
            format!("from ${}", crate::agent::ENV_VAR)
        } else {
            format!("${} unset", crate::agent::ENV_VAR)
        };
        println!("  acting: {acting}   ({src})");
        match role {
            Some(r) => println!(
                "  role:   {r}  — lane enforcement ON (build/fix/validate/quality steps are gated to their role)"
            ),
            None => println!(
                "  role:   solo  — lane enforcement OFF; export {}=llm:<role> (builder|analyzer|fixer|validator|quality) to enable separation of duties",
                crate::agent::ENV_VAR
            ),
        }
    }
    Ok(())
}
