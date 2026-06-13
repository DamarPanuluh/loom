//! Deprecated compatibility alias for `loom layer`.

use anyhow::Result;

use crate::cli::DomainCmd;
use crate::output::Printer;

pub fn run(cmd: DomainCmd, printer: &Printer) -> Result<()> {
    crate::commands::layer::run_deprecated_domain_alias(cmd, printer)
}
