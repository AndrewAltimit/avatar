#!/usr/bin/env bash
# Wire git commit hooks into this clone.
#
# Primary path: the pre-commit framework (.pre-commit-config.yaml) — file
# hygiene, actionlint, shellcheck, plus local Rust fmt/clippy. Needs the
# `pre-commit` tool (pipx install pre-commit  /  pip install pre-commit).
#
# Fallback: if `pre-commit` isn't installed, point git at the tracked native
# hook (scripts/git-hooks/pre-commit), which covers Rust fmt + clippy only.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if command -v pre-commit >/dev/null 2>&1; then
    # The pre-commit framework installs into .git/hooks, which a non-default
    # core.hooksPath would shadow — clear any stale setting from the fallback.
    if git config --get core.hooksPath >/dev/null 2>&1; then
        git config --unset core.hooksPath
    fi
    pre-commit install
    echo "Installed pre-commit framework hooks. Run 'pre-commit run --all-files' to sweep the tree."
else
    echo "pre-commit not found — falling back to the native Rust-only hook." >&2
    echo "  Install the full hook set with: pipx install pre-commit && scripts/install-hooks.sh" >&2
    chmod +x "$repo_root/scripts/git-hooks/pre-commit"
    git config core.hooksPath scripts/git-hooks
    echo "Installed native git hook (core.hooksPath = scripts/git-hooks)."
fi
