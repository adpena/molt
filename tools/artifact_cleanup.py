#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Sequence

REPO_ROOT = Path(__file__).resolve().parents[1]

try:
    from tools import harness_memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct script import from tools/
    import harness_memory_guard  # type: ignore

DEFAULT_PATHS: tuple[str, ...] = (
    ".hypothesis/",
    ".molt_cache/",
    ".molt_cache-*/",
    ".pytest_cache/",
    ".ruff_cache/",
    ".uv-cache/",
    ".uv-cache-*/",
    ".mypy_cache/",
    "bench/results/",
    "bench/friends/repos/",
    "bench/scoreboard/host_calibration/",
    "bin/",
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
    "main_stub.c",
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
    "node_modules/",
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


def _guarded_dev_cleanup_process(
    repo_root: Path,
    cmd: Sequence[str],
    *,
    capture_output: bool = False,
) -> harness_memory_guard.GuardedCompletedProcess:
    env = harness_memory_guard.canonical_harness_env(None, repo_root=repo_root)
    return harness_memory_guard.guarded_completed_process(
        list(cmd),
        prefix="MOLT_DEV_CLEANUP",
        cwd=repo_root,
        env=env,
        capture_output=capture_output,
        text=True,
    )


def run_process_sentinel(
    repo_root: Path,
    *,
    capture_output: bool = False,
) -> harness_memory_guard.GuardedCompletedProcess:
    cmd = [
        sys.executable,
        str(repo_root / "tools" / "process_sentinel.py"),
        "--once",
        "--kill-all",
    ]
    if capture_output:
        cmd.append("--json")
    return _guarded_dev_cleanup_process(
        repo_root,
        cmd,
        capture_output=capture_output,
    )


def run_git_clean(
    repo_root: Path,
    *,
    apply: bool,
    pathspecs: Sequence[str],
    capture_output: bool = False,
) -> harness_memory_guard.GuardedCompletedProcess:
    cmd = build_git_clean_command(apply=apply, pathspecs=pathspecs)
    return _guarded_dev_cleanup_process(
        repo_root,
        cmd,
        capture_output=capture_output,
    )


def _git_clean_entries(stdout: str) -> list[dict[str, str]]:
    entries: list[dict[str, str]] = []
    for line in stdout.splitlines():
        if line.startswith("Would remove "):
            entries.append(
                {"action": "would_remove", "path": line.removeprefix("Would remove ")}
            )
        elif line.startswith("Removing "):
            entries.append(
                {"action": "removed", "path": line.removeprefix("Removing ")}
            )
        elif line:
            entries.append({"action": "output", "line": line})
    return entries


def _json_lines(text: str) -> list[dict[str, object]]:
    events: list[dict[str, object]] = []
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        decoded = json.loads(line)
        if not isinstance(decoded, dict):
            raise ValueError("expected JSON object per process sentinel line")
        events.append(decoded)
    return events


def _emit_json(payload: dict[str, object]) -> None:
    print(json.dumps(payload, indent=2, sort_keys=True))


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
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit a machine-readable cleanup report.",
    )
    parser.add_argument(
        "--verbose",
        action="store_true",
        help="Include command/stdout/stderr details in JSON output.",
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
        if args.json:
            _emit_json(
                {
                    "command": "artifact_cleanup",
                    "status": "ok",
                    "data": {
                        "default_pathspecs": list(default_pathspecs()),
                        "stateful_pathspecs": list(stateful_pathspecs()),
                    },
                }
            )
        else:
            print("# default artifact cleanup pathspecs")
            for pathspec in default_pathspecs():
                print(pathspec)
            print("# stateful pathspecs intentionally excluded")
            for pathspec in stateful_pathspecs():
                print(pathspec)
        return 0

    mode = "apply" if args.apply else "dry-run"
    if not args.json:
        print(f"artifact_cleanup.mode={mode}")
        print(f"artifact_cleanup.repo_root={repo_root}")
    sentinel_result: harness_memory_guard.GuardedCompletedProcess | None = None
    if args.kill_processes:
        sentinel_result = run_process_sentinel(repo_root, capture_output=args.json)
        if sentinel_result.returncode not in {0, 1}:
            if args.json:
                _emit_json(
                    {
                        "command": "artifact_cleanup",
                        "status": "error",
                        "data": {
                            "mode": mode,
                            "repo_root": str(repo_root),
                            "pathspecs": pathspecs,
                            "sentinel_returncode": sentinel_result.returncode,
                        },
                        "errors": ["process sentinel failed before artifact cleanup"],
                    }
                )
            return sentinel_result.returncode
    sentinel_events: list[dict[str, object]] = []
    if sentinel_result is not None and args.json:
        try:
            sentinel_events = _json_lines(sentinel_result.stdout or "")
        except ValueError as exc:
            _emit_json(
                {
                    "command": "artifact_cleanup",
                    "status": "error",
                    "data": {
                        "mode": mode,
                        "repo_root": str(repo_root),
                        "pathspecs": pathspecs,
                        "sentinel_returncode": sentinel_result.returncode,
                    },
                    "errors": [f"process sentinel emitted malformed JSON: {exc}"],
                }
            )
            return 2
        if sentinel_result.returncode == 1 and not sentinel_events:
            _emit_json(
                {
                    "command": "artifact_cleanup",
                    "status": "error",
                    "data": {
                        "mode": mode,
                        "repo_root": str(repo_root),
                        "pathspecs": pathspecs,
                        "sentinel_returncode": sentinel_result.returncode,
                    },
                    "errors": [
                        "process sentinel reported a violation but emitted no JSON events"
                    ],
                }
            )
            return 2
    result = run_git_clean(
        repo_root,
        apply=args.apply,
        pathspecs=pathspecs,
        capture_output=args.json,
    )
    if args.json:
        stdout = result.stdout or ""
        stderr = result.stderr or ""
        data: dict[str, object] = {
            "mode": mode,
            "repo_root": str(repo_root),
            "pathspecs": pathspecs,
            "entries": _git_clean_entries(stdout),
            "returncode": result.returncode,
        }
        if sentinel_result is not None:
            data["sentinel_returncode"] = sentinel_result.returncode
            if sentinel_events:
                data["sentinel_events"] = sentinel_events
        if args.verbose:
            data["stdout"] = stdout.splitlines()
            data["stderr"] = stderr.splitlines()
            data["command"] = build_git_clean_command(
                apply=args.apply,
                pathspecs=pathspecs,
            )
        _emit_json(
            {
                "command": "artifact_cleanup",
                "status": "ok" if result.returncode == 0 else "error",
                "data": data,
                "errors": stderr.splitlines() if result.returncode else [],
            }
        )
    return result.returncode


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
