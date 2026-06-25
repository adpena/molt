#!/usr/bin/env python3
from __future__ import annotations

import argparse
import os
import sys
from collections.abc import Mapping
from pathlib import Path

try:
    from tools import harness_memory_guard
except ModuleNotFoundError:  # pragma: no cover - direct script execution
    import harness_memory_guard  # type: ignore


ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "src"
if str(SRC) not in sys.path:
    sys.path.insert(0, str(SRC))

try:
    from molt.cli.toolchain_validation import _llvm_backend_unavailable_message
except ModuleNotFoundError:  # pragma: no cover - source tree corruption
    _llvm_backend_unavailable_message = None  # type: ignore[assignment]


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


def _toolchain_preflight_error(command: list[str]) -> str | None:
    if not _cargo_requests_backend_llvm(command):
        return None
    if _llvm_backend_unavailable_message is None:
        return "Unable to load Molt LLVM toolchain detector from src/molt/cli."
    return _llvm_backend_unavailable_message(ROOT)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description="Run a command under Molt's canonical harness memory guard."
    )
    parser.add_argument("--prefix", default="MOLT")
    parser.add_argument("--cwd", type=Path, default=ROOT)
    parser.add_argument("--timeout", type=float, default=None)
    parser.add_argument("--timeout-env", default=None)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args(argv)
    command = list(args.command)
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("command is required after --")

    preflight_error = _toolchain_preflight_error(command)
    if preflight_error is not None:
        print(
            "guarded_exec preflight: backend LLVM toolchain is not ready.",
            file=sys.stderr,
        )
        print(preflight_error, file=sys.stderr)
        return 2

    env = harness_memory_guard.canonical_harness_env(os.environ, repo_root=ROOT)
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
    if result.stderr:
        sys.stderr.write(result.stderr)
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
