#!/usr/bin/env python3
"""Idempotent installer for Molt's pre-push drift gate.

Installs ``.githooks/pre-push`` (the drift-harvest enforcement gate) into the
git *common* hooks dir so it fires on ``git push`` from the checkout AND every
linked worktree.

Why not ``core.hooksPath=.githooks``? That would ALSO enable ``.githooks/pre-commit``
(a project type-check with pre-existing diagnostics), which would block every
commit for every agent. So this installer wires ONLY the pre-push gate, straight
into ``<git-common-dir>/hooks/pre-push``, leaving commits untouched.

Idempotent and non-destructive:
  * Already-current Molt hook  -> no-op.
  * Stale Molt hook            -> refreshed from source.
  * Foreign (non-Molt) hook    -> preserved as ``pre-push.local`` and CHAINED
                                  (our hook runs it first; if it fails, the push
                                  is blocked before the drift gate even runs).
  * No existing hook           -> installed.

Usage:
  install_git_hooks.py            install / refresh the pre-push drift gate
  install_git_hooks.py --check    exit 1 if not installed/current (for CI/gates)
  install_git_hooks.py --uninstall  remove our hook (restore a chained foreign one)
"""

from __future__ import annotations

import argparse
import os
import stat
import subprocess
import sys
from pathlib import Path

MARKER = "molt-drift-gate-hook"
REPO_ROOT = Path(__file__).resolve().parent.parent
SOURCE = REPO_ROOT / ".githooks" / "pre-push"


def _common_hooks_dir(repo_root: Path = REPO_ROOT) -> Path:
    out = subprocess.run(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=str(repo_root),
        capture_output=True,
        text=True,
    )
    common = Path(out.stdout.strip() or ".git")
    if not common.is_absolute():
        common = (repo_root / common).resolve()
    return common / "hooks"


def _read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="ignore")
    except OSError:
        return ""


def _is_molt_hook(text: str) -> bool:
    return MARKER in text


def _make_executable(path: Path) -> None:
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _chained_wrapper(source_text: str) -> str:
    # A Molt hook that first runs a preserved foreign hook (pre-push.local),
    # then the drift gate. Insert the chain call right after the shebang line.
    lines = source_text.splitlines(keepends=True)
    shebang = lines[0] if lines and lines[0].startswith("#!") else "#!/usr/bin/env bash\n"
    rest = "".join(lines[1:]) if lines and lines[0].startswith("#!") else source_text
    chain = (
        'local_hook="$(dirname "$0")/pre-push.local"\n'
        'if [ -x "$local_hook" ]; then "$local_hook" "$@" || exit $?; fi\n'
    )
    return f"{shebang}{chain}{rest}"


def install(*, check: bool, uninstall: bool, repo_root: Path = REPO_ROOT) -> int:
    if not SOURCE.exists():
        print(f"drift-gate source missing: {SOURCE}", file=sys.stderr)
        return 2
    hooks = _common_hooks_dir(repo_root)
    target = hooks / "pre-push"
    preserved = hooks / "pre-push.local"
    source_text = _read(SOURCE)

    if uninstall:
        if target.exists() and _is_molt_hook(_read(target)):
            target.unlink()
            if preserved.exists():
                preserved.replace(target)
                print("removed drift gate; restored preserved pre-push.local")
            else:
                print("removed drift gate pre-push hook")
        else:
            print("no Molt drift gate installed; nothing to uninstall")
        return 0

    existing = _read(target) if target.exists() else ""
    want = source_text
    if target.exists() and not _is_molt_hook(existing):
        # Foreign hook present -> we will chain it; the installed content wraps source.
        want = _chained_wrapper(source_text)

    current = existing if _is_molt_hook(existing) else ""
    if current == want and target.exists():
        print(f"pre-push drift gate: up to date ({target})")
        return 0

    if check:
        state = "MISSING" if not target.exists() else (
            "FOREIGN (uninstalled)" if not _is_molt_hook(existing) else "OUTDATED"
        )
        print(f"pre-push drift gate: {state} at {target} — run: python tools/install_git_hooks.py")
        return 1

    hooks.mkdir(parents=True, exist_ok=True)
    if target.exists() and not _is_molt_hook(existing):
        target.replace(preserved)
        _make_executable(preserved)
        print(f"preserved existing pre-push hook -> {preserved} (chained)")

    target.write_text(want, encoding="utf-8", newline="\n")
    _make_executable(target)
    # Belt-and-suspenders: do NOT let core.hooksPath shadow us into the broken pre-commit.
    hp = subprocess.run(
        ["git", "config", "--get", "core.hooksPath"],
        cwd=str(repo_root), capture_output=True, text=True,
    ).stdout.strip()
    note = ""
    if hp:
        note = (
            f"\n  WARNING: core.hooksPath={hp} is set — it shadows {target}. "
            "Unset it (git config --unset core.hooksPath) so the drift gate fires "
            "and the pre-existing pre-commit type-check stays off."
        )
    print(f"installed pre-push drift gate -> {target}{note}")
    return 0


def main() -> int:
    ap = argparse.ArgumentParser(description="Install Molt's pre-push drift gate.")
    ap.add_argument("--check", action="store_true", help="exit 1 if not installed/current")
    ap.add_argument("--uninstall", action="store_true", help="remove the drift gate hook")
    args = ap.parse_args()
    return install(check=args.check, uninstall=args.uninstall)


if __name__ == "__main__":
    raise SystemExit(main())
