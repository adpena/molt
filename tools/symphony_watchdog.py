from __future__ import annotations

import argparse
import hashlib
import os
import shlex
import subprocess
import sys
import time
from pathlib import Path
from urllib.error import URLError
from urllib.request import urlopen


DEFAULT_PATTERNS = (
    "WORKFLOW.md",
    "ops/linear/runtime/symphony.env",
    "src/molt/symphony/**/*.py",
    "tools/symphony_run.py",
    "tools/symphony_entry.py",
    "tools/symphony_watchdog.py",
    "tools/symphony_launchd.py",
)


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


def _default_perf_command(
    *,
    repo_root: Path,
    env_file: Path,
    state_url: str,
    ext_root: Path,
    env_values: dict[str, str],
) -> list[str]:
    base_url = state_url.strip()
    if base_url.endswith("/api/v1/state"):
        base_url = base_url[: -len("/api/v1/state")]
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
        str(float(env_values.get("MOLT_SYMPHONY_PERF_MAX_AVG_REGRESSION_RATIO", "0.15"))),
        "--max-p95-regression-ratio",
        str(float(env_values.get("MOLT_SYMPHONY_PERF_MAX_P95_REGRESSION_RATIO", "0.20"))),
    ]
    if _bool_value(
        env_values.get("MOLT_SYMPHONY_PERF_LINEAR_SYNC"), default=False
    ):
        team = str(
            env_values.get("MOLT_LINEAR_TEAM")
            or env_values.get("MOLT_LINEAR_PROJECT_TEAM")
            or "Moltlang"
        ).strip()
        project = str(env_values.get("MOLT_LINEAR_PROJECT_SLUG") or "").split(",")[0].strip()
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
    uid = os.getuid()
    return f"gui/{uid}/{service_label}"


def _restart_service(service_label: str) -> None:
    target = _launchd_target(service_label)
    proc = subprocess.run(
        ["launchctl", "kickstart", "-k", target],
        check=False,
        capture_output=True,
        text=True,
    )
    if proc.returncode != 0:
        _log(
            "WARNING",
            "watchdog_restart_failed",
            target=target,
            returncode=proc.returncode,
            stderr=proc.stderr.strip(),
        )
        return
    _log("INFO", "watchdog_restarted_service", target=target)


def _read_state_counts(state_url: str, timeout_seconds: float) -> dict[str, int] | None:
    try:
        with urlopen(state_url, timeout=max(timeout_seconds, 0.5)) as resp:
            if int(getattr(resp, "status", 200)) != 200:
                return None
            raw = resp.read().decode("utf-8")
    except (OSError, URLError):
        return None
    try:
        import json

        payload = json.loads(raw)
    except Exception:
        return None
    if not isinstance(payload, dict):
        return None
    counts = payload.get("counts")
    if not isinstance(counts, dict):
        return None
    running = int(counts.get("running", 0) or 0)
    retrying = int(counts.get("retrying", 0) or 0)
    return {"running": max(running, 0), "retrying": max(retrying, 0)}


def _service_is_busy(state_url: str, timeout_seconds: float) -> tuple[bool | None, str]:
    counts = _read_state_counts(state_url, timeout_seconds)
    if counts is None:
        return None, "state_unavailable"
    running = int(counts.get("running", 0))
    retrying = int(counts.get("retrying", 0))
    busy = running > 0 or retrying > 0
    detail = f"running={running} retrying={retrying}"
    return busy, detail


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
    ext_root = Path(
        str(env_values.get("MOLT_EXT_ROOT") or os.environ.get("MOLT_EXT_ROOT") or "")
    ).expanduser()
    if not ext_root:
        ext_root = Path("/Volumes/APDataStore/Molt")
    patterns = tuple(DEFAULT_PATTERNS) + tuple(args.pattern)

    interval_s = max(int(args.interval_ms), 250) / 1000.0
    quiet_s = max(int(args.quiet_ms), 250) / 1000.0
    cooldown_s = max(int(args.cooldown_ms), 250) / 1000.0
    defer_log_interval_s = max(int(args.defer_log_interval_ms), 500) / 1000.0
    perf_interval_s = max(int(args.perf_interval_ms), 60_000) / 1000.0
    perf_timeout_s = max(int(args.perf_timeout_ms), 5_000) / 1000.0

    previous = _fingerprint(repo_root, patterns)
    pending_change_at: float | None = None
    last_restart_at = 0.0
    last_defer_log_at = 0.0
    last_defer_reason = ""
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
        defer_log_interval_ms=int(defer_log_interval_s * 1000),
        perf_enabled=perf_enabled,
        perf_interval_ms=int(perf_interval_s * 1000),
        target=_launchd_target(args.service_label),
    )
    state_timeout_s = max(int(args.state_timeout_ms), 100) / 1000.0

    def _maybe_run_perf_guard(now: float) -> None:
        nonlocal next_perf_due_at, perf_runs
        if not perf_enabled:
            return
        if now < next_perf_due_at:
            return
        if bool(args.perf_defer_when_busy):
            busy, detail = _service_is_busy(args.state_url, state_timeout_s)
            if busy is True:
                _log("INFO", "watchdog_perf_deferred_busy", detail=detail)
                next_perf_due_at = now + min(perf_interval_s / 8.0, 600.0)
                return
            if busy is None:
                _log("WARNING", "watchdog_perf_deferred_state_probe_failed", detail=detail)
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
            busy, detail = _service_is_busy(args.state_url, state_timeout_s)
            if busy is True:
                reason = f"busy:{detail}"
                if (reason != last_defer_reason) or (
                    now - last_defer_log_at >= defer_log_interval_s
                ):
                    _log(
                        "INFO",
                        "watchdog_restart_deferred_busy",
                        detail=detail,
                        state_url=args.state_url,
                    )
                    last_defer_log_at = now
                    last_defer_reason = reason
                continue
            if busy is None:
                reason = f"state_unavailable:{detail}"
                if (reason != last_defer_reason) or (
                    now - last_defer_log_at >= defer_log_interval_s
                ):
                    _log(
                        "WARNING",
                        "watchdog_state_probe_failed",
                        detail=detail,
                        state_url=args.state_url,
                    )
                    last_defer_log_at = now
                    last_defer_reason = reason
                continue

        _restart_service(args.service_label)
        last_restart_at = now
        pending_change_at = None
        last_defer_reason = ""


if __name__ == "__main__":
    raise SystemExit(main())
