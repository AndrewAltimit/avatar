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

The **Avatar inspector** page (`analyzer.html`) additionally needs the wasm bundle (and, because module/wasm
loading is blocked under `file://`, an http server):

```sh
wasm-pack build crates/web-analyzer --target web --release --out-dir ../../site/wasm
python3 -m http.server -d site   # then open http://localhost:8000/analyzer.html
```

## Layout

| Path | What it is |
|------|------------|
| `_gen.py` | The generator: fragment → page, search index, self-maintaining `.gitignore`. Also auto-links bare `<code>x.md</code>` refs to GitHub and wraps every bare `<table>` in a `.table-wrap` scroll container. |
| `_content/*.html` | Per-page `<main>` body fragments (the only thing you edit). |
| `js/layout.js` | The single source of truth for nav order (incl. the `accent` "Try it" tag on the inspector item); injects the chrome and the light/dark theme toggle (`data-theme` on `<html>`, persisted as `theme` in localStorage; follows `prefers-color-scheme` when unset). |
| `js/main.js` | Scroll-spy, copy buttons, the search overlay, generic `.tabs` wiring (`[data-tabs]`), and the architecture stack diagram's hover highlighting. |
| `js/analyzer.js` | The inspector page: drag-and-drop FBX → the wasm report, rendered as tabs; owns the `TextureLibrary` (image files / folders dropped alongside the `.fbx`, matched to materials by Unity `.mat` `_MainTex` guid → `.meta` guid, then by basename, then by stem) and hands it to the viewer. |
| `js/viewer.js` | The three.js 3D preview (`createViewer`): builds the scene from `SceneView`, decodes embedded or library-resolved textures (PNG/JPEG/BMP/TGA, alpha detected → alpha-tested material), exposes `applyTextures` / `textureStatus`. |
| `wasm/` | **Generated** — the `crates/web-analyzer` bundle wasm-pack builds (never committed; wasm-pack writes its own `.gitignore`). |
| `css/styles.css` | The stylesheet: tokens for both themes, the three-column layout, and the shared components documented in its header comment (`.card-grid`, `.pill`, `.badge`, `.meter`, `.tabs`, `.kbd`, `.table-wrap`, `.notice`/`.hint`, `.cli`, `.see-also`, `.btn`). Page-specific styles for the inspector live in `css/analyzer.css` / `css/viewer.css`. |
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
   then `<section class="doc-section" id="...">` blocks with `h2`/`h3` headings; put a copyable
   `<div class="cli"><code>…</code></div>` near the top of any page that has a CLI, and end with a
   `<nav class="see-also">` row of related pages).
2. Add a `PAGES` entry in `_gen.py` and a matching `NAV` item (same `key`) in `js/layout.js`.
3. Run `python3 site/_gen.py`.

The home page's pipeline strip (`.pipeline`) and the architecture page's inline SVG stack diagram
(`.stack-figure`; crate boxes are `<a class="sd-crate" data-key="…">`, styled with theme tokens and
`currentColor` so they read in both themes) are plain markup in their fragments - no build step.

A bare `<code>some/doc.md</code>` in prose is auto-linked to that file on GitHub when the path
resolves to a committed Markdown file.
