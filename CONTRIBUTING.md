# Contributing

This is a Rust monorepo (workspace, edition 2024, `resolver = "3"`) of tools that operate on the
*files* a VRChat SDK3 avatar is made of. Read [`PLAN.md`](PLAN.md) for the architecture and roadmap,
[`CLAUDE.md`](CLAUDE.md) for the documentation map, and the per-crate `README.md`s for each crate's
purpose and API. This file is the how-to for building, testing, and extending the codebase.

## Build, test, lint

```sh
cargo build --workspace
cargo clippy --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
cargo run -p avatar-cli -- <subcommand>      # e.g. lint path/to/UnityProject
```

CI runs `fmt --check` + `clippy --all-targets -D warnings` + `test --workspace`; all three must be
green. Warnings are denied, so treat clippy lints as errors. The toolchain is pinned in
`rust-toolchain.toml` (fbxcel + edition 2024 want a recent compiler) — let it select the version.

### Pre-commit hook

Install the tracked hook once per clone; it runs `fmt` + `clippy` before each commit:

```sh
scripts/install-hooks.sh
```

This sets `core.hooksPath` to `scripts/git-hooks`. It is the same gate CI runs, locally.

## Conventions

These mirror the `legend-of-legaia-re` style; keep new code consistent with it.

- **Naming.** Crate directories are unprefixed (`crates/fbx`). Package names are `avatar-<slug>`
  (`avatar-fbx`); library names are `avatar_<slug>` (`avatar_fbx`). The one binary is `avatar`, in
  `crates/cli`.
- **Workspace inheritance.** `version`, `edition`, and `license` come from `[workspace.package]` via
  `field.workspace = true`. License is `MIT OR Unlicense`. Shared dependencies live in
  `[workspace.dependencies]` and are pulled in with `dep.workspace = true`; crate-specific deps are
  declared per-crate, pinned to a major version.
- **Error handling.** `anyhow` everywhere — `Result`, `Context`, `bail!`. No `thiserror`, no `eyre`.
  Validate inputs and fail loudly with `bail!` / `.context(...)`.
- **Parsing.** Manual, transparent parsing over heavy combinator frameworks: match the format
  byte/field for field and validate as you go, like the Legaia format crates.
- **CLI.** `clap` v4 derive. Logging (binaries only) is `log` + `env_logger`. Reports serialize with
  `serde` / `serde_json` (every report type derives `Serialize` so `--json` is a one-liner).
- **FBX scope.** Binary FBX only (FBX 7.x). ASCII FBX is out of scope — re-export as binary.

## Testing

Unit tests live next to the code. Integration tests live in `crates/<name>/tests/`.

Integration tests that need a real asset are **gated by an environment variable** pointing at a
sample file, e.g. `AVATAR_SAMPLE_FBX`. The pattern: if the env var is unset, print a skip notice and
return `Ok(())` so CI without fixtures stays green; if it is set, load that asset and run the real
assertion. Never commit user FBX / Unity projects — they are git-ignored (see `.gitignore`).

```rust
#[test]
fn round_trips_a_real_fbx() -> anyhow::Result<()> {
    let Ok(path) = std::env::var("AVATAR_SAMPLE_FBX") else {
        eprintln!("skipping: set AVATAR_SAMPLE_FBX to a binary FBX to run this test");
        return Ok(());
    };
    // ... exercise the real asset ...
    Ok(())
}
```

When authoring Unity-YAML fixtures: a Unity GUID is 32 hex chars and **must contain letters**
(e.g. `aaaa…`). An all-digit "guid" is parsed by `yaml-rust2` as a *number*, so `as_str()` returns
`None` and guid resolution silently breaks. Use a guid with letters in test fixtures.

## How to add a lint rule

Lint rules live in `crates/lint` (engine in `src/lib.rs`, built-in tables in `src/builtins.rs`).
Each finding is a `Diagnostic { severity, code, message, file, hint }` pushed into the report.

1. Pick the next free `VRCNNN` code. The full registry and the value-type / budget encodings are in
   [`docs/reference/sdk3-lint-rules.md`](docs/reference/sdk3-lint-rules.md) — read it first and keep
   it in sync when you add a rule.
2. Choose a severity deliberately: **error** = VRChat will reject it or the avatar breaks; **warn** =
   likely-but-not-certain (e.g. a menu referencing a parameter we can't find a declaration for, which
   another tool may generate at build time); **info** = advisory.
3. Emit the `Diagnostic` from the relevant scan in `run()`, with a project-relative `file` when
   applicable and a `hint` describing the fix.
4. Add a unit/integration test exercising the rule, and document the new code in
   `docs/reference/sdk3-lint-rules.md`.

## How to add a crate

Mirror an existing crate (e.g. `crates/lint` or `crates/stats`):

1. Create `crates/<slug>/` with `src/lib.rs`. Package `avatar-<slug>`, library `avatar_<slug>`.
2. `Cargo.toml` inherits workspace fields and pulls shared deps via `.workspace = true`:

   ```toml
   [package]
   name = "avatar-<slug>"
   version.workspace = true
   edition.workspace = true
   license.workspace = true

   [dependencies]
   anyhow.workspace = true
   # ... other shared deps via `.workspace = true`, crate-specific deps pinned to a major version
   ```

3. Add the crate to the workspace `members` list, and to `[workspace.dependencies]` (as
   `avatar-<slug> = { path = "crates/<slug>" }`) if other crates will depend on it.
4. Add a `crates/<slug>/README.md` (purpose · key API · status) and link it from the doc map in
   `CLAUDE.md` and `README.md`.
5. Keep `glam` out of the lint/CLI graph — it is confined to the runtime rig tier
   (`pose` / `input` / `gltf`).
</content>
</invoke>
