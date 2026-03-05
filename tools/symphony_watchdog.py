from __future__ import annotations

import argparse
import hashlib
import os
import subprocess
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
    parser.add_argument("--service-label", default="com.molt.symphony")
    parser.add_argument("--interval-ms", type=int, default=1500)
    parser.add_argument("--quiet-ms", type=int, default=1200)
    parser.add_argument("--cooldown-ms", type=int, default=5000)
    parser.add_argument("--state-url", default="http://127.0.0.1:8089/api/v1/state")
    parser.add_argument("--state-timeout-ms", type=int, default=600)
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
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    repo_root = Path(args.repo_root).resolve()
    patterns = tuple(DEFAULT_PATTERNS) + tuple(args.pattern)

    interval_s = max(int(args.interval_ms), 250) / 1000.0
    quiet_s = max(int(args.quiet_ms), 250) / 1000.0
    cooldown_s = max(int(args.cooldown_ms), 250) / 1000.0

    previous = _fingerprint(repo_root, patterns)
    pending_change_at: float | None = None
    last_restart_at = 0.0
    _log(
        "INFO",
        "watchdog_started",
        repo_root=repo_root,
        patterns=len(patterns),
        interval_ms=int(interval_s * 1000),
        quiet_ms=int(quiet_s * 1000),
        cooldown_ms=int(cooldown_s * 1000),
        target=_launchd_target(args.service_label),
    )
    state_timeout_s = max(int(args.state_timeout_ms), 100) / 1000.0

    while True:
        time.sleep(interval_s)
        current = _fingerprint(repo_root, patterns)
        now = time.monotonic()
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
                _log(
                    "INFO",
                    "watchdog_restart_deferred_busy",
                    detail=detail,
                    state_url=args.state_url,
                )
                continue
            if busy is None:
                _log(
                    "WARNING",
                    "watchdog_state_probe_failed",
                    detail=detail,
                    state_url=args.state_url,
                )

        _restart_service(args.service_label)
        last_restart_at = now
        pending_change_at = None


if __name__ == "__main__":
    raise SystemExit(main())
