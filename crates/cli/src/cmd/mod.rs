//! Per-command-group modules for the `avatar` binary. Each submodule owns one command group's
//! clap arg structs, handlers, and formatters; `main.rs` keeps only the top-level `Cli`/`Command`
//! enum and the `run()` dispatcher. Shared helpers used by more than one group live here.

pub mod anim_gen;
pub mod asset;
pub mod describe;
pub mod fbx;
pub mod lint;
pub mod mcp;
pub mod migrate;
pub mod osc;
pub mod physbone;
pub mod render;
pub mod schema;
pub mod stats;
pub mod toggle;
pub mod unitypackage;

use std::path::Path;

use anyhow::{Context, Result, bail};

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

/// Common write-safety flags for the generator commands (`anim-gen …`), so an agent can preview a
/// change and never silently clobber an existing asset. Mirror these on each generator's `Args`.
#[derive(clap::Args, Debug, Clone, Copy)]
pub(crate) struct WriteGuard {
    /// Preview only: report what would be written (and to where) without touching the filesystem.
    #[arg(long)]
    pub dry_run: bool,
    /// Allow overwriting an existing output file (otherwise the write is refused).
    #[arg(long)]
    pub force: bool,
}

/// Write generated `text` to `output` under the [`WriteGuard`] policy: stdout is always allowed; a
/// file write is reported-and-skipped under `--dry-run`, and refused (rather than clobbering) when
/// the file already exists without `--force`. Returns whether bytes were actually written.
pub(crate) fn write_out_guarded(
    output: Option<&Path>,
    text: &str,
    guard: WriteGuard,
) -> Result<bool> {
    let Some(path) = output else {
        // No file target: in dry-run we still suppress output so the command is a pure preview.
        if guard.dry_run {
            eprintln!("dry run: would write {} byte(s) to stdout", text.len());
            return Ok(false);
        }
        print!("{text}");
        return Ok(true);
    };
    if guard.dry_run {
        let exists = if path.exists() {
            " (would overwrite)"
        } else {
            ""
        };
        eprintln!(
            "dry run: would write {} byte(s) to {}{exists}",
            text.len(),
            path.display()
        );
        return Ok(false);
    }
    if path.exists() && !guard.force {
        bail!(
            "refusing to overwrite existing file {} (pass --force to overwrite, or --dry-run to preview)",
            path.display()
        );
    }
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    eprintln!("wrote {}", path.display());
    Ok(true)
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
