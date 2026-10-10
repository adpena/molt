#!/usr/bin/env python3
"""Idempotent installer for Molt's managed Git hooks.

Installs each managed hook from ``.githooks/`` into the git *common* hooks dir,
so it fires in the checkout AND every linked worktree:

  * ``pre-push``   — the drift-harvest enforcement gate, then the generated-
                     projection check CI runs as ``repository.generators``.
  * ``commit-msg`` — the commit-attribution policy (no Claude attribution);
                     CI enforces the same rule over every pushed range.

Both hooks start Python through ``.githooks/molt-hook-launch.sh``.

Why not ``core.hooksPath=.githooks``? That would ALSO enable ``.githooks/pre-commit``
(a project type-check with pre-existing diagnostics), which would block every
commit for every agent. So this installer wires ONLY the managed hooks, straight
into ``<git-common-dir>/hooks/``.

Idempotent and non-destructive, per hook:
  * Already-current Molt hook  -> no-op.
  * Stale Molt hook            -> refreshed from source.
  * Foreign (non-Molt) hook    -> preserved as ``<hook>.local`` and CHAINED
                                  (our hook runs it first; if it fails, the
                                  Git operation is blocked before ours runs).
  * No existing hook           -> installed.

Usage:
  install_git_hooks.py              install / refresh every managed hook
  install_git_hooks.py --check      exit 1 if any hook is not installed/current
  install_git_hooks.py --uninstall  remove our hooks (restore chained foreign ones)
"""

from __future__ import annotations

import argparse
import stat
from dataclasses import dataclass
import sys
from pathlib import Path

try:
    from tools.command_execution import CommandExecutor
except ModuleNotFoundError:  # pragma: no cover - direct tools/ execution
    from command_execution import CommandExecutor

_COMMANDS = CommandExecutor.for_file(__file__)

REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class ManagedHook:
    name: str
    marker: str
    purpose: str

    @property
    def source(self) -> Path:
        return HOOKS_SOURCE_DIR / self.name


HOOKS_SOURCE_DIR = REPO_ROOT / ".githooks"
HOOKS: tuple[ManagedHook, ...] = (
    ManagedHook("pre-push", "molt-drift-gate-hook", "drift gate"),
    ManagedHook(
        "commit-msg", "molt-commit-attribution-hook", "commit attribution policy"
    ),
)


def _common_hooks_dir(repo_root: Path = REPO_ROOT) -> Path:
    out = _COMMANDS.run(
        ["git", "rev-parse", "--git-common-dir"],
        cwd=str(repo_root),
        capture_output=True,
        text=True,
        encoding="utf-8",
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


def _is_molt_hook(text: str, hook: ManagedHook) -> bool:
    return hook.marker in text


def _make_executable(path: Path) -> None:
    mode = path.stat().st_mode
    path.chmod(mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _chained_wrapper(source_text: str, hook: ManagedHook) -> str:
    # A Molt hook that first runs a preserved foreign hook (<hook>.local), then
    # ours. Insert the chain call right after the shebang line.
    lines = source_text.splitlines(keepends=True)
    shebang = (
        lines[0] if lines and lines[0].startswith("#!") else "#!/usr/bin/env bash\n"
    )
    rest = "".join(lines[1:]) if lines and lines[0].startswith("#!") else source_text
    chain = (
        f'local_hook="$(dirname "$0")/{hook.name}.local"\n'
        'if [ -x "$local_hook" ]; then "$local_hook" "$@" || exit $?; fi\n'
    )
    return f"{shebang}{chain}{rest}"


def _install_hook(
    hook: ManagedHook, *, check: bool, uninstall: bool, repo_root: Path
) -> int:
    source = hook.source
    if not source.exists():
        print(f"{hook.purpose} source missing: {source}", file=sys.stderr)
        return 2
    hooks = _common_hooks_dir(repo_root)
    target = hooks / hook.name
    preserved = hooks / f"{hook.name}.local"
    source_text = _read(source)

    if uninstall:
        if target.exists() and _is_molt_hook(_read(target), hook):
            target.unlink()
            if preserved.exists():
                preserved.replace(target)
                print(f"removed {hook.purpose}; restored preserved {preserved.name}")
            else:
                print(f"removed {hook.purpose} {hook.name} hook")
        else:
            print(f"no Molt {hook.purpose} installed; nothing to uninstall")
        return 0

    existing = _read(target) if target.exists() else ""
    want = source_text
    if preserved.exists() or (target.exists() and not _is_molt_hook(existing, hook)):
        # Foreign hook present -> we will chain it; the installed content wraps source.
        want = _chained_wrapper(source_text, hook)

    current = existing if _is_molt_hook(existing, hook) else ""
    if current == want and target.exists():
        print(f"{hook.name} {hook.purpose}: up to date ({target})")
        return 0

    if check:
        state = (
            "MISSING"
            if not target.exists()
            else (
                "FOREIGN (uninstalled)"
                if not _is_molt_hook(existing, hook)
                else "OUTDATED"
            )
        )
        print(
            f"{hook.name} {hook.purpose}: {state} at {target} — run: python tools/install_git_hooks.py"
        )
        return 1

    hooks.mkdir(parents=True, exist_ok=True)
    if target.exists() and not _is_molt_hook(existing, hook):
        target.replace(preserved)
        _make_executable(preserved)
        print(f"preserved existing {hook.name} hook -> {preserved} (chained)")

    target.write_text(want, encoding="utf-8", newline="\n")
    _make_executable(target)
    # Belt-and-suspenders: do NOT let core.hooksPath shadow us into the broken pre-commit.
    hp = _COMMANDS.run(
        ["git", "config", "--get", "core.hooksPath"],
        cwd=str(repo_root),
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()
    note = ""
    # A hooksPath that resolves to this same directory (e.g. through a
    # symlinked checkout path) shadows nothing.
    if hp and (repo_root / hp).resolve() != hooks.resolve():
        note = (
            f"\n  WARNING: core.hooksPath={hp} is set — it shadows {target}. "
            "Unset it (git config --unset core.hooksPath) so the managed hooks fire "
            "and the pre-existing pre-commit type-check stays off."
        )
    print(f"installed {hook.name} {hook.purpose} -> {target}{note}")
    return 0


def install(
    *,
    check: bool,
    uninstall: bool,
    repo_root: Path = REPO_ROOT,
    hooks: tuple[ManagedHook, ...] = HOOKS,
) -> int:
    """Install, check, or uninstall every managed hook; return the worst status."""
    return max(
        _install_hook(hook, check=check, uninstall=uninstall, repo_root=repo_root)
        for hook in hooks
    )


def main() -> int:
    ap = argparse.ArgumentParser(description="Install Molt's managed Git hooks.")
    ap.add_argument(
        "--check", action="store_true", help="exit 1 if not installed/current"
    )
    ap.add_argument("--uninstall", action="store_true", help="remove the managed hooks")
    args = ap.parse_args()
    return install(check=args.check, uninstall=args.uninstall)


if __name__ == "__main__":
    raise SystemExit(main())
