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

/// A named schema generator: builds one report type's JSON Schema as a serde `Value`.
#[cfg(feature = "schema")]
type Entry = (&'static str, fn() -> serde_json::Value);

/// The published report-type schemas, keyed by the command whose `--json` emits the type. Keep in
/// sync as report types are added. Shared by the `avatar schema` command and the `avatar_schema`
/// MCP tool so both expose exactly the same set.
#[cfg(feature = "schema")]
fn entries() -> &'static [Entry] {
    use schemars::schema_for;
    // The `: &'static [Entry]` annotation is load-bearing: it gives each non-capturing closure the
    // expected `fn() -> Value` type so they coerce to a uniform element type (rvalue static
    // promotion then makes the array `'static`).
    let entries: &'static [Entry] = &[
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
        ("migrate", || {
            serde_json::to_value(schema_for!(avatar_migrate::MigrationReport)).unwrap()
        }),
        ("physbone", || {
            serde_json::to_value(schema_for!(avatar_migrate::physbone::PhysBoneInfo)).unwrap()
        }),
    ];
    entries
}

/// The names of the available schemas, in publication order.
#[cfg(feature = "schema")]
pub fn schema_names() -> Vec<&'static str> {
    entries().iter().map(|(n, _)| *n).collect()
}

/// Build one report type's JSON Schema by name. Errors (listing the valid names) on an unknown name.
#[cfg(feature = "schema")]
pub fn schema_value(name: &str) -> Result<serde_json::Value> {
    match entries().iter().find(|(n, _)| *n == name) {
        Some((_, make)) => Ok(make()),
        None => anyhow::bail!(
            "unknown schema '{name}'; available: {}",
            schema_names().join(", ")
        ),
    }
}

#[cfg(feature = "schema")]
pub fn schema(args: &SchemaArgs) -> Result<()> {
    use serde_json::Value;

    let entries = entries();

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

    println!("{}", serde_json::to_string_pretty(&schema_value(name)?)?);
    Ok(())
}

#[cfg(not(feature = "schema"))]
pub fn schema(_args: &SchemaArgs) -> Result<()> {
    anyhow::bail!(
        "this `avatar` was built without the `schema` feature; rebuild with `--features schema` \
         (it is on by default) to emit JSON Schemas"
    )
}
