#!/usr/bin/env bash
# Point git at the repo's tracked hooks (scripts/git-hooks).
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
chmod +x "$repo_root/scripts/git-hooks/pre-commit"
git -C "$repo_root" config core.hooksPath scripts/git-hooks
echo "Installed git hooks (core.hooksPath = scripts/git-hooks)."
