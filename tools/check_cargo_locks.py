#!/usr/bin/env python3
"""Check that every committed Cargo.lock satisfies its manifest and our patches.

Cargo's --locked refuses a lockfile its manifest has outgrown, but only when
something builds that workspace. The MLIR backend and the fuzz workspaces sit
outside the main workspace, so their lockfiles went stale unnoticed until a
build met them. This check resolves every tracked Cargo.lock with
`cargo metadata --locked`, which fails without writing when a lock is stale.

Cargo applies `[patch]` only in the workspace it resolves, so each standalone
workspace must patch the crates we vendor itself. The root `Cargo.toml`
`[patch.crates-io]` table names the vendored crates; a lockfile that resolves
a version Cargo would patch from anywhere else, or patches it to another
directory, fails (HF-378: the MLIR and fuzz workspaces built crates.io
`object` without the ARM64 COFF addend fix).
"""

from __future__ import annotations

import argparse
import re
import sys
import tomllib
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

ROOT = bind_repository_imports(__file__)

from molt import process_guard  # noqa: E402
from molt.cargo_execution_policy import cargo_subprocess_environment  # noqa: E402
from molt.cargo_workspace import require_cargo_workspace_root  # noqa: E402

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


def _read_toml(path: Path) -> dict:
    return tomllib.loads(path.read_text(encoding="utf-8"))


def _crates_io_patch_paths(manifest: Path) -> dict[str, Path]:
    """Crate name -> resolved directory of each path entry in `[patch.crates-io]`."""
    table = _read_toml(manifest).get("patch", {}).get("crates-io", {})
    return {
        spec.get("package", key): (manifest.parent / spec["path"]).resolve()
        for key, spec in table.items()
        if isinstance(spec, dict) and "path" in spec
    }


def _caret_bucket(version: str) -> tuple[int, ...]:
    """Versions in one bucket are caret-compatible, so one `[patch]` covers them."""
    match = re.match(r"(\d+)\.(\d+)\.(\d+)", version)
    if match is None:
        raise ValueError(f"not a semver version: {version!r}")
    major, minor, patch = (int(part) for part in match.groups())
    if major:
        return (major,)
    return (0, minor) if minor else (0, 0, patch)


def vendored_crates(root: Path) -> dict[str, tuple[Path, str]]:
    """The root workspace's vendored crates: name -> (directory, version)."""
    return {
        name: (directory, _read_toml(directory / "Cargo.toml")["package"]["version"])
        for name, directory in _crates_io_patch_paths(root / "Cargo.toml").items()
    }


def unpatched_vendored_crates(root: Path) -> list[str]:
    """One diagnostic per lockfile package that misses a vendored crate's patch."""
    vendored = vendored_crates(root)
    if not vendored:
        return []
    problems = []
    for lockfile in tracked_lockfiles(root):
        relative = lockfile.relative_to(root)
        patched = _crates_io_patch_paths(lockfile.with_name("Cargo.toml"))
        for package in _read_toml(lockfile).get("package", []):
            name, version = package["name"], package["version"]
            entry = vendored.get(name)
            if entry is None or _caret_bucket(version) != _caret_bucket(entry[1]):
                continue
            directory = entry[0]
            vendor = directory.relative_to(root)
            if "source" in package:
                problems.append(
                    f"{relative}: {name} {version} resolves from "
                    f"{package['source']}, not {vendor}"
                )
            elif patched.get(name) != directory:
                problems.append(
                    f"{relative}: {name} {version} is not patched to {vendor}"
                )
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n", 1)[0])
    parser.add_argument("--root", type=Path, default=ROOT)
    args = parser.parse_args(argv)
    try:
        for lockfile in tracked_lockfiles(args.root):
            require_cargo_workspace_root(lockfile.with_name("Cargo.toml"))
    except ValueError as exc:
        print(f"cargo-locks: {exc}", file=sys.stderr)
        return 1
    unpatched = unpatched_vendored_crates(args.root)
    for diagnostic in unpatched:
        print(f"cargo-locks: {diagnostic}", file=sys.stderr)
    if unpatched:
        print(
            "cargo-locks: add the root Cargo.toml [patch.crates-io] entry to that "
            "workspace's manifest, with its path relative to the manifest",
            file=sys.stderr,
        )
        return 1
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
