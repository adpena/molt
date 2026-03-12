from __future__ import annotations

import argparse
import os
import plistlib
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

try:
    from molt.symphony.paths import default_molt_ext_root, resolve_symphony_env_file
except ModuleNotFoundError:  # pragma: no cover - script execution path.
    _REPO_ROOT = Path(__file__).resolve().parents[1]
    _SRC_ROOT = _REPO_ROOT / "src"
    if str(_SRC_ROOT) not in sys.path:
        sys.path.insert(0, str(_SRC_ROOT))
    from molt.symphony.paths import default_molt_ext_root, resolve_symphony_env_file


LABEL = "com.molt.symphony"
WATCHDOG_LABEL = "com.molt.symphony.watchdog"
DEFAULT_ENV_FILE = resolve_symphony_env_file()
DEFAULT_EXT_ROOT = default_molt_ext_root()
DEFAULT_WAIT_FOR_EXTERNAL_ROOT_SECONDS = -1
DEFAULT_WAIT_FOR_EXTERNAL_ROOT_INTERVAL_MS = 5_000
DEFAULT_HEALTH_INTERVAL_MS = 10_000
DEFAULT_HEALTH_STARTUP_GRACE_MS = 90_000
DEFAULT_HEALTH_FAILURE_THRESHOLD = 3


@dataclass(frozen=True)
class LaunchdServiceInfo:
    label: str
    loaded: bool
    plist_exists: bool
    plist_path: Path
    state: str | None = None
    active_count: int | None = None
    last_exit_code: int | None = None
    last_exit_detail: str | None = None


def _uid() -> int:
    return int(os.getuid())


def launchd_domain() -> str:
    return f"gui/{_uid()}"


def launchd_target(label: str) -> str:
    return f"{launchd_domain()}/{label}"


def plist_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{LABEL}.plist"


def watchdog_plist_path() -> Path:
    return Path.home() / "Library" / "LaunchAgents" / f"{WATCHDOG_LABEL}.plist"


def service_plist_path(label: str) -> Path | None:
    if label == LABEL:
        return plist_path()
    if label == WATCHDOG_LABEL:
        return watchdog_plist_path()
    return None


def launchd_log_root() -> Path:
    # launchd cannot even spawn the process if stdout/stderr live on an absent
    # external volume. Keep a tiny local control-plane log root so the service
    # can wait for the canonical external storage and self-heal once it returns.
    root = Path.home() / "Library" / "Logs" / "Molt" / "symphony-launchd"
    root.mkdir(parents=True, exist_ok=True)
    return root


def _launchctl(
    *args: str, capture_output: bool = False
) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["launchctl", *args],
        check=False,
        capture_output=capture_output,
        text=True,
    )


def _safe_bootout(target: Path) -> None:
    proc = _launchctl("bootout", launchd_domain(), str(target), capture_output=True)
    if proc.returncode == 0:
        return
    combined = f"{proc.stdout or ''}\n{proc.stderr or ''}".lower()
    if (
        "no such process" in combined
        or "could not find service" in combined
        or "service cannot load in requested session" in combined
    ):
        return
    _launchctl("unload", str(target))


def _load_service(target: Path, label: str) -> None:
    _safe_bootout(target)
    proc = _launchctl("bootstrap", launchd_domain(), str(target), capture_output=True)
    if proc.returncode != 0:
        fallback = _launchctl("load", str(target), capture_output=True)
        if fallback.returncode != 0:
            stderr = (fallback.stderr or proc.stderr or "").strip()
            raise RuntimeError(f"launchctl load failed for {label}: {stderr}")
    _launchctl("enable", launchd_target(label))


def _unload_service(target: Path) -> None:
    _safe_bootout(target)
    _launchctl("unload", str(target))


def ensure_service_loaded(label: str) -> bool:
    plist = service_plist_path(label)
    if plist is None or not plist.exists():
        return False
    _load_service(plist, label)
    return True


def restart_service(label: str) -> bool:
    target = launchd_target(label)
    proc = _launchctl("kickstart", "-k", target, capture_output=True)
    if proc.returncode == 0:
        return True
    return ensure_service_loaded(label)


def inspect_service(label: str) -> LaunchdServiceInfo:
    plist = service_plist_path(label)
    if plist is None:
        raise RuntimeError(f"unknown launchd label: {label}")
    proc = _launchctl("print", launchd_target(label), capture_output=True)
    if proc.returncode != 0:
        return LaunchdServiceInfo(
            label=label,
            loaded=False,
            plist_exists=plist.exists(),
            plist_path=plist,
        )
    state: str | None = None
    active_count: int | None = None
    last_exit_code: int | None = None
    last_exit_detail: str | None = None
    for raw_line in (proc.stdout or "").splitlines():
        line = raw_line.strip()
        if line.startswith("state = "):
            state = line.split("=", 1)[1].strip()
            continue
        if line.startswith("active count = "):
            try:
                active_count = int(line.split("=", 1)[1].strip())
            except ValueError:
                active_count = None
            continue
        if line.startswith("last exit code = "):
            payload = line.split("=", 1)[1].strip()
            code_raw, _, detail = payload.partition(":")
            try:
                last_exit_code = int(code_raw.strip())
            except ValueError:
                last_exit_code = None
            last_exit_detail = detail.strip() or None
    return LaunchdServiceInfo(
        label=label,
        loaded=True,
        plist_exists=plist.exists(),
        plist_path=plist,
        state=state,
        active_count=active_count,
        last_exit_code=last_exit_code,
        last_exit_detail=last_exit_detail,
    )


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
    wait_for_external_root_seconds: int,
    wait_for_external_root_interval_ms: int,
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
        "--wait-for-external-root-seconds",
        str(wait_for_external_root_seconds),
        "--wait-for-external-root-interval-ms",
        str(wait_for_external_root_interval_ms),
    ]
    for build_arg in molt_build_args:
        args.extend(["--molt-build-arg", build_arg])
    if compiled_output:
        args.extend(["--compiled-output", compiled_output])
    return args


def build_watchdog_program(
    repo_root: Path,
    python_bin: str,
    env_file: Path,
    interval_ms: int,
    quiet_ms: int,
    cooldown_ms: int,
    *,
    state_url: str,
    state_timeout_ms: int,
    defer_log_interval_ms: int,
    restart_when_idle: bool,
    perf_check: bool,
    perf_interval_ms: int,
    perf_timeout_ms: int,
    perf_command: str | None,
    perf_defer_when_busy: bool,
    health_check: bool,
    health_interval_ms: int,
    health_startup_grace_ms: int,
    health_failure_threshold: int,
) -> list[str]:
    args = [
        python_bin,
        "tools/symphony_watchdog.py",
        "--repo-root",
        str(repo_root),
        "--env-file",
        str(env_file),
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
        "--health-interval-ms",
        str(health_interval_ms),
        "--health-startup-grace-ms",
        str(health_startup_grace_ms),
        "--health-failure-threshold",
        str(health_failure_threshold),
        "--perf-interval-ms",
        str(perf_interval_ms),
        "--perf-timeout-ms",
        str(perf_timeout_ms),
    ]
    if health_check:
        args.append("--health-check")
    else:
        args.append("--no-health-check")
    if perf_check:
        args.append("--perf-check")
    else:
        args.append("--no-perf-check")
    if perf_defer_when_busy:
        args.append("--perf-defer-when-busy")
    else:
        args.append("--perf-run-when-busy")
    if perf_command:
        args.extend(["--perf-command", perf_command])
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
    watchdog_perf_check: bool,
    watchdog_perf_interval_ms: int,
    watchdog_perf_timeout_ms: int,
    watchdog_perf_command: str | None,
    watchdog_perf_defer_when_busy: bool,
    exec_mode: str,
    molt_profile: str,
    molt_build_args: list[str],
    compiled_output: str | None,
    wait_for_external_root_seconds: int,
    wait_for_external_root_interval_ms: int,
    watchdog_health_check: bool,
    watchdog_health_interval_ms: int,
    watchdog_health_startup_grace_ms: int,
    watchdog_health_failure_threshold: int,
) -> None:
    target = plist_path()
    target.parent.mkdir(parents=True, exist_ok=True)
    log_root = launchd_log_root()
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
            wait_for_external_root_seconds=wait_for_external_root_seconds,
            wait_for_external_root_interval_ms=wait_for_external_root_interval_ms,
        ),
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "ProcessType": "Background",
        "ThrottleInterval": 15,
        "ExitTimeOut": 30,
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

    _load_service(target, LABEL)

    watchdog_target = watchdog_plist_path()
    if enable_watchdog:
        watchdog_plist = {
            "Label": WATCHDOG_LABEL,
            "ProgramArguments": build_watchdog_program(
                repo_root=repo_root,
                python_bin=python_bin,
                env_file=env_file,
                interval_ms=max(watchdog_interval_ms, 250),
                quiet_ms=max(watchdog_quiet_ms, 250),
                cooldown_ms=max(watchdog_cooldown_ms, 250),
                state_url=watchdog_state_url,
                state_timeout_ms=max(watchdog_state_timeout_ms, 100),
                defer_log_interval_ms=max(watchdog_defer_log_interval_ms, 500),
                restart_when_idle=watchdog_restart_when_idle,
                perf_check=watchdog_perf_check,
                perf_interval_ms=max(watchdog_perf_interval_ms, 60_000),
                perf_timeout_ms=max(watchdog_perf_timeout_ms, 5_000),
                perf_command=watchdog_perf_command,
                perf_defer_when_busy=watchdog_perf_defer_when_busy,
                health_check=watchdog_health_check,
                health_interval_ms=max(watchdog_health_interval_ms, 1_000),
                health_startup_grace_ms=max(
                    watchdog_health_startup_grace_ms,
                    5_000,
                ),
                health_failure_threshold=max(watchdog_health_failure_threshold, 1),
            ),
            "RunAtLoad": True,
            "KeepAlive": {"SuccessfulExit": False},
            "ProcessType": "Background",
            "ThrottleInterval": 15,
            "ExitTimeOut": 30,
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

        _load_service(watchdog_target, WATCHDOG_LABEL)
    elif watchdog_target.exists():
        _unload_service(watchdog_target)
        watchdog_target.unlink()


def uninstall(*, include_watchdog: bool = True) -> None:
    target = plist_path()
    if target.exists():
        _unload_service(target)
        target.unlink()
    watchdog_target = watchdog_plist_path()
    if include_watchdog and watchdog_target.exists():
        _unload_service(watchdog_target)
        watchdog_target.unlink()


def status() -> int:
    info = inspect_service(LABEL)
    watchdog = inspect_service(WATCHDOG_LABEL)
    print(f"plist: {info.plist_path}")
    print(f"exists: {info.plist_exists}")
    print(f"loaded: {info.loaded}")
    print(f"state: {info.state}")
    print(f"active_count: {info.active_count}")
    print(f"last_exit_code: {info.last_exit_code}")
    print(f"last_exit_detail: {info.last_exit_detail}")
    print(f"watchdog_plist: {watchdog.plist_path}")
    print(f"watchdog_exists: {watchdog.plist_exists}")
    print(f"watchdog_loaded: {watchdog.loaded}")
    print(f"watchdog_state: {watchdog.state}")
    print(f"watchdog_active_count: {watchdog.active_count}")
    print(f"watchdog_last_exit_code: {watchdog.last_exit_code}")
    print(f"watchdog_last_exit_detail: {watchdog.last_exit_detail}")
    print(f"launchd_log_root: {launchd_log_root()}")
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
        help="External root used for runtime defaults",
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
        "--wait-for-external-root-seconds",
        type=int,
        default=DEFAULT_WAIT_FOR_EXTERNAL_ROOT_SECONDS,
        help=(
            "How long the main service should wait for the canonical external roots. "
            "Use -1 to wait forever (default)."
        ),
    )
    install_p.add_argument(
        "--wait-for-external-root-interval-ms",
        type=int,
        default=DEFAULT_WAIT_FOR_EXTERNAL_ROOT_INTERVAL_MS,
        help="Polling interval while waiting for the external volume.",
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
        "--watchdog-health-check",
        action="store_true",
        default=True,
        help="Enable watchdog health-based self-heal probes (default: enabled).",
    )
    install_p.add_argument(
        "--watchdog-no-health-check",
        dest="watchdog_health_check",
        action="store_false",
        help="Disable watchdog health-based self-heal probes.",
    )
    install_p.add_argument(
        "--watchdog-health-interval-ms",
        type=int,
        default=DEFAULT_HEALTH_INTERVAL_MS,
        help="Interval between watchdog health probes.",
    )
    install_p.add_argument(
        "--watchdog-health-startup-grace-ms",
        type=int,
        default=DEFAULT_HEALTH_STARTUP_GRACE_MS,
        help="Grace period after startup/restart before unhealthy probes trigger repair.",
    )
    install_p.add_argument(
        "--watchdog-health-failure-threshold",
        type=int,
        default=DEFAULT_HEALTH_FAILURE_THRESHOLD,
        help="Consecutive unhealthy probes required before watchdog repair kicks in.",
    )
    install_p.add_argument(
        "--watchdog-perf-check",
        action="store_true",
        default=True,
        help="Enable watchdog startup/nightly perf guard (default: enabled).",
    )
    install_p.add_argument(
        "--watchdog-no-perf-check",
        dest="watchdog_perf_check",
        action="store_false",
        help="Disable watchdog perf guard scheduler.",
    )
    install_p.add_argument(
        "--watchdog-perf-interval-ms",
        type=int,
        default=86_400_000,
        help="Watchdog perf guard interval (default: 24h).",
    )
    install_p.add_argument(
        "--watchdog-perf-timeout-ms",
        type=int,
        default=1_800_000,
        help="Watchdog perf guard timeout per run (default: 30m).",
    )
    install_p.add_argument(
        "--watchdog-perf-command",
        default=None,
        help="Optional custom perf guard command string for watchdog.",
    )
    install_p.add_argument(
        "--watchdog-perf-defer-when-busy",
        action="store_true",
        default=True,
        help="Defer watchdog perf checks while running/retrying agents are active.",
    )
    install_p.add_argument(
        "--watchdog-perf-run-when-busy",
        dest="watchdog_perf_defer_when_busy",
        action="store_false",
        help="Allow watchdog perf checks even while active runs exist.",
    )
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
            watchdog_perf_check=bool(args.watchdog_perf_check),
            watchdog_perf_interval_ms=int(args.watchdog_perf_interval_ms),
            watchdog_perf_timeout_ms=int(args.watchdog_perf_timeout_ms),
            watchdog_perf_command=(
                str(args.watchdog_perf_command)
                if args.watchdog_perf_command is not None
                else None
            ),
            watchdog_perf_defer_when_busy=bool(args.watchdog_perf_defer_when_busy),
            exec_mode=str(args.exec_mode),
            molt_profile=str(args.molt_profile),
            molt_build_args=list(args.molt_build_arg),
            compiled_output=(
                str(args.compiled_output) if args.compiled_output is not None else None
            ),
            wait_for_external_root_seconds=int(args.wait_for_external_root_seconds),
            wait_for_external_root_interval_ms=int(
                args.wait_for_external_root_interval_ms
            ),
            watchdog_health_check=bool(args.watchdog_health_check),
            watchdog_health_interval_ms=int(args.watchdog_health_interval_ms),
            watchdog_health_startup_grace_ms=int(args.watchdog_health_startup_grace_ms),
            watchdog_health_failure_threshold=int(
                args.watchdog_health_failure_threshold
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
