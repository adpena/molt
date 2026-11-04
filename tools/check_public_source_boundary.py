#!/usr/bin/env python3
"""Refuse tracked files that belong outside public source (HF-768).

`docs/ROOT_LAYOUT.md` is the contract; this tool is its one enforced list. It
reads the tracked paths (`git ls-files`) and refuses two kinds:

- a path under a local artifact root (logs, scratch, build output, machine-local
  settings, raw benchmark logs and guard traces);
- a repository-root entry outside `ROOT_ENTRIES`. A new root entry is a
  deliberate edit of that set, reviewed with the change that needs it.

It runs in the always-run static gates (`tools/proof_plan.toml`) and the
`public-source-boundary` pre-commit hook. Standard library only, so it runs
before the project environment exists.
"""

from __future__ import annotations

import argparse
from collections.abc import Iterable
from dataclasses import dataclass
from pathlib import Path
import re
import subprocess
import sys


# Local artifact roots and machine-local files (docs/ROOT_LAYOUT.md, "Review
# Rule"). A canonical artifact home is not public repository material.
LOCAL_ARTIFACT_PATTERNS: tuple[re.Pattern[str], ...] = tuple(
    re.compile(pattern)
    for pattern in (
        r"^(?:logs|tmp|target|build|dist)/",
        # Package and Cloudflare local state at any depth: Wrangler account
        # records, Miniflare request geolocation (cf.json), local secrets.
        r"(?:^|/)(?:node_modules|\.wrangler|\.mf)/",
        r"(?:^|/)\.dev\.vars(?:\.[^/]*)?$",
        r"^\.claude/settings\.local\.json$",
        r"^bench/(?:results|scoreboard)/.*\.log$",
        r"^bench/results/.*/memory_guard/",
    )
)

# Every entry the repository root may hold.
ROOT_ENTRIES: frozenset[str] = frozenset(
    {
        # Manifests, lockfiles and toolchain pins.
        "Cargo.toml",
        "Cargo.lock",
        "pyproject.toml",
        "uv.lock",
        ".python-version",
        "rust-toolchain.toml",
        "clippy.toml",
        "deny.toml",
        "molt.capabilities.toml",
        "Makefile.pgo",
        "sitecustomize.py",
        # Project documents.
        "README.md",
        "README_GPU.md",
        "AGENTS.md",
        "CLAUDE.md",
        "CONTRIBUTING.md",
        "CHANGELOG.md",
        "LICENSE",
        "ROADMAP.md",
        "SUPPORTED.md",
        # Repository and agent configuration.
        ".gitattributes",
        ".gitignore",
        ".githooks",
        ".github",
        ".cargo",
        ".claude",
        ".codex",
        ".agents",
        ".mcp.json",
        ".molt",
        ".pre-commit-config.yaml",
        # Source, workspace and maintained data directories.
        "src",
        "runtime",
        "tests",
        "tools",
        "docs",
        "examples",
        "demo",
        "demos",
        "deploy",
        "drivers",
        "bench",
        "ops",
        "packaging",
        "vendor",
        "include",
        "formal",
        "fuzz",
        "wasm",
        "config",
        "collab",
        "models",
    }
)


@dataclass(frozen=True)
class Violation:
    path: str
    reason: str


def boundary_violations(paths: Iterable[str]) -> list[Violation]:
    """Tracked paths that the public-source boundary refuses, in input order."""
    violations: list[Violation] = []
    for path in paths:
        if any(pattern.search(path) for pattern in LOCAL_ARTIFACT_PATTERNS):
            violations.append(Violation(path, "local artifact root"))
        elif path.split("/", 1)[0] not in ROOT_ENTRIES:
            violations.append(Violation(path, "root entry outside ROOT_ENTRIES"))
    return violations


def tracked_paths(repo: Path) -> list[str]:
    result = subprocess.run(
        ["git", "ls-files", "-z"],
        cwd=repo,
        capture_output=True,
        check=False,
        timeout=60,
    )
    if result.returncode != 0:
        raise RuntimeError(
            f"git ls-files failed in {repo} (exit {result.returncode}): "
            + result.stderr.decode("utf-8", "replace").strip()
        )
    return [path for path in result.stdout.decode("utf-8").split("\0") if path]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--repo",
        type=Path,
        default=Path(__file__).resolve().parents[1],
        help="checkout to check (default: this tool's checkout)",
    )
    args = parser.parse_args(argv)
    try:
        violations = boundary_violations(tracked_paths(args.repo))
    except (OSError, RuntimeError, subprocess.TimeoutExpired) as exc:
        print(f"check_public_source_boundary: {exc}", file=sys.stderr)
        return 2
    if not violations:
        return 0
    print(
        "check_public_source_boundary: tracked files outside public source "
        "(docs/ROOT_LAYOUT.md):",
        file=sys.stderr,
    )
    for violation in violations:
        print(f"  {violation.path}: {violation.reason}", file=sys.stderr)
    print(
        "fix: keep logs, scratch and lane evidence outside tracked source and run "
        "`git rm --cached -- <path>`; a new root entry is an edit of ROOT_ENTRIES "
        "in tools/check_public_source_boundary.py",
        file=sys.stderr,
    )
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
