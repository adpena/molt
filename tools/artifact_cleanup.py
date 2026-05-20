#!/usr/bin/env python3
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]

DEFAULT_PATHS: tuple[str, ...] = (
    ".hypothesis/",
    ".molt_cache/",
    ".pytest_cache/",
    ".ruff_cache/",
    ".uv-cache/",
    "bench/results/",
    "build/",
    "deploy/browser/simd-ops-rs/target/",
    "deploy/browser/simd-ops-zig/libsimd.a",
    "deploy/cloudflare/node_modules/",
    "dist/",
    "examples/cloudflare-demo/dist/",
    "formal/lean/.lake/build/",
    "hello_molt",
    "libsimd.a",
    "logs/",
    "models/paddleocr/korean_rec/.cache/",
    "models/paddleocr/unified_mobile_rec/.cache/",
    "molt_runtime.*.rcgu.o",
    "output.o",
    "output.wasm",
    "output_linked.cwasm",
    "output_linked.wasm",
    "output_optimized.wasm",
    "output_treeshaken.wasm",
    "runtime/molt-backend-mlir/target/",
    "runtime/molt-backend/.molt_cache/",
    "runtime/molt-backend/fuzz/target/",
    "runtime/molt-backend/tmp/",
    "runtime/molt-runtime/fuzz/target/",
    "runtime/target/",
    "src/molt/__pycache__/",
    "src/molt.egg-info/",
    "target/",
    "target-*",
    "tests/__pycache__/",
    "tests/harness/reports/",
    "tests/tools/__pycache__/",
    "tmp/",
    "tools/__pycache__/",
    "type_facts.json",
    "wasm/molt_runtime.wasm",
    "wasm/molt_runtime_reloc.wasm",
)

STATEFUL_PATHS: tuple[str, ...] = (
    ".omx/",
    ".venv/",
    "runtime/molt-backend/fuzz/corpus/",
    "runtime/molt-runtime/fuzz/corpus/",
    "tests/e2e/test_images/",
    "tests/harness/corpus/molt_adapted/",
    "third_party/",
)


def default_pathspecs() -> tuple[str, ...]:
    return DEFAULT_PATHS


def stateful_pathspecs() -> tuple[str, ...]:
    return STATEFUL_PATHS


def validate_repo_root(repo_root: Path) -> None:
    expected_tool = repo_root / "tools" / "artifact_cleanup.py"
    try:
        same_tool = expected_tool.resolve() == Path(__file__).resolve()
    except OSError:
        same_tool = False
    if not same_tool or not (repo_root / "pyproject.toml").is_file():
        raise ValueError(f"{repo_root} is not this Molt checkout")


def _literal_pathspec_key(pathspec: str) -> str:
    raw = pathspec.strip()
    if not raw:
        raise ValueError("extra cleanup pathspec must not be empty")
    if raw.startswith("/") or raw.startswith(":"):
        raise ValueError(f"extra cleanup pathspec must be repo-relative: {pathspec!r}")
    if any(ch in raw for ch in "*?["):
        raise ValueError(f"extra cleanup pathspec must be a literal path: {pathspec!r}")
    parts = [part for part in raw.strip("/").split("/") if part not in {"", "."}]
    if any(part == ".." for part in parts):
        raise ValueError(
            f"extra cleanup pathspec must stay inside the repo: {pathspec!r}"
        )
    return "/".join(parts)


def validate_extra_pathspecs(pathspecs: Sequence[str]) -> None:
    stateful = tuple(
        _literal_pathspec_key(pathspec) for pathspec in stateful_pathspecs()
    )
    for pathspec in pathspecs:
        key = _literal_pathspec_key(pathspec)
        if any(key == blocked or key.startswith(f"{blocked}/") for blocked in stateful):
            raise ValueError(
                f"extra cleanup pathspec targets stateful data: {pathspec!r}"
            )


def build_git_clean_command(*, apply: bool, pathspecs: Sequence[str]) -> list[str]:
    mode = "-fdX" if apply else "-ndX"
    return ["git", "clean", mode, "--", *pathspecs]


def run_process_sentinel(repo_root: Path) -> int:
    cmd = [
        sys.executable,
        str(repo_root / "tools" / "process_sentinel.py"),
        "--once",
        "--kill-all",
    ]
    result = subprocess.run(cmd, cwd=repo_root, text=True)
    if result.returncode not in {0, 1}:
        return result.returncode
    return 0


def run_git_clean(repo_root: Path, *, apply: bool, pathspecs: Sequence[str]) -> int:
    cmd = build_git_clean_command(apply=apply, pathspecs=pathspecs)
    result = subprocess.run(cmd, cwd=repo_root, text=True)
    return result.returncode


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Clean ignored Molt build/test artifacts through a canonical "
            "git-clean pathspec allowlist."
        )
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="Repository root to clean.",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="Delete ignored artifacts. Default is a dry run.",
    )
    parser.add_argument(
        "--kill-processes",
        action="store_true",
        help="Run process_sentinel before cleanup to stop repo-scoped build/test jobs.",
    )
    parser.add_argument(
        "--extra-path",
        action="append",
        default=[],
        help="Additional git-clean pathspec. Still removes ignored files only.",
    )
    parser.add_argument(
        "--list-paths",
        action="store_true",
        help="Print the default cleanup pathspecs and exit.",
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    repo_root = args.repo_root.expanduser().resolve()
    try:
        validate_repo_root(repo_root)
        validate_extra_pathspecs(args.extra_path)
    except ValueError as exc:
        print(f"artifact_cleanup: {exc}", file=sys.stderr)
        return 2
    pathspecs = [*default_pathspecs(), *args.extra_path]

    if args.list_paths:
        print("# default artifact cleanup pathspecs")
        for pathspec in default_pathspecs():
            print(pathspec)
        print("# stateful pathspecs intentionally excluded")
        for pathspec in stateful_pathspecs():
            print(pathspec)
        return 0

    mode = "apply" if args.apply else "dry-run"
    print(f"artifact_cleanup.mode={mode}")
    print(f"artifact_cleanup.repo_root={repo_root}")
    if args.kill_processes:
        rc = run_process_sentinel(repo_root)
        if rc != 0:
            return rc
    return run_git_clean(repo_root, apply=args.apply, pathspecs=pathspecs)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
