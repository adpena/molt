#!/usr/bin/env python3
"""Idempotent installer for Molt's managed Git hooks.

Installs each managed hook from ``.githooks/`` into the git *common* hooks dir,
so it fires in the checkout AND every linked worktree:

  * ``commit-msg`` — the commit-attribution policy (no Claude attribution);
                     CI enforces the same rule over every pushed range.

The attribution hook starts Python through ``.githooks/molt-hook-launch.sh``.

It also registers the managed merge drivers in the common Git config, which
every worktree shares:
  * ``molt-ledger`` — merges docs/agent/V1_HANDOFF_FINDINGS.md by finding ID
                      (``.gitattributes`` names it; ``.githooks/molt-ledger-merge``
                      runs ``tools/ledger_merge.py``).

Why not ``core.hooksPath=.githooks``? That would ALSO enable ``.githooks/pre-commit``
(a project type-check with pre-existing diagnostics), which would block every
commit for every agent. So this installer wires ONLY the managed hooks, straight
into ``<git-common-dir>/hooks/``.

The obsolete Molt pre-push hook is retired, restoring any preserved foreign
hook. Drift maintenance stays explicit; integrate validates its selected commit
and CI checks generated projections. No replacement push hook is installed.

Idempotent and non-destructive, per active hook:
  * Already-current Molt hook  -> no-op.
  * Stale Molt hook            -> refreshed from source.
  * Foreign (non-Molt) hook    -> preserved as ``<hook>.local`` and CHAINED
                                  (our hook runs it first; if it fails, the
                                  Git operation is blocked before ours runs).
  * No existing hook           -> installed.

Usage:
  install_git_hooks.py              install / refresh active hooks and retire obsolete hooks
  install_git_hooks.py --check      exit 1 if active hooks are not current or retirement is pending
  install_git_hooks.py --uninstall  remove our hooks (restore chained foreign ones)
"""

from __future__ import annotations

import argparse
import stat
from dataclasses import dataclass
import sys
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from tools.command_execution import CommandExecutor  # noqa: E402

_COMMANDS = CommandExecutor.for_file(__file__)

REPO_ROOT = Path(__file__).resolve().parent.parent


@dataclass(frozen=True)
class ManagedHook:
    name: str
    marker: str
    purpose: str
    retired: bool = False

    @property
    def source(self) -> Path:
        return HOOKS_SOURCE_DIR / self.name


HOOKS_SOURCE_DIR = REPO_ROOT / ".githooks"
HOOKS: tuple[ManagedHook, ...] = (
    ManagedHook("pre-push", "molt-drift-gate-hook", "retired drift gate", retired=True),
    ManagedHook(
        "commit-msg", "molt-commit-attribution-hook", "commit attribution policy"
    ),
)


@dataclass(frozen=True)
class ManagedMergeDriver:
    name: str
    description: str
    script: str

    @property
    def settings(self) -> dict[str, str]:
        # Git runs a driver through the shell; the script finds its own
        # worktree, so one shared config entry serves every worktree.
        return {
            f"merge.{self.name}.name": self.description,
            f"merge.{self.name}.driver": (
                f'sh "$(git rev-parse --show-toplevel)/{self.script}" %O %A %B'
            ),
        }


MERGE_DRIVERS: tuple[ManagedMergeDriver, ...] = (
    ManagedMergeDriver(
        "molt-ledger",
        "Molt ledger rows by finding ID",
        ".githooks/molt-ledger-merge",
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


def _remove_owned_hook(hook: ManagedHook, *, check: bool, repo_root: Path) -> int:
    """Retire our hook without overwriting an independently installed hook."""
    hooks = _common_hooks_dir(repo_root)
    target = hooks / hook.name
    preserved = hooks / f"{hook.name}.local"
    target_present = target.exists() or target.is_symlink()
    owned = (
        target.is_file()
        and not target.is_symlink()
        and _is_molt_hook(_read(target), hook)
    )
    if target_present and not owned:
        print(f"{hook.name}: foreign hook preserved; no Molt hook to remove")
        return 0
    preserved_present = preserved.exists() or preserved.is_symlink()
    if not owned and not preserved_present:
        print(f"{hook.name}: no Molt hook or preserved hook to retire")
        return 0
    if check:
        print(
            f"{hook.name}: retirement pending — run: python tools/install_git_hooks.py"
        )
        return 1
    if preserved_present:
        if preserved.is_dir() and not preserved.is_symlink():
            print(
                f"{hook.name}: preserved hook is a directory; left unchanged",
                file=sys.stderr,
            )
            return 2
        # Atomic replacement retains the foreign hook's exact bytes and mode.
        # An orphaned .local is restored only when the active path is absent.
        preserved.replace(target)
        print(f"removed {hook.purpose}; restored preserved {preserved.name}")
    else:
        target.unlink()
        print(f"removed {hook.purpose} {hook.name} hook")
    return 0


def _install_hook(
    hook: ManagedHook, *, check: bool, uninstall: bool, repo_root: Path
) -> int:
    if hook.retired or uninstall:
        return _remove_owned_hook(hook, check=check, repo_root=repo_root)
    source = hook.source
    if not source.exists():
        print(f"{hook.purpose} source missing: {source}", file=sys.stderr)
        return 2
    hooks = _common_hooks_dir(repo_root)
    target = hooks / hook.name
    preserved = hooks / f"{hook.name}.local"
    source_text = _read(source)

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
        if preserved.exists() or preserved.is_symlink():
            print(
                f"{hook.name}: foreign hook and preserved hook both exist; left unchanged",
                file=sys.stderr,
            )
            return 2
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


def _git_config_get(key: str, repo_root: Path) -> str | None:
    completed = _COMMANDS.run(
        ["git", "config", "--get", key],
        cwd=str(repo_root),
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return completed.stdout.rstrip("\n") if completed.returncode == 0 else None


def _install_merge_driver(
    driver: ManagedMergeDriver, *, check: bool, uninstall: bool, repo_root: Path
) -> int:
    current = {key: _git_config_get(key, repo_root) for key in driver.settings}
    if uninstall:
        for key, value in current.items():
            if value is not None:
                _COMMANDS.run(
                    ["git", "config", "--unset", key],
                    cwd=str(repo_root),
                    capture_output=True,
                    text=True,
                    encoding="utf-8",
                )
        print(f"merge driver {driver.name}: removed")
        return 0
    if current == driver.settings:
        print(f"merge driver {driver.name}: up to date")
        return 0
    if check:
        state = "MISSING" if all(v is None for v in current.values()) else "OUTDATED"
        print(
            f"merge driver {driver.name}: {state} — run: python tools/install_git_hooks.py"
        )
        return 1
    for key, value in driver.settings.items():
        _COMMANDS.run(
            ["git", "config", key, value],
            cwd=str(repo_root),
            capture_output=True,
            text=True,
            encoding="utf-8",
            check=True,
        )
    print(f"installed merge driver {driver.name} ({driver.description})")
    return 0


def install(
    *,
    check: bool,
    uninstall: bool,
    repo_root: Path = REPO_ROOT,
    hooks: tuple[ManagedHook, ...] = HOOKS,
    merge_drivers: tuple[ManagedMergeDriver, ...] = MERGE_DRIVERS,
) -> int:
    """Install, check, or uninstall every managed hook and merge driver.

    Returns the worst status.
    """
    return max(
        [
            *(
                _install_hook(
                    hook, check=check, uninstall=uninstall, repo_root=repo_root
                )
                for hook in hooks
            ),
            *(
                _install_merge_driver(
                    driver, check=check, uninstall=uninstall, repo_root=repo_root
                )
                for driver in merge_drivers
            ),
        ],
        default=0,
    )


def main() -> int:
    ap = argparse.ArgumentParser(description="Install Molt's managed Git hooks.")
    ap.add_argument(
        "--check",
        action="store_true",
        help="exit 1 if active hooks are not current or retirement is pending",
    )
    ap.add_argument("--uninstall", action="store_true", help="remove the managed hooks")
    args = ap.parse_args()
    return install(check=args.check, uninstall=args.uninstall)


if __name__ == "__main__":
    raise SystemExit(main())
