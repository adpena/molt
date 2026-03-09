from __future__ import annotations

import argparse
import os
import secrets
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from molt.symphony.paths import (
    resolve_molt_ext_root,
    symphony_api_token_file,
    symphony_artifact_root,
    symphony_durable_root,
    symphony_log_root,
    symphony_security_events_file,
    symphony_state_root,
    resolve_symphony_parent_root,
    resolve_symphony_store_root,
    symphony_workspace_root,
)

try:
    from . import compile_governor
except ImportError:  # pragma: no cover - script execution path.
    import compile_governor  # type: ignore[no-redef]

DEFAULT_ENV_FILE = Path("ops/linear/runtime/symphony.env")


def _default_quint_node_fallback() -> str:
    if os.name == "nt":
        for candidate in (
            Path("C:/Program Files/nodejs/node.exe"),
            Path("C:/Program Files (x86)/nodejs/node.exe"),
        ):
            if candidate.exists():
                return str(candidate)
        return "npx -y node@22"
    for candidate in (
        Path("/opt/homebrew/opt/node@22/bin/node"),
        Path("/usr/local/opt/node@22/bin/node"),
    ):
        if candidate.exists():
            return str(candidate)
    return "npx -y node@22"


def _default_java_home() -> str | None:
    for env_key in ("JAVA_HOME", "MOLT_JAVA_HOME"):
        raw = str(os.environ.get(env_key) or "").strip()
        if raw and (Path(raw) / "bin" / "java").exists():
            return raw
    if os.name == "nt":
        for candidate in (
            Path("C:/Program Files/Eclipse Adoptium/jdk-21"),
            Path("C:/Program Files/Java/jdk-21"),
            Path("C:/Program Files/Java/jdk-17"),
        ):
            if (candidate / "bin" / "java.exe").exists():
                return str(candidate)
        return None
    if sys.platform.startswith("linux"):
        for candidate in (
            Path("/usr/lib/jvm/java-21-openjdk"),
            Path("/usr/lib/jvm/temurin-21-jdk"),
            Path("/usr/lib/jvm/default-java"),
        ):
            if (candidate / "bin" / "java").exists():
                return str(candidate)
        return None
    for candidate in (
        Path("/opt/homebrew/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"),
        Path("/opt/homebrew/opt/openjdk/libexec/openjdk.jdk/Contents/Home"),
        Path("/usr/local/opt/openjdk@21/libexec/openjdk.jdk/Contents/Home"),
        Path("/usr/local/opt/openjdk/libexec/openjdk.jdk/Contents/Home"),
        Path("/Library/Java/JavaVirtualMachines/openjdk-21.jdk/Contents/Home"),
        Path("/Library/Java/JavaVirtualMachines/temurin-21.jdk/Contents/Home"),
    ):
        if (candidate / "bin" / "java").exists():
            return str(candidate)
    return None


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
    parser.add_argument(
        "--wait-for-external-root-seconds",
        type=int,
        default=int(
            os.environ.get("MOLT_SYMPHONY_WAIT_FOR_EXTERNAL_ROOT_SECONDS", "0")
        ),
        help=(
            "How long to wait for the canonical external roots before failing. "
            "Use -1 to wait forever."
        ),
    )
    parser.add_argument(
        "--wait-for-external-root-interval-ms",
        type=int,
        default=int(
            os.environ.get("MOLT_SYMPHONY_WAIT_FOR_EXTERNAL_ROOT_INTERVAL_MS", "5000")
        ),
        help="Polling interval while waiting for the external roots.",
    )
    return parser


def ensure_external_root(path: Path) -> None:
    if path.exists() and path.is_dir():
        return
    raise RuntimeError(
        f"External workspace root is unavailable: {path}. Mount it before running Symphony."
    )


def _wait_for_external_roots(
    *paths: Path,
    timeout_seconds: int,
    poll_interval_ms: int,
) -> None:
    unique_paths = tuple(dict.fromkeys(path.resolve() for path in paths))
    if not unique_paths:
        return
    deadline = (
        None if timeout_seconds < 0 else time.monotonic() + max(timeout_seconds, 0)
    )
    poll_seconds = max(int(poll_interval_ms), 250) / 1000.0
    last_message_at = 0.0
    while True:
        missing = [
            path for path in unique_paths if not (path.exists() and path.is_dir())
        ]
        if not missing:
            return
        now = time.monotonic()
        if deadline is not None and now >= deadline:
            rendered = ", ".join(str(path) for path in missing)
            raise RuntimeError(
                f"External workspace root is unavailable: {rendered}. Mount it before running Symphony."
            )
        if last_message_at == 0.0 or now - last_message_at >= max(30.0, poll_seconds):
            rendered = ", ".join(str(path) for path in missing)
            print(
                f"symphony_run.waiting_for_external_roots missing={rendered}",
                file=sys.stderr,
                flush=True,
            )
            last_message_at = now
        time.sleep(poll_seconds)


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


def _uv_python_entrypoint() -> str:
    return "python" if sys.platform.startswith("win") else "python3"


def _uv_python_args(uv_bin: str, *python_args: str) -> list[str]:
    return [uv_bin, "run", "--python", "3.12", _uv_python_entrypoint(), *python_args]


def _default_daemon_socket_dir() -> str:
    if os.name == "nt":
        return str(Path(tempfile.gettempdir()) / "molt_backend_sockets")
    return "/tmp/molt_backend_sockets"


def _has_respect_pythonpath_flag(args: list[str]) -> bool:
    return any(
        item == "--respect-pythonpath" or item.startswith("--respect-pythonpath=")
        for item in args
    )


def _ensure_dashboard_security_defaults(
    *, env: dict[str, str], port: int | None, ext_root: Path | None = None
) -> None:
    if ext_root is not None:
        # Standalone callers (including unit tests) can pin a temporary store root
        # without mutating the committed env file.
        env.setdefault("MOLT_EXT_ROOT", str(ext_root))
        env.setdefault("MOLT_SYMPHONY_PARENT_ROOT", str(ext_root.parent / "symphony"))
        env.setdefault("MOLT_SYMPHONY_PROJECT_KEY", "molt")
        env.setdefault(
            "MOLT_SYMPHONY_STORE_ROOT", str(resolve_symphony_store_root(env))
        )
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
    env.setdefault("MOLT_SYMPHONY_LOG_ROOT", str(symphony_log_root(env)))
    env.setdefault("MOLT_SYMPHONY_STATE_ROOT", str(symphony_state_root(env)))
    env.setdefault("MOLT_SYMPHONY_ARTIFACT_ROOT", str(symphony_artifact_root(env)))
    env.setdefault("MOLT_SYMPHONY_WORKSPACE_ROOT", str(symphony_workspace_root(env)))
    env.setdefault("MOLT_SYMPHONY_DURABLE_ROOT", str(symphony_durable_root(env)))
    env.setdefault(
        "MOLT_SYMPHONY_SECURITY_EVENTS_FILE",
        str(symphony_security_events_file(env)),
    )
    security_events_file = symphony_security_events_file(env)
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
        token_file = symphony_api_token_file(env)
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

    ext_root = resolve_molt_ext_root(env)
    symphony_parent_root = resolve_symphony_parent_root(env)
    symphony_store_root = resolve_symphony_store_root(env)
    if int(args.wait_for_external_root_seconds) != 0:
        _wait_for_external_roots(
            ext_root,
            symphony_parent_root,
            timeout_seconds=int(args.wait_for_external_root_seconds),
            poll_interval_ms=int(args.wait_for_external_root_interval_ms),
        )
    ensure_external_root(ext_root)
    ensure_external_root(symphony_parent_root)

    env.setdefault("MOLT_EXT_ROOT", str(ext_root))
    env.setdefault("MOLT_SYMPHONY_PARENT_ROOT", str(symphony_parent_root))
    env.setdefault("MOLT_SYMPHONY_PROJECT_KEY", "molt")
    env.setdefault("MOLT_SYMPHONY_STORE_ROOT", str(symphony_store_root))
    env.setdefault("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
    env.setdefault("MOLT_CACHE", str(ext_root / "molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    env.setdefault(
        "MOLT_COMPILE_GUARD_DIR",
        str(ext_root / "cargo-target" / ".molt_state" / "compile_guard"),
    )
    env.setdefault("MOLT_APALACHE_WORK_DIR", str(ext_root / "tmp" / "apalache"))
    env.setdefault("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    env.setdefault("MOLT_BACKEND_DAEMON_SOCKET_DIR", _default_daemon_socket_dir())
    env.setdefault("TMPDIR", str(ext_root / "tmp"))
    env.setdefault("PYTHONPATH", "src")
    env.setdefault("MOLT_SYMPHONY_LOG_ROOT", str(symphony_log_root(env)))
    env.setdefault("MOLT_SYMPHONY_STATE_ROOT", str(symphony_state_root(env)))
    env.setdefault("MOLT_SYMPHONY_ARTIFACT_ROOT", str(symphony_artifact_root(env)))
    env.setdefault("MOLT_SYMPHONY_WORKSPACE_ROOT", str(symphony_workspace_root(env)))
    env.setdefault("MOLT_SYMPHONY_DURABLE_ROOT", str(symphony_durable_root(env)))
    for key in (
        "CARGO_TARGET_DIR",
        "MOLT_CACHE",
        "MOLT_DIFF_ROOT",
        "MOLT_DIFF_TMPDIR",
        "MOLT_APALACHE_WORK_DIR",
        "UV_CACHE_DIR",
        "TMPDIR",
        "MOLT_SYMPHONY_LOG_ROOT",
        "MOLT_SYMPHONY_STATE_ROOT",
        "MOLT_SYMPHONY_ARTIFACT_ROOT",
        "MOLT_SYMPHONY_WORKSPACE_ROOT",
        "MOLT_SYMPHONY_DURABLE_ROOT",
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
    env.setdefault("MOLT_SYMPHONY_TRUSTED_USERS", "adpena,symphony")
    env.setdefault("MOLT_SYMPHONY_TRUSTED_MACHINES", "")
    env.setdefault("MOLT_SYMPHONY_AUTOLAND_ENABLED", "1")
    env.setdefault("MOLT_SYMPHONY_AUTOLAND_MODE", "direct-main")
    env.setdefault("MOLT_SYMPHONY_AUTOLAND_COMMIT_MESSAGE", "chore: sync all changes")
    env.setdefault("MOLT_SYMPHONY_AUTOLAND_PR_AUTOMERGE", "1")
    env.setdefault("MOLT_SYMPHONY_AUTOLAND_PR_BASE", "main")
    env.setdefault(
        "MOLT_SYMPHONY_CODEX_ARGS",
        (
            "-c model_reasoning_effort=low "
            "-c model_reasoning_summary=none "
            "-c hide_agent_reasoning=true "
            "-c model_verbosity=low "
            "-c tool_output_token_limit=6000 "
            "-c model_auto_compact_token_limit=120000"
        ),
    )
    env.setdefault("MOLT_QUINT_NODE_FALLBACK", _default_quint_node_fallback())
    default_java_home = _default_java_home()
    if default_java_home:
        env.setdefault("JAVA_HOME", default_java_home)
    java_home = str(env.get("JAVA_HOME") or "").strip()
    if java_home:
        java_bin_dir = str(Path(java_home) / "bin")
        if Path(java_bin_dir).exists():
            current_path = env.get("PATH", "")
            path_parts = [part for part in current_path.split(os.pathsep) if part]
            if java_bin_dir not in path_parts:
                env["PATH"] = (
                    f"{java_bin_dir}{os.pathsep}{current_path}"
                    if current_path
                    else java_bin_dir
                )
    _ensure_dashboard_security_defaults(env=env, port=args.port, ext_root=ext_root)

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
        with compile_governor.compile_slot(env=env, label="symphony_run:molt-run"):
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
        with compile_governor.compile_slot(
            env=env,
            label="symphony_run:molt-bin-build",
        ):
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
