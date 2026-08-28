#!/usr/bin/env python3
"""Generate the avatar-tools documentation site from per-page content fragments.

Modeled on legend-of-legaia-re/site/_gen.py. Layout (sidebar nav, TOC rail,
prev/next footer, search overlay) is shared via JS (site/js/layout.js), so each
generated HTML file is just <head> + <main> with the page-specific body. The
chrome is injected at runtime by layout.js.

Also writes:
  - site/search-index.json: one entry per (page, h2/h3 heading) plus one root
    entry per page. Drives the in-page search overlay.
  - site/.gitignore: a self-maintaining manifest of every file this run writes,
    so the generated artifacts stay untracked (CI rebuilds them on deploy).

Run from the repo root (or anywhere — paths are resolved relative to this file):
    python3 site/_gen.py
"""
from __future__ import annotations
import json
import re
import sys
from html.parser import HTMLParser
from pathlib import Path

ROOT = Path(__file__).resolve().parent
CONTENT = ROOT / "_content"
REPO_ROOT = ROOT.parent

# Base for linking a committed repo file (the `<repo>/blob/main/<path>` form the
# pages use for "full reference" links).
REPO_BLOB = "https://github.com/AndrewAltimit/avatar/blob/main"

SITE_TITLE = "avatar tools"


def _committed_md_index() -> tuple[set[str], dict[str, str]]:
    """Index the committed Markdown files the site can link to: everything under
    `docs/` and `crates/` plus the top-level `*.md` (README / PLAN / ...).
    Returns `(paths, by_basename)` where `paths` is the set of repo-relative
    paths and `by_basename` maps a basename to its path **only when that
    basename is unique** (ambiguous basenames like `README.md` are left out so
    they only resolve via an exact path). Deliberately excludes generated trees
    (`target/`), so an unresolved reference is left as plain text rather than
    linked to a 404."""
    paths: set[str] = set()
    for base in ("docs", "crates"):
        d = REPO_ROOT / base
        if d.exists():
            for p in d.rglob("*.md"):
                paths.add(p.relative_to(REPO_ROOT).as_posix())
    for p in REPO_ROOT.glob("*.md"):
        paths.add(p.name)
    by_basename: dict[str, str] = {}
    clash: set[str] = set()
    for path in paths:
        name = path.rsplit("/", 1)[-1]
        if name in by_basename:
            clash.add(name)
        by_basename[name] = path
    for name in clash:
        by_basename.pop(name, None)
    return paths, by_basename


def _resolve_md(ref: str, paths: set[str], by_basename: dict[str, str]) -> str | None:
    """Resolve a Markdown reference as written in page prose to a committed
    repo-relative path, or `None` if it isn't a committed file. Tries the
    reference verbatim, then under `docs/`, then by unique basename."""
    ref = ref.strip().removeprefix("./")
    if ref in paths:
        return ref
    docs_rel = f"docs/{ref}"
    if docs_rel in paths:
        return docs_rel
    return by_basename.get(ref.rsplit("/", 1)[-1])


_TABLE_RE = re.compile(r"<table\b[^>]*>.*?</table>", re.S)


def wrap_tables(body: str) -> str:
    """Wrap every bare `<table>` in a `.table-wrap` div so wide tables scroll
    horizontally on narrow screens instead of overflowing the page. A table
    already inside a `.table-wrap` (the fragment author wrapped it by hand)
    is left alone."""

    def repl(m: re.Match) -> str:
        before = body[: m.start()].rstrip()
        if before.endswith('<div class="table-wrap">'):
            return m.group(0)
        return f'<div class="table-wrap">{m.group(0)}</div>'

    return _TABLE_RE.sub(repl, body)


_MD_CODE_RE = re.compile(r"<code>([^<>]+?\.md)</code>")


def autolink_md_refs(body: str, paths: set[str], by_basename: dict[str, str]) -> str:
    """Wrap every bare `<code>PATH.md</code>` whose path resolves to a committed
    repo file in a link to that file on GitHub. Skips `<code>` spans already
    inside an `<a>` (so existing links aren't double-wrapped) and references
    that don't resolve to a committed file."""

    def inside_anchor(upto: str) -> bool:
        return upto.rfind("<a ") > upto.rfind("</a>")

    def repl(m: re.Match) -> str:
        if inside_anchor(body[: m.start()]):
            return m.group(0)
        resolved = _resolve_md(m.group(1), paths, by_basename)
        if resolved is None:
            return m.group(0)
        return (
            f'<a href="{REPO_BLOB}/{resolved}" target="_blank" rel="noopener">'
            f"{m.group(0)}</a>"
        )

    return _MD_CODE_RE.sub(repl, body)


# Pages that benefit from breaking out of the prose reading-width cap (wide
# tables / card grids). Everything else stays narrow for readability.
WIDE_PAGES: set[str] = {
    "crates/index",
    "cli/index",
    "reference/sdk3-lint-rules",
    "reference/humanoid-bones",
}


def asset_version() -> str:
    """Short content hash of every js/css asset, appended as `?v=` so browsers never serve a stale
    module after a deploy (or a local rebuild — `http.server` sends no cache headers)."""
    import hashlib
    h = hashlib.sha1()
    for sub in ("js", "css"):
        for f in sorted((ROOT / sub).glob("*")):
            if f.is_file():
                h.update(f.name.encode())
                h.update(f.read_bytes())
    return h.hexdigest()[:10]


ASSET_V = None


def versioned(extra_head: str) -> str:
    """Append `?v=` to the local css/js references inside a page's HEAD block."""
    return re.sub(r'((?:href|src)="(?:\.\./)*(?:css|js)/[^"?]+)"', rf'\1?v={ASSET_V}"', extra_head)


def html_template(title: str, depth: int, active_key: str, body: str, extra_head: str = "") -> str:
    global ASSET_V
    if ASSET_V is None:
        ASSET_V = asset_version()
    v = "?v=" + ASSET_V
    css = "../" * depth + "css/styles.css" + v
    layout_js = "../" * depth + "js/layout.js" + v
    main_js = "../" * depth + "js/main.js" + v
    extra_head = versioned(extra_head)
    favicon = "../" * depth + "img/favicon.svg"
    content_cls = "content wide-page" if active_key in WIDE_PAGES else "content"
    return f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8">
  <meta name="viewport" content="width=device-width, initial-scale=1.0">
  <title>{title} - {SITE_TITLE}</title>
  <link rel="icon" href="{favicon}" type="image/svg+xml">
  <link rel="stylesheet" href="{css}">
  {extra_head}
</head>
<body>
<a class="skip-link" href="#content">Skip to content</a>
<div class="app">
<main class="{content_cls}" id="content">
{body}
</main>
</div>
<script src="{layout_js}"></script>
<script>injectLayout({{ active: {active_key!r} }});</script>
<script src="{main_js}"></script>
</body>
</html>
"""


# (out_path, title, active_key, body_file)
# active_key MUST match the `key` used for this page in site/js/layout.js NAV.
PAGES: list[tuple[str, str, str, str]] = [
    # depth = 0 (root)
    ("index.html",       "Home",          "home",         "home.html"),
    ("architecture.html","How it stacks", "architecture", "architecture.html"),
    ("quickstart.html",  "Quick start",   "quickstart",   "quickstart.html"),
    ("analyzer.html",    "Avatar inspector", "analyzer",  "analyzer.html"),
    # depth = 1
    ("crates/index.html","Crates",        "crates/index", "crates/index.html"),
    ("cli/index.html",   "CLI commands",  "cli/index",    "cli/index.html"),
    ("reference/index.html",            "Reference",          "reference/index",            "reference/index.html"),
    ("reference/humanoid-bones.html",   "Humanoid bones",     "reference/humanoid-bones",   "reference/humanoid-bones.html"),
    ("reference/sdk3-lint-rules.html",  "SDK3 lint rules",    "reference/sdk3-lint-rules",  "reference/sdk3-lint-rules.html"),
    ("reference/armature-repair.html",  "Armature repair",    "reference/armature-repair",  "reference/armature-repair.html"),
    ("reference/performance-stats.html","Performance stats",  "reference/performance-stats","reference/performance-stats.html"),
    ("reference/anim-gen.html",         "Asset generation",   "reference/anim-gen",         "reference/anim-gen.html"),
    ("reference/unity-yaml-edit.html",  "Editing Unity YAML", "reference/unity-yaml-edit",  "reference/unity-yaml-edit.html"),
    ("reference/unity-asset.html",      "Typed asset readers","reference/unity-asset",      "reference/unity-asset.html"),
    ("reference/migrate.html",          "SDK2 → SDK3 migration","reference/migrate",        "reference/migrate.html"),
    ("reference/physbone.html",         "PhysBone tuning",    "reference/physbone",         "reference/physbone.html"),
    ("reference/render.html",           "Rendering & preview","reference/render",           "reference/render.html"),
    ("reference/unitypackage.html",     "The .unitypackage format","reference/unitypackage","reference/unitypackage.html"),
    ("reference/rig-runtime.html",      "Runtime rig layer",  "reference/rig-runtime",      "reference/rig-runtime.html"),
    ("reference/osc-runtime.html",      "OSC runtime",        "reference/osc-runtime",      "reference/osc-runtime.html"),
    ("reference/mcp.html",              "MCP server",         "reference/mcp",              "reference/mcp.html"),
    ("reference/testing.html",          "Testing & fixtures", "reference/testing",          "reference/testing.html"),
]


# ---------------------------------------------------------------------------
# Search index: parse each body fragment for its lede + h2/h3 headings.
# ---------------------------------------------------------------------------

class _IndexParser(HTMLParser):
    def __init__(self) -> None:
        super().__init__(convert_charrefs=True)
        self.lede: list[str] = []
        self.headings: list[dict] = []  # [{level, text, id, snippet}]
        self._current_heading: dict | None = None
        self._capture_into: list[str] | None = None
        self._lede_open = False
        self._h_open = False
        self._h_level = 0
        self._h_attrs: dict[str, str] = {}
        self._section_id: str | None = None
        self._snippet_buf: list[str] = []

    def handle_starttag(self, tag, attrs):
        attrs_d = dict(attrs)
        if tag == "p" and self._lede_open is False and "lede" in (attrs_d.get("class") or ""):
            self._lede_open = True
            self._capture_into = self.lede
        elif tag in ("h2", "h3"):
            if self._current_heading is not None:
                self._current_heading["snippet"] = " ".join(self._snippet_buf).strip()[:200]
                self.headings.append(self._current_heading)
                self._current_heading = None
            self._h_open = True
            self._h_level = 2 if tag == "h2" else 3
            self._h_attrs = attrs_d
            self._capture_into = []
            self._snippet_buf = []
        elif tag == "section" and "doc-section" in (attrs_d.get("class") or ""):
            self._section_id = attrs_d.get("id")

    def handle_endtag(self, tag):
        if tag == "p" and self._lede_open:
            self._lede_open = False
            self._capture_into = None
        elif tag in ("h2", "h3") and self._h_open:
            text = "".join(self._capture_into or []).strip()
            self._h_open = False
            heading_id = self._h_attrs.get("id") or self._section_id or _slugify(text)
            self._current_heading = {"level": self._h_level, "text": text, "id": heading_id}
            self._capture_into = None
            self._snippet_buf = []
        elif tag == "section" and self._current_heading is not None:
            self._current_heading["snippet"] = " ".join(self._snippet_buf).strip()[:200]
            self.headings.append(self._current_heading)
            self._current_heading = None
            self._snippet_buf = []
            self._section_id = None

    def handle_data(self, data):
        if self._capture_into is not None:
            self._capture_into.append(data)
        elif self._current_heading is not None:
            self._snippet_buf.append(data)

    def close(self) -> None:
        if self._current_heading is not None:
            self._current_heading["snippet"] = " ".join(self._snippet_buf).strip()[:200]
            self.headings.append(self._current_heading)
            self._current_heading = None
        super().close()


def _slugify(s: str) -> str:
    out = re.sub(r"[^a-z0-9]+", "-", s.lower()).strip("-")
    return out or "section"


def build_search_entries(out_path: str, title: str, body: str, section_label: str) -> list[dict]:
    parser = _IndexParser()
    parser.feed(body)
    parser.close()

    lede_text = re.sub(r"\s+", " ", "".join(parser.lede)).strip()
    entries: list[dict] = [{
        "href": out_path,
        "title": title,
        "section": section_label,
        "snippet": lede_text[:240],
    }]
    for h in parser.headings:
        if not h["text"]:
            continue
        entries.append({
            "href": out_path,
            "anchor": h["id"],
            "title": h["text"],
            "section": title,
            "snippet": (h.get("snippet") or "")[:200],
        })
    return entries


def section_label_for(out_path: str) -> str:
    if "/" not in out_path:
        return "overview"
    return out_path.split("/", 1)[0]


def write_gitignore(generated: list[str]) -> None:
    """Write site/.gitignore listing every artifact this run produced.

    Self-maintaining: the ignore list is exactly what _gen.py writes, so it can
    never drift from the real outputs. The .gitignore itself stays tracked (it's
    the manifest); the listed files do not.
    """
    header = [
        "# Generated by site/_gen.py - do NOT edit, do NOT commit the listed files.",
        "# These are build artifacts derived from _content/. Run `python3 site/_gen.py`",
        "# for a local file:// preview; CI regenerates them on the Pages deploy.",
        "# This manifest file is itself tracked.",
        "",
    ]
    # Anchor every entry to the site root with a leading "/" so a generated
    # output (e.g. /index.html, /crates/index.html) never also matches a
    # same-named source fragment under _content/ (e.g. _content/crates/index.html).
    lines = header + sorted(f"/{p}" for p in generated) + [""]
    (ROOT / ".gitignore").write_text("\n".join(lines))


def main() -> int:
    written = 0
    search_index: list[dict] = []
    generated: list[str] = []

    md_paths, md_by_basename = _committed_md_index()

    for out_path, title, active, body_file in PAGES:
        depth = out_path.count("/")
        src = CONTENT / body_file
        if not src.exists():
            print(f"  skip {out_path:36s} (no content yet)")
            continue
        body = src.read_text()

        extra_head = ""
        if body.startswith("<!--HEAD:"):
            end = body.find("-->")
            extra_head = body[len("<!--HEAD:"):end].strip()
            body = body[end + 3:].lstrip()

        body = autolink_md_refs(body, md_paths, md_by_basename)
        body = wrap_tables(body)

        html = html_template(title, depth, active, body, extra_head)
        out = ROOT / out_path
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(html)
        written += 1
        generated.append(out_path)
        print(f"  wrote {out_path}")

        search_index.extend(
            build_search_entries(out_path, title, body, section_label_for(out_path))
        )

    (ROOT / "search-index.json").write_text(
        json.dumps(search_index, ensure_ascii=False, separators=(",", ":"))
    )
    generated.append("search-index.json")

    write_gitignore(generated)

    print(f"\n{written} pages written, {len(search_index)} search entries")
    return 0


if __name__ == "__main__":
    sys.exit(main())
