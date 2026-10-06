"""Project one non-incremental Cargo target into CI and the cache actions.

``configure`` (the default) selects the target directory before the restore.
``prune`` runs before the save and drops this workspace's own artifacts:
a fresh checkout gives every workspace source a new mtime, so Cargo rebuilds
those crates on every run, and caching them only costs transfer and quota.
Dependency artifacts are keyed by content and stay reusable across commits.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
from collections.abc import Mapping
from pathlib import Path

# Outside the checkout, so verified ephemeral custody admits it, and stable
# across runs, because actions/cache folds its paths into the cache version.
DEFAULT_TARGET_NAME = "molt-cargo-target"
TARGET_ENV = "MOLT_CARGO_CACHE_TARGET"

# Cargo names unit artifacts ``<crate>-<16 hex metadata hash>[.<ext>...]``.
_UNIT_ARTIFACT = re.compile(r"(?P<stem>.+)-[0-9a-f]{16}(?:\..*)?")
_UNIT_DIRECTORIES = ("deps", "build", ".fingerprint", "examples")


def configure_cargo_cache(environ: Mapping[str, str]) -> Path:
    workspace = Path(environ["GITHUB_WORKSPACE"]).resolve()
    raw_target = environ.get("CARGO_TARGET_DIR") or str(
        Path(environ["RUNNER_TEMP"]) / DEFAULT_TARGET_NAME
    )
    if any(character in raw_target for character in "\r\n\0"):
        raise ValueError("Cargo cache target contains a control character")
    target = Path(raw_target)
    if not target.is_absolute():
        target = workspace / target
    target = target.resolve()
    target.mkdir(parents=True, exist_ok=True)
    with Path(environ["GITHUB_ENV"]).open("a", encoding="utf-8") as github_env:
        github_env.write(
            f"CARGO_TARGET_DIR={target}\nCARGO_INCREMENTAL=0\n{TARGET_ENV}={target}\n"
        )
    with Path(environ["GITHUB_OUTPUT"]).open("a", encoding="utf-8") as github_output:
        github_output.write(f"target-dir={target}\n")
    return target


def workspace_crate_names(metadata: Mapping[str, object]) -> frozenset[str]:
    """Normalized package and target names of the workspace's own crates."""
    packages = metadata.get("packages")
    if not isinstance(packages, list):
        raise ValueError("cargo metadata has no package list")
    names: set[str] = set()
    for package in packages:
        names.add(str(package["name"]).replace("-", "_"))
        for target in package.get("targets", ()):
            # Every build script is named build-script-build; package names
            # already cover their build/ and .fingerprint/ units.
            if "custom-build" in target.get("kind", ()):
                continue
            names.add(str(target["name"]).replace("-", "_"))
    return frozenset(names)


def _is_workspace_artifact(name: str, workspace: frozenset[str]) -> bool:
    # Most units carry Cargo's metadata hash; cdylib, bin and uplifted outputs
    # in deps/ may not, so fall back to the name before its extensions.
    match = _UNIT_ARTIFACT.fullmatch(name)
    stem = (match.group("stem") if match else name.split(".", 1)[0]).replace("-", "_")
    return stem in workspace or (stem.startswith("lib") and stem[3:] in workspace)


def _profile_directories(target: Path) -> list[Path]:
    # <target>/<profile>/ for host builds and <target>/<triple>/<profile>/ for
    # cross builds; Cargo marks each profile with a .fingerprint directory.
    candidates: list[Path] = []
    for child in target.iterdir():
        if child.is_dir() and not child.is_symlink():
            candidates.append(child)
            candidates.extend(
                nested
                for nested in child.iterdir()
                if nested.is_dir() and not nested.is_symlink()
            )
    return [path for path in candidates if (path / ".fingerprint").is_dir()]


def prune_workspace_artifacts(target: Path, workspace: frozenset[str]) -> int:
    """Delete this workspace's unit artifacts; returns the removed entry count."""
    removed = 0
    for profile in _profile_directories(target):
        for entry in profile.iterdir():
            # Top-level files are final workspace outputs (binaries, staticlibs
            # and their dep-info); Cargo re-links them from the unit artifacts.
            if entry.is_file() or entry.is_symlink():
                entry.unlink()
                removed += 1
        if (profile / "incremental").is_dir():
            shutil.rmtree(profile / "incremental")
            removed += 1
        for directory in _UNIT_DIRECTORIES:
            root = profile / directory
            if not root.is_dir():
                continue
            for entry in root.iterdir():
                if not _is_workspace_artifact(entry.name, workspace):
                    continue
                if entry.is_dir() and not entry.is_symlink():
                    shutil.rmtree(entry)
                else:
                    entry.unlink()
                removed += 1
    return removed


def _cargo_metadata(workspace: Path) -> Mapping[str, object]:
    completed = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--locked"],
        cwd=workspace,
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
        check=True,
    )
    return json.loads(completed.stdout)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "action", nargs="?", choices=("configure", "prune"), default="configure"
    )
    args = parser.parse_args(argv)
    if args.action == "configure":
        target = configure_cargo_cache(os.environ)
        print(f"Cargo cache target: {target}; incremental compilation disabled")
        return 0
    target = Path(os.environ[TARGET_ENV])
    if not target.is_dir():
        print(f"Cargo cache target {target} is absent; nothing to prune")
        return 0
    workspace = workspace_crate_names(
        _cargo_metadata(Path(os.environ["GITHUB_WORKSPACE"]))
    )
    removed = prune_workspace_artifacts(target, workspace)
    print(f"Pruned {removed} workspace artifacts from {target}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
