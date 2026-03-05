from __future__ import annotations

import argparse
import os
import secrets
import shutil
import subprocess
import sys
import time
from pathlib import Path


DEFAULT_EXT_ROOT = "/Volumes/APDataStore/Molt"
DEFAULT_ENV_FILE = Path("ops/linear/runtime/symphony.env")


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Launch Molt Symphony with external-volume defaults and repository-safe "
            "workspace placement."
        )
    )
    parser.add_argument(
        "workflow_path",
        nargs="?",
        default="WORKFLOW.md",
        help="Path to WORKFLOW.md (default: ./WORKFLOW.md).",
    )
    parser.add_argument(
        "--port", type=int, default=None, help="Optional dashboard/API port."
    )
    parser.add_argument("--once", action="store_true", help="Run one tick and exit.")
    parser.add_argument(
        "--exec-mode",
        choices=["python", "molt-run", "molt-bin"],
        default=os.environ.get("MOLT_SYMPHONY_EXEC_MODE", "python"),
        help="Execution engine for Symphony process (default: MOLT_SYMPHONY_EXEC_MODE or python).",
    )
    parser.add_argument(
        "--molt-profile",
        choices=["dev", "release"],
        default=os.environ.get("MOLT_SYMPHONY_MOLT_PROFILE", "dev"),
        help="Build profile for Molt-backed execution modes.",
    )
    parser.add_argument(
        "--molt-build-arg",
        action="append",
        default=[],
        help="Extra args passed through to Molt build/run pipeline.",
    )
    parser.add_argument(
        "--compiled-output",
        default=None,
        help="Output binary path for --exec-mode molt-bin (default: $MOLT_EXT_ROOT/bin/symphony_molt).",
    )
    parser.add_argument(
        "--rebuild-binary",
        action="store_true",
        help="Force rebuild before running in --exec-mode molt-bin.",
    )
    parser.add_argument(
        "--timing",
        action="store_true",
        help="Print local launch timing for build/run phases.",
    )
    parser.add_argument(
        "--env-file",
        default=None,
        help=(
            "Optional env file (KEY=VALUE) loaded before launch. "
            "If omitted, ops/linear/runtime/symphony.env is used when present."
        ),
    )
    return parser


def ensure_external_root(path: Path) -> None:
    if path.exists() and path.is_dir():
        return
    raise RuntimeError(
        f"External workspace root is unavailable: {path}. Mount it before running Symphony."
    )


def _load_env_file(path: Path) -> dict[str, str]:
    loaded: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        if "=" not in line:
            continue
        key, value = line.split("=", 1)
        key = key.strip()
        value = value.strip().strip('"').strip("'")
        if key:
            loaded[key] = value
    return loaded


def _default_repo_url(cwd: Path) -> str | None:
    proc = subprocess.run(
        ["git", "config", "--get", "remote.origin.url"],
        cwd=cwd,
        check=False,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        return None
    value = proc.stdout.strip()
    return value or None


def _detect_uv_binary() -> str | None:
    env_uv = os.environ.get("UV_BIN", "").strip()
    if env_uv:
        return env_uv
    discovered = shutil.which("uv")
    if discovered:
        return discovered
    for candidate in (
        "/opt/homebrew/bin/uv",
        "/usr/local/bin/uv",
        str(Path.home() / ".local" / "bin" / "uv"),
    ):
        if Path(candidate).exists():
            return candidate
    return None


def _uv_python_args(uv_bin: str, *python_args: str) -> list[str]:
    return [uv_bin, "run", "--python", "3.12", "python3", *python_args]


def _has_respect_pythonpath_flag(args: list[str]) -> bool:
    return any(
        item == "--respect-pythonpath" or item.startswith("--respect-pythonpath=")
        for item in args
    )


def _ensure_dashboard_security_defaults(
    *, env: dict[str, str], ext_root: Path, port: int | None
) -> None:
    env.setdefault("MOLT_SYMPHONY_SECURITY_PROFILE", "local")
    env.setdefault("MOLT_SYMPHONY_BIND_HOST", "127.0.0.1")
    env.setdefault("MOLT_SYMPHONY_ALLOW_NONLOCAL_BIND", "0")
    env.setdefault("MOLT_SYMPHONY_ALLOW_QUERY_TOKEN", "1")
    env.setdefault("MOLT_SYMPHONY_DISABLE_DASHBOARD_UI", "0")
    env.setdefault("MOLT_SYMPHONY_ENFORCE_ORIGIN", "1")
    env.setdefault("MOLT_SYMPHONY_REQUIRE_CSRF_HEADER", "1")
    env.setdefault("MOLT_SYMPHONY_MAX_HTTP_CONNECTIONS", "96")
    env.setdefault("MOLT_SYMPHONY_MAX_STREAM_CLIENTS", "16")
    env.setdefault("MOLT_SYMPHONY_STREAM_MAX_AGE_SECONDS", "300")
    env.setdefault("MOLT_SYMPHONY_HTTP_RATE_LIMIT_MAX_REQUESTS", "240")
    env.setdefault("MOLT_SYMPHONY_HTTP_RATE_LIMIT_WINDOW_SECONDS", "60")
    env.setdefault("MOLT_SYMPHONY_EVENT_QUEUE_MAX", "8192")
    env.setdefault("MOLT_SYMPHONY_EVENT_QUEUE_DROP_LOG_INTERVAL", "250")
    env.setdefault(
        "MOLT_SYMPHONY_SECURITY_EVENTS_FILE",
        str(ext_root / "logs" / "symphony" / "security" / "events.jsonl"),
    )
    security_events_file = Path(
        str(env["MOLT_SYMPHONY_SECURITY_EVENTS_FILE"])
    ).expanduser()
    if not security_events_file.is_absolute():
        security_events_file = (Path.cwd() / security_events_file).resolve()
    try:
        security_events_file.parent.mkdir(parents=True, exist_ok=True)
    except OSError:
        pass
    env["MOLT_SYMPHONY_SECURITY_EVENTS_FILE"] = str(security_events_file)

    token = (
        str(env.get("MOLT_SYMPHONY_API_TOKEN") or "").strip()
        or str(env.get("MOLT_SYMPHONY_DASHBOARD_TOKEN") or "").strip()
    )
    if not token:
        token_file = Path(
            str(
                env.get("MOLT_SYMPHONY_API_TOKEN_FILE")
                or (ext_root / "logs" / "symphony" / "secrets" / "dashboard_api_token")
            )
        ).expanduser()
        if not token_file.is_absolute():
            token_file = (Path.cwd() / token_file).resolve()
        token_file.parent.mkdir(parents=True, exist_ok=True)
        if token_file.exists():
            token = token_file.read_text(encoding="utf-8").strip()
        if not token:
            token = secrets.token_urlsafe(32)
            token_file.write_text(token + "\n", encoding="utf-8")
        try:
            token_file.chmod(0o600)
        except OSError:
            pass
        env["MOLT_SYMPHONY_API_TOKEN_FILE"] = str(token_file)
    env["MOLT_SYMPHONY_API_TOKEN"] = token
    env.setdefault("MOLT_SYMPHONY_DASHBOARD_TOKEN", token)
    if port is not None:
        env.setdefault(
            "MOLT_SYMPHONY_ALLOWED_ORIGINS",
            f"http://127.0.0.1:{port},http://localhost:{port}",
        )


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)

    repo_root = Path.cwd()
    env = os.environ.copy()

    env_file_arg = args.env_file
    if env_file_arg:
        env_file = Path(env_file_arg).expanduser()
    else:
        env_file = DEFAULT_ENV_FILE

    if env_file.exists():
        for key, value in _load_env_file(env_file).items():
            env.setdefault(key, value)

    ext_root = Path(env.get("MOLT_EXT_ROOT", DEFAULT_EXT_ROOT)).expanduser()
    ensure_external_root(ext_root)

    env.setdefault("MOLT_EXT_ROOT", str(ext_root))
    env.setdefault("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
    env.setdefault("MOLT_CACHE", str(ext_root / "molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    env.setdefault("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    env.setdefault("MOLT_BACKEND_DAEMON_SOCKET_DIR", "/tmp/molt_backend_sockets")
    env.setdefault("TMPDIR", str(ext_root / "tmp"))
    env.setdefault("PYTHONPATH", "src")
    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "UV_CACHE_DIR",
        "TMPDIR",
    ):
        path_value = env.get(key)
        if not path_value:
            continue
        try:
            Path(path_value).expanduser().mkdir(parents=True, exist_ok=True)
        except OSError:
            # Keep launcher resilient; downstream commands will surface hard failures.
            pass
    env["MOLT_SYMPHONY_EXEC_MODE"] = args.exec_mode

    repo_url = _default_repo_url(repo_root)
    if repo_url:
        env.setdefault("MOLT_SOURCE_REPO_URL", repo_url)
    env.setdefault("MOLT_SYMPHONY_SYNC_REMOTE", "origin")
    env.setdefault("MOLT_SYMPHONY_SYNC_BRANCH", "main")
    env.setdefault("MOLT_SYMPHONY_AUTOMERGE_ALLOWED_AUTHORS", "adpena,symphony")
    _ensure_dashboard_security_defaults(env=env, ext_root=ext_root, port=args.port)

    if not env.get("MOLT_LINEAR_PROJECT_SLUG"):
        raise RuntimeError(
            "MOLT_LINEAR_PROJECT_SLUG is required. Set it in shell env or env file."
        )
    if not env.get("LINEAR_API_KEY"):
        raise RuntimeError(
            "LINEAR_API_KEY is required for tracker API access. Set it in shell env or env file."
        )
    uv_bin = _detect_uv_binary()
    if uv_bin is None:
        raise RuntimeError("uv is required for Symphony launcher commands")

    runtime_args = [args.workflow_path]
    if args.port is not None:
        runtime_args.extend(["--port", str(args.port)])
    if args.once:
        runtime_args.append("--once")

    mode = args.exec_mode
    symphony_entry_file = "tools/symphony_entry.py"
    if mode == "python":
        cmd = [*_uv_python_args(uv_bin, "-m", "molt.symphony"), *runtime_args]
        start = time.perf_counter()
        proc = subprocess.run(cmd, env=env, check=False)
        if args.timing:
            duration = max(time.perf_counter() - start, 0.0)
            print(f"symphony_run.mode={mode} run_s={duration:.3f}", file=sys.stderr)
        return int(proc.returncode)

    if mode == "molt-run":
        build_args = list(args.molt_build_arg)
        if not _has_respect_pythonpath_flag(build_args):
            build_args.append("--respect-pythonpath")
        cmd = [
            *_uv_python_args(uv_bin, "-m", "molt.cli"),
            "run",
            symphony_entry_file,
            "--profile",
            args.molt_profile,
        ]
        for build_arg in build_args:
            cmd.extend(["--build-arg", build_arg])
        cmd.append("--")
        cmd.extend(runtime_args)
        start = time.perf_counter()
        proc = subprocess.run(cmd, env=env, check=False)
        if args.timing:
            duration = max(time.perf_counter() - start, 0.0)
            print(f"symphony_run.mode={mode} run_s={duration:.3f}", file=sys.stderr)
        return int(proc.returncode)

    compiled_output = args.compiled_output
    if compiled_output:
        output_path = Path(compiled_output).expanduser()
        if not output_path.is_absolute():
            output_path = (repo_root / output_path).resolve()
    else:
        output_path = ext_root / "bin" / "symphony_molt"
    output_path.parent.mkdir(parents=True, exist_ok=True)

    should_build = args.rebuild_binary or not output_path.exists()
    build_seconds = 0.0
    if should_build:
        build_args = list(args.molt_build_arg)
        if not _has_respect_pythonpath_flag(build_args):
            build_args.append("--respect-pythonpath")
        build_cmd = [
            *_uv_python_args(uv_bin, "-m", "molt.cli"),
            "build",
            symphony_entry_file,
            "--profile",
            args.molt_profile,
            "--output",
            str(output_path),
            *build_args,
        ]
        build_start = time.perf_counter()
        build_proc = subprocess.run(build_cmd, env=env, check=False)
        build_seconds = max(time.perf_counter() - build_start, 0.0)
        if build_proc.returncode != 0:
            if args.timing:
                print(
                    f"symphony_run.mode={mode} build_s={build_seconds:.3f}",
                    file=sys.stderr,
                )
            return int(build_proc.returncode)

    run_cmd = [str(output_path), *runtime_args]
    run_start = time.perf_counter()
    run_proc = subprocess.run(run_cmd, env=env, check=False)
    run_seconds = max(time.perf_counter() - run_start, 0.0)

    if args.timing:
        if should_build:
            print(
                f"symphony_run.mode={mode} build_s={build_seconds:.3f}", file=sys.stderr
            )
        print(f"symphony_run.mode={mode} run_s={run_seconds:.3f}", file=sys.stderr)
    return int(run_proc.returncode)


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
