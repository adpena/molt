from __future__ import annotations

import argparse
import plistlib
import subprocess
import sys
from pathlib import Path


LABEL = "com.molt.symphony"
WATCHDOG_LABEL = "com.molt.symphony.watchdog"
DEFAULT_ENV_FILE = Path("ops/linear/runtime/symphony.env")
DEFAULT_EXT_ROOT = Path("/Volumes/APDataStore/Molt")


def plist_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{LABEL}.plist"


def watchdog_plist_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{WATCHDOG_LABEL}.plist"


def build_program(
    repo_root: Path,
    python_bin: str,
    port: int,
    env_file: Path,
    *,
    exec_mode: str,
    molt_profile: str,
    molt_build_args: list[str],
    compiled_output: str | None,
) -> list[str]:
    args = [
        python_bin,
        "tools/symphony_run.py",
        "WORKFLOW.md",
        "--port",
        str(port),
        "--env-file",
        str(env_file),
        "--exec-mode",
        exec_mode,
        "--molt-profile",
        molt_profile,
    ]
    for build_arg in molt_build_args:
        args.extend(["--molt-build-arg", build_arg])
    if compiled_output:
        args.extend(["--compiled-output", compiled_output])
    return args


def build_watchdog_program(
    repo_root: Path,
    python_bin: str,
    interval_ms: int,
    quiet_ms: int,
    cooldown_ms: int,
    *,
    state_url: str,
    state_timeout_ms: int,
    defer_log_interval_ms: int,
    restart_when_idle: bool,
) -> list[str]:
    args = [
        python_bin,
        "tools/symphony_watchdog.py",
        "--repo-root",
        str(repo_root),
        "--service-label",
        LABEL,
        "--interval-ms",
        str(interval_ms),
        "--quiet-ms",
        str(quiet_ms),
        "--cooldown-ms",
        str(cooldown_ms),
        "--state-url",
        state_url,
        "--state-timeout-ms",
        str(state_timeout_ms),
        "--defer-log-interval-ms",
        str(defer_log_interval_ms),
    ]
    if restart_when_idle:
        args.append("--restart-when-idle")
    else:
        args.append("--no-restart-when-idle")
    return args


def install(
    repo_root: Path,
    python_bin: str,
    port: int,
    env_file: Path,
    ext_root: Path,
    *,
    enable_watchdog: bool,
    watchdog_interval_ms: int,
    watchdog_quiet_ms: int,
    watchdog_cooldown_ms: int,
    watchdog_state_url: str,
    watchdog_state_timeout_ms: int,
    watchdog_defer_log_interval_ms: int,
    watchdog_restart_when_idle: bool,
    exec_mode: str,
    molt_profile: str,
    molt_build_args: list[str],
    compiled_output: str | None,
) -> None:
    target = plist_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    log_root = ext_root / "logs" / "symphony"
    log_root.mkdir(parents=True, exist_ok=True)
    plist = {
        "Label": LABEL,
        "ProgramArguments": build_program(
            repo_root,
            python_bin,
            port,
            env_file,
            exec_mode=exec_mode,
            molt_profile=molt_profile,
            molt_build_args=molt_build_args,
            compiled_output=compiled_output,
        ),
        "RunAtLoad": True,
        "KeepAlive": True,
        "StandardOutPath": str(log_root / "launchd.out.log"),
        "StandardErrorPath": str(log_root / "launchd.err.log"),
        "WorkingDirectory": str(repo_root),
        "EnvironmentVariables": {
            "PYTHONPATH": "src",
            "MOLT_EXT_ROOT": str(ext_root),
            "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
        },
    }
    with target.open("wb") as fh:
        plistlib.dump(plist, fh)

    subprocess.run(["launchctl", "unload", str(target)], check=False)
    subprocess.run(["launchctl", "load", str(target)], check=True)

    watchdog_target = watchdog_plist_path()
    if enable_watchdog:
        watchdog_plist = {
            "Label": WATCHDOG_LABEL,
            "ProgramArguments": build_watchdog_program(
                repo_root=repo_root,
                python_bin=python_bin,
                interval_ms=max(watchdog_interval_ms, 250),
                quiet_ms=max(watchdog_quiet_ms, 250),
                cooldown_ms=max(watchdog_cooldown_ms, 250),
                state_url=watchdog_state_url,
                state_timeout_ms=max(watchdog_state_timeout_ms, 100),
                defer_log_interval_ms=max(watchdog_defer_log_interval_ms, 500),
                restart_when_idle=watchdog_restart_when_idle,
            ),
            "RunAtLoad": True,
            "KeepAlive": True,
            "StandardOutPath": str(log_root / "watchdog.out.log"),
            "StandardErrorPath": str(log_root / "watchdog.err.log"),
            "WorkingDirectory": str(repo_root),
            "EnvironmentVariables": {
                "PYTHONPATH": "src",
                "MOLT_EXT_ROOT": str(ext_root),
                "PATH": "/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin",
            },
        }
        with watchdog_target.open("wb") as fh:
            plistlib.dump(watchdog_plist, fh)

        subprocess.run(["launchctl", "unload", str(watchdog_target)], check=False)
        subprocess.run(["launchctl", "load", str(watchdog_target)], check=True)
    elif watchdog_target.exists():
        subprocess.run(["launchctl", "unload", str(watchdog_target)], check=False)
        watchdog_target.unlink()


def uninstall(*, include_watchdog: bool = True) -> None:
    target = plist_path()
    if target.exists():
        subprocess.run(["launchctl", "unload", str(target)], check=False)
        target.unlink()
    watchdog_target = watchdog_plist_path()
    if include_watchdog and watchdog_target.exists():
        subprocess.run(["launchctl", "unload", str(watchdog_target)], check=False)
        watchdog_target.unlink()


def status() -> int:
    proc = subprocess.run(
        ["launchctl", "list"],
        check=False,
        capture_output=True,
        text=True,
    )
    loaded = proc.stdout
    target = plist_path()
    watchdog_target = watchdog_plist_path()
    print(f"plist: {target}")
    print(f"exists: {target.exists()}")
    print(f"loaded: {_is_loaded_label(loaded, LABEL)}")
    print(f"watchdog_plist: {watchdog_target}")
    print(f"watchdog_exists: {watchdog_target.exists()}")
    print(f"watchdog_loaded: {_is_loaded_label(loaded, WATCHDOG_LABEL)}")
    return 0


def _is_loaded_label(launchctl_list_output: str, label: str) -> bool:
    for line in launchctl_list_output.splitlines():
        if not line.strip():
            continue
        if line.rstrip().endswith(f"\t{label}"):
            return True
    return False


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Manage Symphony launchd service")
    sub = parser.add_subparsers(dest="cmd", required=True)

    install_p = sub.add_parser("install")
    install_p.add_argument("--repo-root", default=".")
    install_p.add_argument("--python", default=sys.executable)
    install_p.add_argument("--port", type=int, default=8089)
    install_p.add_argument(
        "--env-file",
        default=str(DEFAULT_ENV_FILE),
        help="Path to runtime env file consumed by tools/symphony_run.py",
    )
    install_p.add_argument(
        "--ext-root",
        default=str(DEFAULT_EXT_ROOT),
        help="External root used for logs and runtime defaults",
    )
    install_p.add_argument(
        "--exec-mode",
        choices=["python", "molt-run", "molt-bin"],
        default="python",
        help="Execution engine passed to tools/symphony_run.py",
    )
    install_p.add_argument(
        "--molt-profile",
        choices=["dev", "release"],
        default="dev",
        help="Build profile for Molt-backed execution modes.",
    )
    install_p.add_argument(
        "--molt-build-arg",
        action="append",
        default=[],
        help="Repeatable build args passed to tools/symphony_run.py.",
    )
    install_p.add_argument(
        "--compiled-output",
        default=None,
        help="Optional path for --exec-mode molt-bin output binary.",
    )
    install_p.add_argument(
        "--watchdog",
        dest="watchdog",
        action="store_true",
        default=True,
        help="Install/refresh watchdog launchd service (default: enabled)",
    )
    install_p.add_argument(
        "--no-watchdog",
        dest="watchdog",
        action="store_false",
        help="Install only the main Symphony service",
    )
    install_p.add_argument("--watchdog-interval-ms", type=int, default=1500)
    install_p.add_argument("--watchdog-quiet-ms", type=int, default=1200)
    install_p.add_argument("--watchdog-cooldown-ms", type=int, default=5000)
    install_p.add_argument(
        "--watchdog-state-url", default="http://127.0.0.1:8089/api/v1/state"
    )
    install_p.add_argument("--watchdog-state-timeout-ms", type=int, default=600)
    install_p.add_argument("--watchdog-defer-log-interval-ms", type=int, default=12000)
    install_p.add_argument(
        "--watchdog-restart-when-idle",
        action="store_true",
        default=True,
        help="Defer watchdog restarts while Symphony has running/retrying agents.",
    )
    install_p.add_argument(
        "--watchdog-force-restart",
        dest="watchdog_restart_when_idle",
        action="store_false",
        help="Allow watchdog to restart immediately, even during active runs.",
    )

    uninstall_p = sub.add_parser("uninstall")
    uninstall_p.add_argument(
        "--main-only",
        action="store_true",
        help="Remove only main Symphony service; keep watchdog if present",
    )
    sub.add_parser("status")

    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.cmd == "install":
        install(
            Path(args.repo_root).resolve(),
            args.python,
            args.port,
            Path(args.env_file).expanduser().resolve(),
            Path(args.ext_root).expanduser().resolve(),
            enable_watchdog=bool(args.watchdog),
            watchdog_interval_ms=int(args.watchdog_interval_ms),
            watchdog_quiet_ms=int(args.watchdog_quiet_ms),
            watchdog_cooldown_ms=int(args.watchdog_cooldown_ms),
            watchdog_state_url=str(args.watchdog_state_url),
            watchdog_state_timeout_ms=int(args.watchdog_state_timeout_ms),
            watchdog_defer_log_interval_ms=int(args.watchdog_defer_log_interval_ms),
            watchdog_restart_when_idle=bool(args.watchdog_restart_when_idle),
            exec_mode=str(args.exec_mode),
            molt_profile=str(args.molt_profile),
            molt_build_args=list(args.molt_build_arg),
            compiled_output=(
                str(args.compiled_output) if args.compiled_output is not None else None
            ),
        )
        return status()
    if args.cmd == "uninstall":
        uninstall(include_watchdog=not bool(args.main_only))
        return status()
    if args.cmd == "status":
        return status()
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
