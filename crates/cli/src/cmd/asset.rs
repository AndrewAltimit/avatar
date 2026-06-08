//! `avatar asset set` — surgically edit a value inside a Unity YAML asset, preserving fileIDs,
//! GUID references, key order, and formatting byte-for-byte everywhere the edit doesn't touch.
//!
//! This is the *modify* counterpart to `lint`/`stats`/`describe` (which only read): an agent can
//! diagnose a problem, then apply the fix to the existing asset without re-emitting (and churning)
//! the whole file. Writes go through the shared [`WriteGuard`](crate::cmd::WriteGuard) — by default
//! the edited asset is printed to stdout (a pure preview); `-o <file>` writes it (refusing to
//! clobber without `--force`), and `--dry-run` reports without touching the filesystem.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use avatar_unity_yaml::{EditableUnityFile, Scalar, parse_path};
use clap::{Args, Subcommand};
use serde_json::json;

use crate::cmd::{WriteGuard, write_out_guarded};

#[derive(Subcommand, Debug)]
pub enum AssetCommand {
    /// Set a scalar value (or re-target a reference) at a path inside a Unity YAML asset.
    Set(SetArgs),
}

#[derive(Args, Debug)]
pub struct SetArgs {
    /// The Unity YAML asset to edit (`.asset`, `.controller`, `.prefab`, `.anim`, `.unity`).
    file: PathBuf,
    /// Select the document to edit by its `&fileID` anchor. Optional when the file has exactly one
    /// document.
    #[arg(long, value_name = "FILEID")]
    doc: Option<i64>,
    /// Path to the value, `/`-separated. Keys descend into mappings; a numeric segment indexes a
    /// sequence; a final segment may name a subfield of an inline reference. Examples:
    /// `m_Name`, `parameters/0/saved`, `m_Script/guid`.
    #[arg(long)]
    path: String,
    /// The new scalar value. Mutually exclusive with `--ref`. Type is inferred (int, then float,
    /// then bool, else string) unless `--type` is given.
    #[arg(long, value_name = "VALUE", conflicts_with = "ref_file_id")]
    value: Option<String>,
    /// Force the interpretation of `--value` instead of inferring it.
    #[arg(long = "type", value_enum)]
    value_type: Option<ScalarType>,
    /// Re-target the reference at `--path`: its new `fileID`. Combine with `--ref-guid`/`--ref-type`
    /// for a cross-asset reference; omit them for a local `{fileID: N}` reference.
    #[arg(long = "ref", value_name = "FILEID")]
    ref_file_id: Option<i64>,
    /// The GUID for a `--ref` cross-asset reference.
    #[arg(long, requires = "ref_file_id")]
    ref_guid: Option<String>,
    /// The Unity asset `type` for a `--ref` cross-asset reference (2 = imported asset, 3 = script).
    #[arg(long, requires = "ref_guid", default_value_t = 2)]
    ref_type: i64,
    /// Write the edited asset here instead of stdout. Pass the input path (with `--force`) to edit
    /// in place.
    #[arg(short, long)]
    output: Option<PathBuf>,
    /// Print a machine-readable JSON report (path, selected doc, the edit, the edited text) instead
    /// of the raw asset. `-o` still controls where the asset is written.
    #[arg(long)]
    json: bool,
    #[command(flatten)]
    guard: WriteGuard,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq)]
pub enum ScalarType {
    Int,
    Float,
    Bool,
    String,
}

pub fn set(args: &SetArgs) -> Result<()> {
    let text = std::fs::read_to_string(&args.file)
        .with_context(|| format!("reading {}", args.file.display()))?;
    let mut file = EditableUnityFile::parse(&text)
        .with_context(|| format!("parsing {} as Unity YAML", args.file.display()))?;

    // Resolve which document to edit.
    let doc = match args.doc {
        Some(file_id) => file.doc_by_file_id(file_id).with_context(|| {
            format!(
                "no document with fileID {file_id} in {}",
                args.file.display()
            )
        })?,
        None => {
            if file.documents().len() != 1 {
                bail!(
                    "{} has {} documents; pass --doc <fileID> to choose one ({})",
                    args.file.display(),
                    file.documents().len(),
                    file.documents()
                        .iter()
                        .map(|d| d.file_id.to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            0
        }
    };
    let doc_file_id = file.documents()[doc].file_id;
    let path = parse_path(&args.path);
    if path.is_empty() {
        bail!("--path is empty");
    }

    // Apply the edit: a reference re-target, or a scalar set.
    let edit_summary = if let Some(ref_id) = args.ref_file_id {
        file.set_reference(doc, &path, ref_id, args.ref_guid.as_deref(), args.ref_type)?;
        match &args.ref_guid {
            Some(g) => format!(
                "reference -> {{fileID: {ref_id}, guid: {g}, type: {}}}",
                args.ref_type
            ),
            None => format!("reference -> {{fileID: {ref_id}}}"),
        }
    } else {
        let raw = args
            .value
            .as_deref()
            .context("provide --value <VALUE> (a scalar) or --ref <FILEID> (a reference)")?;
        let scalar = make_scalar(raw, args.value_type)?;
        file.set_scalar(doc, &path, scalar)?;
        format!("scalar -> {raw}")
    };

    let edited = file.into_string();
    let wrote = if args.output.is_some() || !args.json {
        write_out_guarded(args.output.as_deref(), &edited, args.guard)?
    } else {
        false
    };

    if args.json {
        let report = json!({
            "kind": "asset-set",
            "file": args.file.display().to_string(),
            "doc_file_id": doc_file_id,
            "path": args.path,
            "edit": edit_summary,
            "output": args.output.as_ref().map(|p| p.display().to_string()),
            "written": wrote,
            "asset": edited,
        });
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else if wrote {
        // The asset was written to a file; report the edit on stderr so a piped asset stays clean.
        eprintln!(
            "edited {} (doc {doc_file_id}, {}): {edit_summary}",
            args.file.display(),
            args.path
        );
    }

    Ok(())
}

/// Build a [`Scalar`] from the raw string and an optional explicit type. Without a type, infer:
/// integer, then float, then bool (`true`/`false`), else string.
fn make_scalar(raw: &str, ty: Option<ScalarType>) -> Result<Scalar<'_>> {
    Ok(match ty {
        Some(ScalarType::Int) => Scalar::Int(
            raw.parse()
                .with_context(|| format!("'{raw}' is not an integer"))?,
        ),
        Some(ScalarType::Float) => Scalar::Float(
            raw.parse()
                .with_context(|| format!("'{raw}' is not a number"))?,
        ),
        Some(ScalarType::Bool) => Scalar::Bool(
            crate::cmd::parse_bool(raw).context("--type bool needs a boolean --value")?,
        ),
        Some(ScalarType::String) => Scalar::Str(raw),
        None => {
            if let Ok(i) = raw.parse::<i64>() {
                Scalar::Int(i)
            } else if let Ok(f) = raw.parse::<f64>() {
                Scalar::Float(f)
            } else if let Some(b) = crate::cmd::parse_bool_opt(raw) {
                Scalar::Bool(b)
            } else {
                Scalar::Str(raw)
            }
        }
    })
}
