# avatar-web-analyzer

The WebAssembly bundle behind the docs site's [FBX analyzer](../../site/_content/analyzer.html)
page: the same diagnose graph the CLI uses — `avatar-fbx` (parse) + `avatar-armature` (humanoid
rig check) + `avatar-stats` (performance rank) — compiled to wasm and run entirely in the
visitor's browser. The dropped file is read into memory and analyzed client-side; nothing is
uploaded anywhere.

## Key API

- `analyze(bytes, name) -> Result<Report>` — the pure core: parse one binary FBX, run the
  armature analysis and the geometry performance rank, list blendshape channels. Testable
  off-wasm against the `avatar-testkit` synthetic corpus.
- `analyze_fbx(bytes, name) -> String` *(wasm-bindgen export)* — the same thing as a JSON
  string; errors surface to JS as thrown exceptions. `armature` and `stats` in the JSON are
  the exact serde shapes the CLI's `--json` output uses.

## Build

```sh
wasm-pack build crates/web-analyzer --target web --release --out-dir ../../site/wasm
```

CI does this on every Pages deploy (`deploy-pages` in `.github/workflows/main-ci.yml`); the main
`ci` job additionally runs a plain `cargo build --target wasm32-unknown-unknown` as a cheap
regression gate. The bundle output under `site/wasm/` is never committed.

## Status

Built; the report shape is pinned by an in-crate test. `publish = false` — this crate exists for
the site, not as a library. The whole graph it pulls (`fbx`/`armature`/`stats` and their deps) is
pure Rust with no fs/network use on the analysis path, which is what makes the wasm build work;
keep it that way when extending it.
