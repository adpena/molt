#!/usr/bin/env python3
"""Check that every committed Cargo.lock still satisfies its manifest.

Cargo's --locked refuses a lockfile its manifest has outgrown, but only when
something builds that workspace. The MLIR backend and the fuzz workspaces sit
outside the main workspace, so their lockfiles went stale unnoticed until a
build met them. This check resolves every tracked Cargo.lock with
`cargo metadata --locked`, which fails without writing when a lock is stale.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

if __package__ in (None, ""):
    from import_file import bind_repository_imports
else:
    from tools.import_file import bind_repository_imports

ROOT = bind_repository_imports(__file__)

from molt import process_guard  # noqa: E402
from molt.cargo_execution_policy import cargo_subprocess_environment  # noqa: E402

TIMEOUT_SECONDS = 300


def tracked_lockfiles(root: Path) -> list[Path]:
    result = process_guard.run_completed_command(
        ["git", "ls-files", "-z", "--", "*Cargo.lock", "Cargo.lock"],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
        timeout=60,
        memory_guard_prefix=None,
    )
    return sorted(
        {root / relative for relative in result.stdout.split("\0") if relative}
    )


def stale_lockfiles(root: Path) -> list[str]:
    """Return one diagnostic per lockfile its manifest has outgrown."""
    stale = []
    for lockfile in tracked_lockfiles(root):
        manifest = lockfile.with_name("Cargo.toml")
        command = [
            "cargo",
            "metadata",
            "--format-version",
            "1",
            "--locked",
            "--manifest-path",
            str(manifest),
        ]
        env, _policies = cargo_subprocess_environment(command, None)
        result = process_guard.run_completed_command(
            command,
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=TIMEOUT_SECONDS,
            memory_guard_prefix=None,
        )
        if result.returncode != 0:
            detail = (result.stderr or "").strip().splitlines()
            stale.append(
                f"{lockfile.relative_to(root)}: "
                + (detail[-2] if len(detail) >= 2 else "cargo metadata failed")
            )
    return stale


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    stale = stale_lockfiles(args.root)
    for diagnostic in stale:
        print(f"cargo-locks: {diagnostic}", file=sys.stderr)
    if stale:
        print(
            "cargo-locks: refresh each with `cargo metadata --manifest-path "
            "<dir>/Cargo.toml` (it rewrites only what the manifest requires)",
            file=sys.stderr,
        )
        return 1
    print(f"cargo-locks: OK ({len(tracked_lockfiles(args.root))} lockfiles)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
