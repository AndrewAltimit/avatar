//! `avatar schema` — emit JSON Schemas for the report types the read commands serialize under
//! `--json`.
//!
//! The `--json` output of `describe`/`lint`/`stats`/`armature`/`fbx inspect` is effectively an API
//! that agents couple to. This command publishes the shape of that output as a JSON Schema so a
//! consumer can introspect it instead of inferring it by trial parsing — and so we can catch our
//! own breaking changes by diffing a committed schema.
//!
//! Built behind the (default-on) `schema` feature; a `--no-default-features` build keeps the command
//! but reports that it was compiled out.

use anyhow::Result;
use clap::Args;

#[derive(Args, Debug)]
pub struct SchemaArgs {
    /// Which schema to emit. Omit to list the available names; use `all` to emit every schema as one
    /// `{ name: schema }` object.
    name: Option<String>,
}

#[cfg(feature = "schema")]
pub fn schema(args: &SchemaArgs) -> Result<()> {
    use anyhow::bail;
    use schemars::schema_for;
    use serde_json::Value;

    /// A named schema generator: builds one report type's JSON Schema as a serde `Value`.
    type Entry = (&'static str, fn() -> Value);

    // Keyed by the command whose `--json` emits the type. Keep in sync as report types are added.
    let entries: &[Entry] = &[
        ("describe", || {
            serde_json::to_value(schema_for!(crate::cmd::describe::DescribeReport)).unwrap()
        }),
        ("lint", || {
            serde_json::to_value(schema_for!(avatar_lint::LintReport)).unwrap()
        }),
        ("stats", || {
            serde_json::to_value(schema_for!(avatar_stats::PerfReport)).unwrap()
        }),
        ("armature", || {
            serde_json::to_value(schema_for!(avatar_armature::ArmatureReport)).unwrap()
        }),
        ("fbx-inspect", || {
            serde_json::to_value(schema_for!(crate::cmd::fbx::InspectSummary)).unwrap()
        }),
    ];

    let Some(name) = args.name.as_deref() else {
        println!("Available schemas (use `avatar schema <name>`, or `avatar schema all`):");
        for (n, _) in entries {
            println!("  {n}");
        }
        return Ok(());
    };

    if name == "all" {
        let map: serde_json::Map<String, Value> = entries
            .iter()
            .map(|(n, make)| ((*n).to_string(), make()))
            .collect();
        println!("{}", serde_json::to_string_pretty(&Value::Object(map))?);
        return Ok(());
    }

    match entries.iter().find(|(n, _)| *n == name) {
        Some((_, make)) => {
            println!("{}", serde_json::to_string_pretty(&make())?);
            Ok(())
        }
        None => bail!(
            "unknown schema '{name}'; available: {}",
            entries
                .iter()
                .map(|(n, _)| *n)
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

#[cfg(not(feature = "schema"))]
pub fn schema(_args: &SchemaArgs) -> Result<()> {
    anyhow::bail!(
        "this `avatar` was built without the `schema` feature; rebuild with `--features schema` \
         (it is on by default) to emit JSON Schemas"
    )
}
