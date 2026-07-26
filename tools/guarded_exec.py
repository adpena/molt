#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import sys
from collections.abc import Mapping
from pathlib import Path

try:
    from tools import harness_memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct script execution
    import harness_memory_guard


ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

from molt.llvm_toolchain import (  # noqa: E402
    LlvmToolchainConfigError,
    mlir_toolchain_environment,
)


def _timeout_from_env(name: str | None, env: Mapping[str, str]) -> float | None:
    if not name:
        return None
    raw = env.get(name, "").strip()
    if not raw:
        return None
    try:
        parsed = float(raw)
    except ValueError:
        return None
    return parsed if parsed > 0 else None


def _cargo_args(command: list[str]) -> list[str] | None:
    if not command:
        return None
    exe = Path(command[0]).name.lower()
    if exe in {"cargo", "cargo.exe"}:
        return command[1:]
    return None


def _cargo_args_before_passthrough(args: list[str]) -> list[str]:
    try:
        stop = args.index("--")
    except ValueError:
        return args
    return args[:stop]


def _cargo_packages(args: list[str]) -> set[str]:
    packages: set[str] = set()
    scan = _cargo_args_before_passthrough(args)
    i = 0
    while i < len(scan):
        arg = scan[i]
        if arg in {"-p", "--package"}:
            if i + 1 < len(scan):
                packages.add(scan[i + 1])
                i += 2
                continue
        elif arg.startswith("--package="):
            packages.add(arg.split("=", 1)[1])
        elif arg.startswith("-p") and len(arg) > 2:
            packages.add(arg[2:])
        i += 1
    return packages


def _cargo_feature_tokens(args: list[str]) -> set[str]:
    features: set[str] = set()
    scan = _cargo_args_before_passthrough(args)
    i = 0
    while i < len(scan):
        arg = scan[i]
        if arg == "--all-features":
            features.add("*")
        elif arg in {"--features", "-F"}:
            if i + 1 < len(scan):
                features.update(_split_feature_arg(scan[i + 1]))
                i += 2
                continue
        elif arg.startswith("--features="):
            features.update(_split_feature_arg(arg.split("=", 1)[1]))
        i += 1
    return features


def _split_feature_arg(raw: str) -> set[str]:
    return {part for part in raw.replace(",", " ").split() if part}


def _cargo_requests_backend_llvm(command: list[str]) -> bool:
    args = _cargo_args(command)
    if args is None:
        return False
    packages = _cargo_packages(args)
    if packages and "molt-backend" not in packages:
        return False
    features = _cargo_feature_tokens(args)
    return "*" in features or "llvm" in features


def _project_toolchain_environment(
    command: list[str], env: Mapping[str, str]
) -> tuple[dict[str, str], str | None]:
    if not _cargo_requests_backend_llvm(command):
        return dict(env), None
    try:
        return mlir_toolchain_environment(ROOT, environ=dict(env)), None
    except LlvmToolchainConfigError as exc:
        return dict(env), str(exc)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a command under Molt's canonical harness memory guard."
    )
    parser.add_argument("--prefix", default="MOLT")
    parser.add_argument("--cwd", type=Path, default=ROOT)
    parser.add_argument("--timeout", type=float, default=None)
    parser.add_argument("--timeout-env", default=None)
    parser.add_argument("--metrics-json", type=Path)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("command is required after --")

    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=ROOT)
    env, preflight_error = _project_toolchain_environment(command, env)
    if preflight_error is not None:
        print(
            "guarded_exec preflight: backend LLVM toolchain is not ready.",
            file=sys.stderr,
        )
        print(preflight_error, file=sys.stderr)
        return 2

    context = harness_memory_guard.HarnessExecutionContext.from_env(
        args.prefix,
        env,
        repo_root=ROOT,
    )
    timeout = harness_memory_guard.timeout_from_env(
        args.prefix,
        env,
        explicit=args.timeout,
        default=_timeout_from_env(args.timeout_env, env),
    )
    result = context.run(
        command,
        cwd=args.cwd,
        env=env,
        capture_output=False,
        timeout=timeout,
    )
    if args.metrics_json is not None:
        peak = getattr(result, "peak", None)
        peak_total = getattr(result, "peak_total", None)
        metrics = {
            "schema": "molt.guarded-command-metrics.v1",
            "returncode": int(result.returncode),
            "duration_seconds": getattr(result, "elapsed_s", None),
            "peak_process_rss_bytes": (
                int(peak.rss_kb) * 1024
                if peak is not None and getattr(peak, "rss_kb", None) is not None
                else None
            ),
            "peak_tree_rss_bytes": (
                int(peak_total.rss_kb) * 1024
                if peak_total is not None
                and getattr(peak_total, "rss_kb", None) is not None
                else None
            ),
            "peak_job_commit_bytes": getattr(
                result,
                "peak_job_commit_bytes",
                None,
            ),
            "windows_job_cleanup": harness_memory_guard.memory_guard.windows_job_cleanup_payload(
                getattr(result, "windows_job_cleanup", None)
            ),
        }
        metrics_path = args.metrics_json.resolve()
        metrics_path.parent.mkdir(parents=True, exist_ok=True)
        temporary = metrics_path.with_name(f".{metrics_path.name}.{os.getpid()}.tmp")
        temporary.write_text(
            json.dumps(metrics, sort_keys=True) + "\n", encoding="utf-8"
        )
        temporary.replace(metrics_path)
    if result.stderr:
        sys.stderr.write(str(result.stderr))
    profile_path = harness_memory_guard.command_profile_log_path(env)
    elapsed_s = getattr(result, "elapsed_s", None)
    elapsed = "unknown" if elapsed_s is None else f"{elapsed_s:.2f}s"
    print(
        "guarded_exec: "
        f"elapsed={elapsed} returncode={result.returncode} "
        f"profile={profile_path}",
        file=sys.stderr,
    )
    return int(result.returncode)


if __name__ == "__main__":
    raise SystemExit(main())
