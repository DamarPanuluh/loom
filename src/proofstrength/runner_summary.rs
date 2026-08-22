use crate::store::Store;
use crate::Result;

/// What the runner said it checked, read from the observation Loom recorded.
///
/// A positive structured assertion count on a passing observed run is the
/// authoritative path. Legacy command runners that did not populate that field
/// may still earn the witness from a summary naming positive passes and zero
/// failures. "ok" alone never counts.
///
/// Deliberately conservative and deliberately separate from declared
/// assertions. The tool is reporting on itself, so this is weaker evidence than
/// an expectation loom checked — the witness records which kind you have.
/// The structured observation behind S2: a run that actually checked
/// content assertions. `parse_runner_summary` is the legacy fallback for
/// generic command validations whose runners print their verdict last;
/// compiler-owned Journey proofs never use it — their assertion count is
/// structured machine evidence, and the excerpt must never be a trust input.
pub(super) fn reported_assertions(
    edge: &Option<crate::model::Edge>,
    store: &Store,
    parse_excerpt: bool,
) -> Result<Option<String>> {
    let Some(edge) = edge else { return Ok(None) };
    let Some(view) = store.fact(
        &crate::store::Subject::Edge(edge.id.clone()),
        crate::model::Claim::Verdict,
    )?
    else {
        return Ok(None);
    };
    for row in &view.evidence {
        let crate::evidence::Evidence::Run(run) = &row.payload else {
            continue;
        };
        if run.exit_code == 0 && run.assertions > 0 {
            return Ok(Some(run.assertions.to_string()));
        }
    }
    if !parse_excerpt {
        return Ok(None);
    }
    for row in &view.evidence {
        let crate::evidence::Evidence::Run(run) = &row.payload else {
            continue;
        };
        if run.exit_code == 0 {
            if let Some(summary) = parse_runner_summary(&run.stdout_excerpt) {
                return Ok(Some(summary));
            }
        }
    }
    Ok(None)
}

/// Recognise the common runners' summary lines. Returns a human-readable
/// description of what was checked, or `None` when the output does not state it.
pub fn parse_runner_summary(output: &str) -> Option<String> {
    for line in output.lines() {
        let lower = line.to_ascii_lowercase();
        // Rust: "test result: ok. 4 passed; 0 failed; ..."
        if lower.contains("test result:") && lower.contains("passed") {
            let passed = number_before(&lower, "passed")?;
            let failed = number_before(&lower, "failed").unwrap_or(0);
            if passed > 0 && failed == 0 {
                return Some(format!("{passed} test(s) reported passing by the runner"));
            }
        }
        // pytest: "==== 12 passed in 0.4s ====", jest: "Tests: 12 passed, 12 total"
        if (lower.contains("passed") || lower.contains("passing"))
            && !lower.contains("failed")
            && !lower.contains("failing")
        {
            let passed = number_before(&lower, "passed")
                .or_else(|| number_before(&lower, "passing"))
                .unwrap_or(0);
            if passed > 0 {
                return Some(format!("{passed} test(s) reported passing by the runner"));
            }
        }
    }
    None
}

/// The integer in a `<number> <word>` pair on this line, if there is one.
///
/// Scans TOKEN PAIRS rather than searching for the word: `find("failed")`
/// matches the FAILED in "test result: FAILED. 3 passed; 2 failed", takes the
/// token before it, fails to parse it, and defaults the failure count to zero —
/// which graded a failing run as evidence. My own test caught it.
fn number_before(line: &str, word: &str) -> Option<usize> {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    tokens.windows(2).find_map(|pair| {
        let tail = pair[1].trim_matches(|c: char| !c.is_ascii_alphabetic());
        (tail == word).then(|| {
            pair[0]
                .trim_matches(|c: char| !c.is_ascii_digit())
                .parse()
                .ok()
        })?
    })
}

#[cfg(test)]
mod runner_summary_tests {
    use super::parse_runner_summary;

    /// A runner's own summary states WHAT it checked. Refusing to read it told
    /// every repo with a real test suite that its suite was liveness-only.
    #[test]
    fn common_runners_are_understood() {
        assert!(
            parse_runner_summary("test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured")
                .unwrap()
                .contains('4')
        );
        assert!(parse_runner_summary("==== 12 passed in 0.42s ====")
            .unwrap()
            .contains("12"));
        assert!(parse_runner_summary("Tests:       7 passed, 7 total")
            .unwrap()
            .contains('7'));
    }

    /// Exiting zero having checked nothing is exactly the S1 case this
    /// distinguishes itself from, so it must not be mistaken for evidence.
    #[test]
    fn a_bare_success_is_not_an_assertion() {
        assert_eq!(parse_runner_summary(""), None);
        assert_eq!(parse_runner_summary("ok"), None);
        assert_eq!(parse_runner_summary("Done in 0.2s"), None);
        // Zero tests ran: nothing was checked.
        assert_eq!(
            parse_runner_summary("test result: ok. 0 passed; 0 failed"),
            None
        );
        // Something failed: the run is not evidence the behavior holds.
        assert_eq!(
            parse_runner_summary("test result: FAILED. 3 passed; 2 failed"),
            None
        );
    }
}
