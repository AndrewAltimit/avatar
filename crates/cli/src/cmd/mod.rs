//! Per-command-group modules for the `avatar` binary. Each submodule owns one command group's
//! clap arg structs, handlers, and formatters; `main.rs` keeps only the top-level `Cli`/`Command`
//! enum and the `run()` dispatcher. Shared helpers used by more than one group live here.

pub mod anim_gen;
pub mod fbx;
pub mod lint;
pub mod osc;
pub mod render;
pub mod stats;
pub mod unitypackage;

use std::path::Path;

use anyhow::{Context, Result};

/// Emit a fully-rendered report to a file (when `output` is set) or stdout (otherwise). The text
/// is the complete report — human or `--json` — already formatted by the caller; a trailing newline
/// is added so a redirected file ends cleanly. Used by `lint` and `stats` for their `-o/--output`.
pub(crate) fn emit_report(output: Option<&Path>, text: &str) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, format!("{text}\n"))
                .with_context(|| format!("writing report to {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => println!("{text}"),
    }
    Ok(())
}

/// Write generated text to `output` (a file) or, when `None`, stdout.
pub(crate) fn write_out(output: Option<&Path>, text: &str) -> Result<()> {
    match output {
        Some(path) => {
            std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
            eprintln!("wrote {}", path.display());
        }
        None => print!("{text}"),
    }
    Ok(())
}

/// Parse a bool from the usual textual spellings (`true`/`false`, `1`/`0`, `on`/`off`).
pub(crate) fn parse_bool(s: &str) -> Result<bool> {
    parse_bool_opt(s).with_context(|| format!("'{s}' is not a boolean (try true/false)"))
}

pub(crate) fn parse_bool_opt(s: &str) -> Option<bool> {
    match s.to_ascii_lowercase().as_str() {
        "true" | "1" | "on" | "yes" => Some(true),
        "false" | "0" | "off" | "no" => Some(false),
        _ => None,
    }
}
