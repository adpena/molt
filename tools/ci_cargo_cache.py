"""Project one non-incremental Cargo target into CI and the cache actions.

``configure`` (the default) selects the target directory before the restore.
``prune`` runs before the save and drops this workspace's own artifacts:
a fresh checkout gives every workspace source a new mtime, so Cargo rebuilds
those crates on every run, and caching them only costs transfer and quota.
Dependency artifacts are keyed by content and stay reusable across commits.
``retain`` deletes main's superseded Cargo cache entries. Every save gets a
new immutable key ending in its run ID and attempt, and restores take the
newest entry for the same namespace, OS, architecture and toolchain token, so
the older entries only push useful caches out of the 10 GB quota.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import tomllib
from collections.abc import Mapping
from pathlib import Path


# Outside the checkout, so verified ephemeral custody admits it, and stable
# across runs, because actions/cache folds its paths into the cache version.
DEFAULT_TARGET_NAME = "molt-cargo-target"
TARGET_ENV = "MOLT_CARGO_CACHE_TARGET"

# setup-project saves ``cargo-<namespace>-<os>-<arch>-<toolchain token>-
# <sha256 of the dependency inputs>-<run id>-<run attempt>``.
_CACHE_KEY = re.compile(
    r"cargo-(?P<group>.+)-[0-9a-f]{64}-(?P<run>[0-9]+)-(?P<attempt>[0-9]+)"
)
MAIN_REF = "refs/heads/main"


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


def workspace_clean_commands(workspace: Path, target: Path) -> list[list[str]]:
    """The ``cargo clean --workspace`` runs that drop this workspace's units.

    Cargo's build-directory layout is not a stable interface, so Cargo removes
    the units itself. One run cleans one profile: every profile the workspace
    manifest declares, with and without ``--target``, for the host and every
    target ``rust-toolchain.toml`` installs. Dependency units stay.
    https://doc.rust-lang.org/cargo/commands/cargo-clean.html
    """
    manifest = tomllib.loads((workspace / "Cargo.toml").read_text(encoding="utf-8"))
    declared = manifest.get("profile", {})
    if not isinstance(declared, dict):
        raise ValueError("Cargo.toml [profile] is not a table")
    toolchain = tomllib.loads(
        (workspace / "rust-toolchain.toml").read_text(encoding="utf-8")
    )["toolchain"]
    targets = ["host-tuple", *toolchain.get("targets", ())]
    base = ["cargo", "clean", "--workspace", "--locked", "--target-dir", str(target)]
    commands: list[list[str]] = []
    for profile in sorted({"dev", "release", *declared}):
        commands.append([*base, "--profile", profile])
        commands.append(
            [
                *base,
                "--profile",
                profile,
                *(a for t in targets for a in ("--target", t)),
            ]
        )
    return commands


def prune_workspace_artifacts(workspace: Path, target: Path) -> int:
    """Delete this workspace's units with Cargo; returns the Cargo run count."""
    commands = workspace_clean_commands(workspace, target)
    for command in commands:
        subprocess.run(command, cwd=workspace, timeout=300, check=True)
    return len(commands)


def superseded_cache_ids(entries: list[Mapping[str, object]]) -> list[int]:
    """IDs of main Cargo cache entries that a newer entry in their group replaces."""
    newest: dict[str, tuple[str, int]] = {}
    grouped: list[tuple[str, int]] = []
    for entry in entries:
        match = _CACHE_KEY.fullmatch(str(entry["key"]))
        if match is None or entry.get("ref") != MAIN_REF:
            continue
        group = match.group("group")
        cache_id = int(str(entry["id"]))
        rank = (str(entry["createdAt"]), cache_id)
        grouped.append((group, cache_id))
        if group not in newest or rank > newest[group]:
            newest[group] = rank
    return sorted(
        cache_id for group, cache_id in grouped if cache_id != newest[group][1]
    )


def _gh_json(*args: str) -> list[Mapping[str, object]]:
    completed = subprocess.run(
        ["gh", *args],
        capture_output=True,
        text=True,
        encoding="utf-8",
        timeout=300,
        check=True,
    )
    return json.loads(completed.stdout)


def retain_newest_cargo_caches() -> list[int]:
    entries = _gh_json(
        "cache", "list", "--limit", "1000", "--json", "id,key,ref,createdAt"
    )
    superseded = superseded_cache_ids(entries)
    for cache_id in superseded:
        subprocess.run(
            ["gh", "cache", "delete", str(cache_id)],
            timeout=300,
            check=True,
        )
    return superseded


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "action",
        nargs="?",
        choices=("configure", "prune", "retain"),
        default="configure",
    )
    args = parser.parse_args(argv)
    if args.action == "retain":
        deleted = retain_newest_cargo_caches()
        print(f"Deleted {len(deleted)} superseded Cargo cache entries")
        return 0
    if args.action == "configure":
        target = configure_cargo_cache(os.environ)
        print(f"Cargo cache target: {target}; incremental compilation disabled")
        return 0
    target = Path(os.environ[TARGET_ENV])
    if not target.is_dir():
        print(f"Cargo cache target {target} is absent; nothing to prune")
        return 0
    runs = prune_workspace_artifacts(Path(os.environ["GITHUB_WORKSPACE"]), target)
    print(f"Cleaned this workspace's units from {target} in {runs} Cargo runs")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
