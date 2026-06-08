# Contributing

This repository does not accept external contributions of any kind. All code changes are authored
by the maintainer and by AI agents operating under human direction.

## No External Contributions

This is a single-maintainer project. The development workflow itself — self-hosted CI, the generated
docs site, the commit-hook gate, and the conventions below — is tightly integrated and built around
maintainer + AI-agent authorship with human oversight. External pull requests would break the
assumptions the tooling is built on and will not be merged.

## No Feature Requests, Guidance, or Support

Feature requests from external parties will not be accepted or implemented. The maintainer does not
provide guidance, advice, consulting, or support on the use, adaptation, deployment, or integration
of anything in this repository. No advisory relationship exists or is implied. This project is not
affiliated with or endorsed by VRChat Inc. or Unity Technologies.

## No Community Engagement

The maintainer does not seek community engagement, discussion, or collaboration. Issues and comments
filed by external parties may be ignored without acknowledgment or response. This is intentional.

## What You Can Do

- **Fork it.** Clone the repo and adapt it however you want. The code is dual-licensed
  [MIT](LICENSE-MIT) OR [Unlicense](LICENSE).
- **Study it.** The docs cover the FBX/Unity-YAML layers, SDK3 linting, performance ranking, asset
  generation, and the OSC runtime. Start with [`PLAN.md`](PLAN.md) and [`CLAUDE.md`](CLAUDE.md).
- **Use it.** Any component, for any lawful purpose, under those licenses.

You do so entirely at your own risk and without any expectation of support, maintenance, or
acknowledgment from the maintainer.

---

The remainder of this file is an **internal development reference** for the maintainer and the AI
agents working in this repo. It is documentation of how the codebase is built and extended — not an
invitation to contribute.

## Build, test, lint

```sh
cargo build --workspace
cargo clippy --all-targets --workspace -- -D warnings
cargo fmt --all -- --check
cargo test --workspace
cargo run -p avatar-cli -- <subcommand>      # e.g. lint path/to/UnityProject
```

CI runs `fmt --check` + `clippy --all-targets -D warnings` + `test --workspace` on a self-hosted
runner (`.github/workflows/main-ci.yml`); all three must be green. Warnings are denied, so treat
clippy lints as errors. The toolchain is pinned in `rust-toolchain.toml` (fbxcel + edition 2024 want
a recent compiler) — let it select the version.

### Pre-commit hooks

Install the hooks once per clone:

```sh
pipx install pre-commit   # or: pip install pre-commit
scripts/install-hooks.sh
```

This wires in the [pre-commit framework](https://pre-commit.com) (`.pre-commit-config.yaml`): file
hygiene (trailing whitespace, EOF, line endings, JSON/YAML validation, merge-conflict / large-file
guards), [`actionlint`](https://github.com/rhysd/actionlint) on the workflows,
[`shellcheck`](https://www.shellcheck.net) on shell scripts, and local `cargo fmt` (autofix) +
`cargo clippy --all-targets --workspace -D warnings` — the same Rust gate CI runs. Sweep the whole
tree at any time with `pre-commit run --all-files`.

If `pre-commit` isn't installed, `scripts/install-hooks.sh` falls back to the tracked native hook
(`scripts/git-hooks/pre-commit`, `core.hooksPath = scripts/git-hooks`), which covers `fmt` + `clippy`
only. The lint test fixtures (`crates/*/tests/fixtures/`) and the `acceptance/` Unity project are
excluded from the hygiene hooks because tests read them verbatim.

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
sample file. The pattern: if the env var is unset, print a skip notice and return `Ok(())` so CI
without fixtures stays green; if it is set, load that asset and run the real assertion. Never commit
user FBX / Unity projects — they are git-ignored (see `.gitignore`). The gated vars:

| Var | Points at | Exercised by |
|-----|-----------|--------------|
| `AVATAR_SAMPLE_FBX` | a binary `.fbx` | armature check/fix + FBX geometry stats |
| `AVATAR_SAMPLE_UNITYPACKAGE` | an avatar `.unitypackage` | package open/extract, **and** the full read pipeline (real FBX parse + lint + project stats) over the extracted tree |
| `AVATAR_SAMPLE_UNITYPACKAGE_WORLD` | a world `.unitypackage` | the avatar-vs-world co-import cross-check |

On the self-hosted CI runner these point at local files so the otherwise-skipped real-data paths run
on every push (see `.github/workflows/main-ci.yml`); the committed synthetic fixture projects under
`fixtures/projects/` already cover the lint / project-stats paths hermetically.

**Golden (snapshot) tests.** Report-producing read paths are pinned with golden tests via the
`avatar-testkit` harness: the whole serialized report is diffed against a committed
`crates/<crate>/tests/golden/*.json` snapshot, so any change to the report surface is a reviewable
diff rather than a silent regression. After an intentional change, regenerate and review:
`UPDATE_GOLDEN=1 cargo test --workspace` then `git diff -- '**/tests/golden/**'`. The corpus +
harness are documented in [`docs/reference/testing.md`](docs/reference/testing.md) and
[`fixtures/README.md`](fixtures/README.md).

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
