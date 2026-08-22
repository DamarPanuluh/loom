use super::*;

pub(crate) fn journey_lint(
    graph: Option<&Path>,
    journey_key: Option<&str>,
    json_output: bool,
) -> Result<()> {
    let store = open_read(graph)?;
    let journeys = if let Some(key) = journey_key {
        vec![resolve_journey(&store, key)?]
    } else {
        let mut nodes = store.list_nodes(Some(NodeType::Journey), usize::MAX)?;
        nodes.sort_by(|a, b| a.name.cmp(&b.name));
        nodes
    };
    let mut findings = Vec::new();
    let mut scanned = 0;
    for journey in journeys {
        let (_, spec, hash) = load_registered_journey(&store, &journey.id)?;
        let artifact = journey
            .body
            .get("artifact")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Journey '{}' has no artifact", journey.name))?;
        let surface = Path::new(artifact)
            .parent()
            .unwrap_or(Path::new(""))
            .join("surfaces")
            .join(format!("{}.surface.json", journey.name));
        let absolute = store.root().join(&surface);
        if !absolute.is_file() {
            bail!(
                "Journey '{}' has no surface manifest at '{}'",
                journey.name,
                surface.display()
            );
        }
        let manifest = crate::journey::SurfaceManifest::parse_json(&absolute)?;
        manifest.validate_for(&spec, &hash)?;
        manifest.validate_setup_for_store(&store)?;
        let report = manifest.lint(&store, &spec, &surface.to_string_lossy())?;
        scanned += report.scanned;
        findings.extend(report.findings);
    }
    let report = crate::journey::JourneyLintReport::new(scanned, findings);
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        for finding in &report.findings {
            let location = finding
                .operation
                .as_deref()
                .map(|op| format!(" operation={op}"))
                .unwrap_or_default();
            println!(
                "{:?} {} {}{}: {}",
                finding.severity, finding.rule, finding.journey_id, location, finding.message
            );
        }
        println!(
            "{}: scanned={}, blocking={}, advisory={}",
            report.status, report.scanned, report.blocking, report.advisory
        );
    }
    if report.blocking > 0 {
        let message = format!("Journey lint found {} blocking finding(s)", report.blocking);
        if json_output {
            return Err(super::JsonStdoutComplete::fail(message));
        }
        bail!("{message}");
    }
    Ok(())
}
