from __future__ import annotations

import os
from pathlib import Path


def _backend_daemon_log_tail(log_path: Path, *, max_lines: int = 30) -> str | None:
    try:
        lines = log_path.read_text(encoding="utf-8", errors="replace").splitlines()
    except OSError:
        return None
    if not lines:
        return None
    tail = lines[-max_lines:]
    return "\n".join(tail).strip() or None


_BACKEND_DAEMON_VERBOSE_LOG_MAX_BYTES = 1024 * 1024


def _backend_daemon_log_mark(log_path: Path) -> int:
    try:
        return log_path.stat().st_size
    except OSError:
        return 0


def _backend_daemon_log_since(
    log_path: Path,
    offset: int,
    *,
    max_bytes: int = _BACKEND_DAEMON_VERBOSE_LOG_MAX_BYTES,
) -> str | None:
    if max_bytes <= 0:
        max_bytes = _BACKEND_DAEMON_VERBOSE_LOG_MAX_BYTES
    try:
        size = log_path.stat().st_size
    except OSError:
        return None
    if offset < 0 or offset > size:
        offset = 0
    start = offset
    truncated = False
    if size - start > max_bytes:
        start = max(offset, size - max_bytes)
        truncated = start > offset
    try:
        with log_path.open("rb") as handle:
            handle.seek(start)
            data = handle.read()
    except OSError:
        return None
    text = data.decode("utf-8", errors="replace")
    if truncated:
        first_newline = text.find("\n")
        if first_newline >= 0:
            text = text[first_newline + 1 :]
        text = "...(daemon log truncated to recent output)\n" + text
    return text.strip() or None


# Maximum daemon log size before rotation. The daemon writes structured
# diagnostic lines for every compile and warm-cache decision; on long-running
# sessions this naturally grows multiple megabytes. Rotating at 5 MiB keeps
# tail-based diagnostics fast while preserving the most recent build context.
_BACKEND_DAEMON_LOG_MAX_BYTES = 5 * 1024 * 1024


def _backend_daemon_log_max_bytes_cached(raw: str) -> int:
    if not raw:
        return _BACKEND_DAEMON_LOG_MAX_BYTES
    try:
        parsed = int(raw)
    except ValueError:
        return _BACKEND_DAEMON_LOG_MAX_BYTES
    return parsed if parsed > 0 else _BACKEND_DAEMON_LOG_MAX_BYTES


def _backend_daemon_log_max_bytes() -> int:
    return _backend_daemon_log_max_bytes_cached(
        os.environ.get("MOLT_BACKEND_DAEMON_LOG_MAX_BYTES", "")
    )


def _rotate_backend_daemon_log_if_large(log_path: Path) -> None:
    """Rotate the daemon log to ``<log>.old`` when it exceeds the limit."""
    try:
        size = log_path.stat().st_size
    except OSError:
        return
    if size <= _backend_daemon_log_max_bytes():
        return
    rotated = log_path.with_name(f"{log_path.name}.old")
    try:
        if rotated.exists():
            rotated.unlink()
    except OSError:
        pass
    try:
        log_path.replace(rotated)
    except OSError:
        # Best-effort: keep daemon spawn responsive if rename fails.
        try:
            with log_path.open("wb"):
                pass
        except OSError:
            pass
