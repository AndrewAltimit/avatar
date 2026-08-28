# site/ — the avatar-tools documentation site

A small static site published to GitHub Pages. It mirrors the committed docs (`README.md`,
`PLAN.md`, `docs/`, per-crate READMEs), which remain the source of truth.

Modeled on the `legend-of-legaia-re` site: each page is authored as a `<main>`-only HTML fragment
under `_content/`, and the shared chrome (sidebar nav, on-this-page TOC, prev/next footer, search
overlay) is injected at runtime by `js/layout.js`. There is no build toolchain beyond Python 3 —
`_gen.py` stitches fragments into full pages and emits a search index.

## Build / preview locally

```sh
python3 site/_gen.py        # regenerate the HTML pages + search-index.json
# then open site/index.html in a browser (file:// works for the doc pages)
```

The **FBX analyzer** page additionally needs the wasm bundle (and, because module/wasm
loading is blocked under `file://`, an http server):

```sh
wasm-pack build crates/web-analyzer --target web --release --out-dir ../../site/wasm
python3 -m http.server -d site   # then open http://localhost:8000/analyzer.html
```

## Layout

| Path | What it is |
|------|------------|
| `_gen.py` | The generator: fragment → page, search index, self-maintaining `.gitignore`. |
| `_content/*.html` | Per-page `<main>` body fragments (the only thing you edit). |
| `js/layout.js` | The single source of truth for nav order; injects the chrome. |
| `js/main.js` | Scroll-spy, copy buttons, the search overlay. |
| `js/analyzer.js` | The Analyzer page: drag-and-drop FBX → the wasm report, rendered. |
| `wasm/` | **Generated** — the `crates/web-analyzer` bundle wasm-pack builds (never committed; wasm-pack writes its own `.gitignore`). |
| `css/styles.css` | The stylesheet (three-column wiki layout). |
| `img/favicon.svg` | Favicon. |
| `.gitignore` | **Generated** manifest of build outputs — they are not committed. |

The generated `*.html` and `search-index.json` are git-ignored: CI regenerates them on every
Pages deploy, and `_gen.py` rebuilds them for local preview, so committing them would only
duplicate `_content/`.

## How it's published

The `deploy-pages` job in `.github/workflows/main-ci.yml` runs on a **self-hosted** runner: on a
push to `main` it builds the analyzer wasm bundle (`wasm-pack build crates/web-analyzer`), runs
`python3 site/_gen.py`, then `actions/upload-pages-artifact` + `actions/deploy-pages`. The main
`ci` job carries a cheap `wasm32-unknown-unknown` build check so wasm regressions fail before the
deploy job.

## Adding a page

1. Write `_content/<name>.html` (start with a `<header class="page-header">` + `<p class="lede">`,
   then `<section class="doc-section" id="...">` blocks with `h2`/`h3` headings).
2. Add a `PAGES` entry in `_gen.py` and a matching `NAV` item (same `key`) in `js/layout.js`.
3. Run `python3 site/_gen.py`.

A bare `<code>some/doc.md</code>` in prose is auto-linked to that file on GitHub when the path
resolves to a committed Markdown file.
