from __future__ import annotations

import argparse
import hashlib
import json
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path
from urllib.error import URLError
from urllib.request import Request, urlopen

try:
    import tools.symphony_launchd as symphony_launchd
except ImportError:  # pragma: no cover - script execution path.
    import symphony_launchd  # type: ignore[no-redef]


DEFAULT_PATTERNS = (
    "WORKFLOW.md",
    "ops/linear/runtime/symphony.env",
    "src/molt/symphony/**/*.py",
    "tools/symphony_run.py",
    "tools/symphony_entry.py",
    "tools/symphony_watchdog.py",
    "tools/symphony_launchd.py",
)
DEFAULT_EXT_ROOT = Path("/Volumes/APDataStore/Molt")
DEFAULT_SYMPHONY_PARENT_ROOT = Path("/Volumes/APDataStore/symphony")


def _utc_now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())


def _log(level: str, message: str, **fields: object) -> None:
    parts = [f"ts={_utc_now_iso()}", f"level={level}", f'msg="{message}"']
    for key in sorted(fields):
        parts.append(f"{key}={fields[key]}")
    print(" ".join(parts), flush=True)


def _collect_paths(repo_root: Path, patterns: tuple[str, ...]) -> list[Path]:
    paths: set[Path] = set()
    for pattern in patterns:
        for path in repo_root.glob(pattern):
            if path.is_file():
                paths.add(path.resolve())
    return sorted(paths)


def _file_digest(path: Path) -> str:
    digest = hashlib.sha1()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def _fingerprint(
    repo_root: Path, patterns: tuple[str, ...]
) -> tuple[tuple[str, int, int, str], ...]:
    rows: list[tuple[str, int, int, str]] = []
    for path in _collect_paths(repo_root, patterns):
        try:
            stat = path.stat()
        except FileNotFoundError:
            continue
        rel = str(path.relative_to(repo_root))
        digest = _file_digest(path)
        rows.append((rel, int(stat.st_mtime_ns), int(stat.st_size), digest))
    rows.sort()
    return tuple(rows)


def _load_env_file(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    loaded: dict[str, str] = {}
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, value = line.split("=", 1)
        loaded[key.strip()] = value.strip().strip('"').strip("'")
    return loaded


def _bool_value(value: str | None, *, default: bool) -> bool:
    text = str(value or "").strip().lower()
    if not text:
        return default
    if text in {"1", "true", "yes", "on"}:
        return True
    if text in {"0", "false", "no", "off"}:
        return False
    return default


def _path_from_env(
    env_values: dict[str, str],
    key: str,
    *,
    default: Path,
    fallback_env_key: str | None = None,
) -> Path:
    raw = str(env_values.get(key) or "").strip()
    if not raw and fallback_env_key:
        raw = str(os.environ.get(fallback_env_key) or "").strip()
    return Path(raw or str(default)).expanduser()


def _external_roots_available(
    *, ext_root: Path, symphony_parent_root: Path
) -> tuple[bool, tuple[Path, ...]]:
    missing = tuple(
        path
        for path in (ext_root, symphony_parent_root)
        if not (path.exists() and path.is_dir())
    )
    return (not missing, missing)


def _load_api_token(env_values: dict[str, str]) -> str | None:
    for key in ("MOLT_SYMPHONY_API_TOKEN", "MOLT_SYMPHONY_DASHBOARD_TOKEN"):
        candidate_token = str(
            env_values.get(key) or os.environ.get(key) or ""
        ).strip()
        if candidate_token:
            return candidate_token
    token_file_raw = str(
        env_values.get("MOLT_SYMPHONY_API_TOKEN_FILE")
        or os.environ.get("MOLT_SYMPHONY_API_TOKEN_FILE")
        or ""
    ).strip()
    if not token_file_raw:
        return None
    token_file = Path(token_file_raw).expanduser()
    try:
        candidate_token = token_file.read_text(encoding="utf-8").strip()
    except OSError:
        return None
    return candidate_token or None


def _dashboard_endpoint_url(state_url: str, endpoint_path: str) -> str:
    base = state_url.strip().rstrip("/")
    for suffix in ("/api/v1/state", "/api/v1/activity", "/api/v1/health"):
        if base.endswith(suffix):
            return base[: -len(suffix)] + endpoint_path
    return base + endpoint_path


def _activity_url_from_state_url(state_url: str) -> str:
    return _dashboard_endpoint_url(state_url, "/api/v1/activity")


def _health_url_from_state_url(state_url: str) -> str:
    return _dashboard_endpoint_url(state_url, "/api/v1/health")


def _default_perf_command(
    *,
    repo_root: Path,
    env_file: Path,
    state_url: str,
    ext_root: Path,
    env_values: dict[str, str],
) -> list[str]:
    base_url = _dashboard_endpoint_url(state_url, "")
    report_dir = str(
        env_values.get("MOLT_SYMPHONY_PERF_GUARD_REPORTS_DIR")
        or (ext_root / "logs" / "symphony")
    )
    verdict_path = str(
        env_values.get("MOLT_SYMPHONY_PERF_VERDICT_FILE")
        or (Path(report_dir) / "perf" / "verdict.json")
    )
    cmd = [
        sys.executable,
        "tools/symphony_perf.py",
        "WORKFLOW.md",
        "--skip-mode-runs",
        "--env-file",
        str(env_file),
        "--dashboard-url",
        base_url,
        "--api-samples",
        str(int(env_values.get("MOLT_SYMPHONY_PERF_GUARD_API_SAMPLES", "80"))),
        "--api-interval-ms",
        str(int(env_values.get("MOLT_SYMPHONY_PERF_GUARD_API_INTERVAL_MS", "250"))),
        "--auto-compare-latest",
        "--reports-dir",
        report_dir,
        "--keep-reports",
        str(int(env_values.get("MOLT_SYMPHONY_PERF_GUARD_KEEP_REPORTS", "120"))),
        "--verdict-json",
        verdict_path,
        "--fail-on-regression",
        "--max-dashboard-avg-latency-regression-ms",
        str(
            float(
                env_values.get(
                    "MOLT_SYMPHONY_PERF_MAX_DASHBOARD_AVG_LATENCY_REGRESSION_MS", "5"
                )
            )
        ),
        "--max-dashboard-p95-latency-regression-ms",
        str(
            float(
                env_values.get(
                    "MOLT_SYMPHONY_PERF_MAX_DASHBOARD_P95_LATENCY_REGRESSION_MS", "10"
                )
            )
        ),
        "--max-avg-regression-ratio",
        str(
            float(env_values.get("MOLT_SYMPHONY_PERF_MAX_AVG_REGRESSION_RATIO", "0.15"))
        ),
        "--max-p95-regression-ratio",
        str(
            float(env_values.get("MOLT_SYMPHONY_PERF_MAX_P95_REGRESSION_RATIO", "0.20"))
        ),
    ]
    if _bool_value(env_values.get("MOLT_SYMPHONY_PERF_LINEAR_SYNC"), default=False):
        team = str(
            env_values.get("MOLT_LINEAR_TEAM")
            or env_values.get("MOLT_LINEAR_PROJECT_TEAM")
            or "Moltlang"
        ).strip()
        project = (
            str(env_values.get("MOLT_LINEAR_PROJECT_SLUG") or "").split(",")[0].strip()
        )
        cmd.extend(["--linear-sync-regressions", "--linear-team", team])
        if project:
            cmd.extend(["--linear-project", project])
        cmd.extend(
            [
                "--linear-max-issues",
                str(int(env_values.get("MOLT_SYMPHONY_PERF_LINEAR_MAX_ISSUES", "8"))),
            ]
        )
    return cmd


def _launchd_target(service_label: str) -> str:
    return symphony_launchd.launchd_target(service_label)


def _launchd_snapshot(service_label: str) -> dict[str, object]:
    try:
        info = symphony_launchd.inspect_service(service_label)
    except Exception as exc:
        return {
            "loaded": False,
            "state": None,
            "active_count": None,
            "last_exit_code": None,
            "last_exit_detail": None,
            "error": str(exc),
        }
    return {
        "loaded": bool(info.loaded),
        "state": info.state,
        "active_count": info.active_count,
        "last_exit_code": info.last_exit_code,
        "last_exit_detail": info.last_exit_detail,
        "plist_exists": info.plist_exists,
        "plist_path": str(info.plist_path),
    }


def _restart_service(service_label: str) -> bool:
    target = _launchd_target(service_label)
    ok = symphony_launchd.restart_service(service_label)
    if not ok:
        snapshot = _launchd_snapshot(service_label)
        _log(
            "WARNING",
            "watchdog_restart_failed",
            target=target,
            state=snapshot.get("state"),
            active_count=snapshot.get("active_count"),
            last_exit_code=snapshot.get("last_exit_code"),
            last_exit_detail=snapshot.get("last_exit_detail"),
            error=snapshot.get("error"),
        )
        return False
    _log("INFO", "watchdog_restarted_service", target=target)
    return True


def _read_json_payload(
    url: str,
    timeout_seconds: float,
    *,
    auth_token: str | None = None,
) -> dict[str, object] | None:
    try:
        headers: dict[str, str] = {}
        if auth_token:
            headers["Authorization"] = f"Bearer {auth_token}"
        request = Request(url, headers=headers, method="GET")
        with urlopen(request, timeout=max(timeout_seconds, 0.5)) as resp:
            if int(getattr(resp, "status", 200)) != 200:
                return None
            raw = resp.read().decode("utf-8")
    except (OSError, URLError):
        return None
    try:
        payload = json.loads(raw)
    except Exception:
        return None
    if isinstance(payload, dict):
        return payload
    return None


def _read_activity_counts(
    activity_url: str,
    timeout_seconds: float,
    *,
    auth_token: str | None = None,
) -> dict[str, int] | None:
    payload = _read_json_payload(
        activity_url,
        timeout_seconds,
        auth_token=auth_token,
    )
    if payload is None:
        return None
    counts = payload.get("counts")
    if not isinstance(counts, dict):
        return None
    running = int(counts.get("running", 0) or 0)
    retrying = int(counts.get("retrying", 0) or 0)
    return {"running": max(running, 0), "retrying": max(retrying, 0)}


def _probe_health_endpoint(
    health_url: str,
    timeout_seconds: float,
    *,
    auth_token: str | None = None,
) -> bool:
    try:
        headers: dict[str, str] = {}
        if auth_token:
            headers["Authorization"] = f"Bearer {auth_token}"
        request = Request(health_url, headers=headers, method="GET")
        with urlopen(request, timeout=max(timeout_seconds, 0.5)) as resp:
            return int(getattr(resp, "status", 200)) == 200
    except (OSError, URLError):
        return False


def _service_is_busy(
    activity_url: str,
    timeout_seconds: float,
    *,
    auth_token: str | None = None,
) -> tuple[bool | None, str]:
    counts = _read_activity_counts(
        activity_url,
        timeout_seconds,
        auth_token=auth_token,
    )
    if counts is None:
        return None, "activity_unavailable"
    running = int(counts.get("running", 0))
    retrying = int(counts.get("retrying", 0))
    busy = running > 0 or retrying > 0
    detail = f"running={running} retrying={retrying}"
    return busy, detail


def _service_health_snapshot(
    service_label: str,
    health_url: str,
    timeout_seconds: float,
    *,
    auth_token: str | None = None,
) -> dict[str, object]:
    if _probe_health_endpoint(health_url, timeout_seconds, auth_token=auth_token):
        return {
            "healthy": True,
            "reason": "health_ok",
            "detail": "health_endpoint_200",
            "loaded": True,
            "active_count": 1,
        }
    snapshot = _launchd_snapshot(service_label)
    loaded = bool(snapshot.get("loaded"))
    active_count = snapshot.get("active_count")
    state = snapshot.get("state")
    last_exit_code = snapshot.get("last_exit_code")
    last_exit_detail = snapshot.get("last_exit_detail")
    if not loaded:
        return {
            **snapshot,
            "healthy": False,
            "reason": "launchd_unloaded",
            "detail": str(snapshot.get("error") or "launchd service not loaded"),
        }
    if isinstance(active_count, int) and active_count > 0:
        return {
            **snapshot,
            "healthy": False,
            "reason": "state_unavailable_active",
            "detail": f"state={state} active_count={active_count}",
        }
    return {
        **snapshot,
        "healthy": False,
        "reason": "launchd_inactive",
        "detail": (
            f"state={state} active_count={active_count} "
            f"last_exit_code={last_exit_code} last_exit_detail={last_exit_detail}"
        ),
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "Watch Symphony-related files and restart the launchd Symphony service "
            "on change (debounced with cooldown)."
        )
    )
    parser.add_argument("--repo-root", default=".")
    parser.add_argument(
        "--env-file",
        default="ops/linear/runtime/symphony.env",
        help="Path to Symphony env file used for perf guard defaults.",
    )
    parser.add_argument("--service-label", default="com.molt.symphony")
    parser.add_argument("--interval-ms", type=int, default=1500)
    parser.add_argument("--quiet-ms", type=int, default=1200)
    parser.add_argument("--cooldown-ms", type=int, default=5000)
    parser.add_argument("--state-url", default="http://127.0.0.1:8089/api/v1/state")
    parser.add_argument("--state-timeout-ms", type=int, default=600)
    parser.add_argument(
        "--defer-log-interval-ms",
        type=int,
        default=12000,
        help="Minimum interval for repeated deferred-restart logs while busy.",
    )
    parser.add_argument(
        "--restart-when-idle",
        action="store_true",
        default=True,
        help="Only restart service when no running/retrying agents are active.",
    )
    parser.add_argument(
        "--no-restart-when-idle",
        dest="restart_when_idle",
        action="store_false",
        help="Allow immediate restart even while agents are running.",
    )
    parser.add_argument(
        "--pattern",
        action="append",
        default=[],
        help="Additional glob patterns relative to repo root",
    )
    parser.add_argument(
        "--health-check",
        action="store_true",
        default=True,
        help="Enable health-based self-heal probes (default: enabled).",
    )
    parser.add_argument(
        "--no-health-check",
        dest="health_check",
        action="store_false",
        help="Disable health-based self-heal probes.",
    )
    parser.add_argument(
        "--health-interval-ms",
        type=int,
        default=10_000,
        help="Interval between health probes.",
    )
    parser.add_argument(
        "--health-startup-grace-ms",
        type=int,
        default=90_000,
        help="Grace period after startup or repair before health restarts begin.",
    )
    parser.add_argument(
        "--health-failure-threshold",
        type=int,
        default=3,
        help="Consecutive unhealthy probes required before restart.",
    )
    parser.add_argument(
        "--perf-check",
        action="store_true",
        default=True,
        help="Enable periodic perf guard execution (default: on).",
    )
    parser.add_argument(
        "--no-perf-check",
        dest="perf_check",
        action="store_false",
        help="Disable periodic perf guard execution.",
    )
    parser.add_argument(
        "--perf-interval-ms",
        type=int,
        default=86_400_000,
        help="Interval between perf-guard runs (default: 24h).",
    )
    parser.add_argument(
        "--perf-timeout-ms",
        type=int,
        default=1_800_000,
        help="Timeout for each perf-guard command run (default: 30m).",
    )
    parser.add_argument(
        "--perf-command",
        default="",
        help="Optional full perf command string. Defaults to built-in symphony_perf invocation.",
    )
    parser.add_argument(
        "--perf-defer-when-busy",
        action="store_true",
        default=True,
        help="Defer perf checks while running/retrying agents are active.",
    )
    parser.add_argument(
        "--perf-run-when-busy",
        dest="perf_defer_when_busy",
        action="store_false",
        help="Allow perf checks even while service is busy.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repo_root = Path(args.repo_root).resolve()
    env_file = Path(args.env_file).expanduser()
    if not env_file.is_absolute():
        env_file = (repo_root / env_file).resolve()
    env_values = _load_env_file(env_file)
    ext_root = _path_from_env(
        env_values,
        "MOLT_EXT_ROOT",
        default=DEFAULT_EXT_ROOT,
        fallback_env_key="MOLT_EXT_ROOT",
    )
    symphony_parent_root = _path_from_env(
        env_values,
        "MOLT_SYMPHONY_PARENT_ROOT",
        default=DEFAULT_SYMPHONY_PARENT_ROOT,
        fallback_env_key="MOLT_SYMPHONY_PARENT_ROOT",
    )
    patterns = tuple(DEFAULT_PATTERNS) + tuple(args.pattern)

    interval_s = max(int(args.interval_ms), 250) / 1000.0
    quiet_s = max(int(args.quiet_ms), 250) / 1000.0
    cooldown_s = max(int(args.cooldown_ms), 250) / 1000.0
    defer_log_interval_s = max(int(args.defer_log_interval_ms), 500) / 1000.0
    health_interval_s = max(int(args.health_interval_ms), 1_000) / 1000.0
    health_startup_grace_s = max(int(args.health_startup_grace_ms), 5_000) / 1000.0
    health_failure_threshold = max(int(args.health_failure_threshold), 1)
    perf_interval_s = max(int(args.perf_interval_ms), 60_000) / 1000.0
    perf_timeout_s = max(int(args.perf_timeout_ms), 5_000) / 1000.0

    previous = _fingerprint(repo_root, patterns)
    pending_change_at: float | None = None
    last_restart_at = 0.0
    last_defer_log_at = 0.0
    last_defer_reason = ""
    last_external_roots_log_at = 0.0

    health_enabled = bool(args.health_check)
    health_failure_count = 0
    health_grace_until = time.monotonic() + health_startup_grace_s
    next_health_due_at = time.monotonic() if health_enabled else float("inf")
    api_token = _load_api_token(env_values)
    activity_url = _activity_url_from_state_url(args.state_url)
    health_url = _health_url_from_state_url(args.state_url)

    perf_enabled_env = _bool_value(
        env_values.get("MOLT_SYMPHONY_PERF_GUARD"),
        default=bool(args.perf_check),
    )
    perf_enabled = bool(args.perf_check) and perf_enabled_env
    perf_command_raw = str(args.perf_command or "").strip()
    if perf_command_raw:
        perf_command = shlex.split(perf_command_raw)
    else:
        perf_command = _default_perf_command(
            repo_root=repo_root,
            env_file=env_file,
            state_url=str(args.state_url),
            ext_root=ext_root,
            env_values=env_values,
        )
    next_perf_due_at = time.monotonic() if perf_enabled else float("inf")
    perf_runs = 0

    _log(
        "INFO",
        "watchdog_started",
        repo_root=repo_root,
        env_file=env_file,
        patterns=len(patterns),
        interval_ms=int(interval_s * 1000),
        quiet_ms=int(quiet_s * 1000),
        cooldown_ms=int(cooldown_s * 1000),
        health_enabled=health_enabled,
        health_interval_ms=int(health_interval_s * 1000),
        health_startup_grace_ms=int(health_startup_grace_s * 1000),
        health_failure_threshold=health_failure_threshold,
        defer_log_interval_ms=int(defer_log_interval_s * 1000),
        perf_enabled=perf_enabled,
        perf_interval_ms=int(perf_interval_s * 1000),
        target=_launchd_target(args.service_label),
        activity_url=activity_url,
        health_url=health_url,
    )
    state_timeout_s = max(int(args.state_timeout_ms), 100) / 1000.0

    def _roots_ready(now: float) -> bool:
        nonlocal last_external_roots_log_at
        ready, missing = _external_roots_available(
            ext_root=ext_root,
            symphony_parent_root=symphony_parent_root,
        )
        if ready:
            return True
        if (
            last_external_roots_log_at == 0.0
            or now - last_external_roots_log_at >= defer_log_interval_s
        ):
            _log(
                "WARNING",
                "watchdog_external_roots_missing",
                missing=",".join(str(path) for path in missing),
            )
            last_external_roots_log_at = now
        return False

    def _maybe_run_health_check(now: float) -> None:
        nonlocal next_health_due_at, health_failure_count, health_grace_until
        nonlocal last_restart_at
        if not health_enabled:
            return
        if now < next_health_due_at:
            return
        next_health_due_at = now + health_interval_s
        if not _roots_ready(now):
            health_failure_count = 0
            return
        snapshot = _service_health_snapshot(
            args.service_label,
            health_url,
            state_timeout_s,
            auth_token=api_token,
        )
        if bool(snapshot.get("healthy")):
            if health_failure_count:
                _log("INFO", "watchdog_health_recovered", detail=snapshot.get("detail"))
            health_failure_count = 0
            return
        if now < health_grace_until:
            _log(
                "INFO",
                "watchdog_health_in_startup_grace",
                reason=snapshot.get("reason"),
                detail=snapshot.get("detail"),
            )
            return
        health_failure_count += 1
        _log(
            "WARNING",
            "watchdog_health_probe_failed",
            consecutive_failures=health_failure_count,
            threshold=health_failure_threshold,
            reason=snapshot.get("reason"),
            detail=snapshot.get("detail"),
            state=snapshot.get("state"),
            active_count=snapshot.get("active_count"),
            last_exit_code=snapshot.get("last_exit_code"),
            last_exit_detail=snapshot.get("last_exit_detail"),
        )
        if health_failure_count < health_failure_threshold:
            return
        if now - last_restart_at < cooldown_s:
            return
        if _restart_service(args.service_label):
            last_restart_at = now
            health_failure_count = 0
            health_grace_until = now + health_startup_grace_s

    def _maybe_run_perf_guard(now: float) -> None:
        nonlocal next_perf_due_at, perf_runs
        if not perf_enabled:
            return
        if now < next_perf_due_at:
            return
        if not _roots_ready(now):
            next_perf_due_at = now + min(perf_interval_s / 8.0, 600.0)
            return
        if bool(args.perf_defer_when_busy):
            busy, detail = _service_is_busy(
                activity_url,
                state_timeout_s,
                auth_token=api_token,
            )
            if busy is True:
                _log("INFO", "watchdog_perf_deferred_busy", detail=detail)
                next_perf_due_at = now + min(perf_interval_s / 8.0, 600.0)
                return
            if busy is None:
                _log(
                    "WARNING",
                    "watchdog_perf_deferred_activity_probe_failed",
                    detail=detail,
                )
                next_perf_due_at = now + min(perf_interval_s / 8.0, 600.0)
                return
        _log(
            "INFO",
            "watchdog_perf_start",
            command=" ".join(shlex.quote(part) for part in perf_command),
        )
        perf_env = os.environ.copy()
        perf_env.setdefault("MOLT_EXT_ROOT", str(ext_root))
        perf_env.setdefault("PYTHONPATH", "src")
        if api_token:
            perf_env.setdefault("MOLT_SYMPHONY_API_TOKEN", api_token)
        started = time.perf_counter()
        try:
            proc = subprocess.run(
                perf_command,
                cwd=repo_root,
                check=False,
                capture_output=True,
                text=True,
                env=perf_env,
                timeout=perf_timeout_s,
            )
            returncode = int(proc.returncode)
            stdout_tail = (proc.stdout or "").strip()[-400:]
            stderr_tail = (proc.stderr or "").strip()[-400:]
        except subprocess.TimeoutExpired as exc:
            returncode = 124
            stdout_tail = str(exc.stdout or "")[-400:]
            stderr_tail = str(exc.stderr or "timeout")[-400:]
        elapsed_ms = int(max((time.perf_counter() - started) * 1000.0, 0.0))
        perf_runs += 1
        next_perf_due_at = now + perf_interval_s
        _log(
            "INFO" if returncode == 0 else "WARNING",
            "watchdog_perf_finished",
            run=perf_runs,
            returncode=returncode,
            elapsed_ms=elapsed_ms,
            stdout_tail=stdout_tail,
            stderr_tail=stderr_tail,
        )

    while True:
        time.sleep(interval_s)
        current = _fingerprint(repo_root, patterns)
        now = time.monotonic()
        _maybe_run_health_check(now)
        _maybe_run_perf_guard(now)
        if current != previous:
            previous = current
            pending_change_at = now
            _log("INFO", "watchdog_change_detected")
            continue

        if pending_change_at is None:
            continue
        if now - pending_change_at < quiet_s:
            continue
        if now - last_restart_at < cooldown_s:
            continue
        if bool(args.restart_when_idle):
            busy, detail = _service_is_busy(
                activity_url,
                state_timeout_s,
                auth_token=api_token,
            )
            if busy is True:
                reason = f"busy:{detail}"
                if (reason != last_defer_reason) or (
                    now - last_defer_log_at >= defer_log_interval_s
                ):
                    _log(
                        "INFO",
                        "watchdog_restart_deferred_busy",
                        detail=detail,
                        activity_url=activity_url,
                    )
                    last_defer_log_at = now
                    last_defer_reason = reason
                continue
            if busy is None:
                reason = f"activity_unavailable:{detail}"
                if (reason != last_defer_reason) or (
                    now - last_defer_log_at >= defer_log_interval_s
                ):
                    _log(
                        "WARNING",
                        "watchdog_activity_probe_failed",
                        detail=detail,
                        activity_url=activity_url,
                    )
                    last_defer_log_at = now
                    last_defer_reason = reason
                continue

        if _restart_service(args.service_label):
            last_restart_at = now
            health_failure_count = 0
            health_grace_until = now + health_startup_grace_s
            pending_change_at = None
            last_defer_reason = ""


if __name__ == "__main__":
    raise SystemExit(main())
