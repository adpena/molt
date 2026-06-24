from __future__ import annotations

import contextlib
import json
from pathlib import Path
import sys
import time
from typing import Any

from tools.memory_guard_core.memory_limits import ResolvedMemoryLimits
from tools.memory_guard_core.payloads import _rss_record_payload, memory_limits_payload


DEFAULT_SAMPLES_MAX_MB = 2.0


def _samples_max_bytes_from_mb(value: float | None) -> int | None:
    if value is None:
        value = DEFAULT_SAMPLES_MAX_MB
    if value <= 0:
        return None
    return max(1024, int(value * 1024 * 1024))


def _rotate_jsonl_if_needed(
    path: Path, incoming_bytes: int, max_bytes: int | None
) -> None:
    if max_bytes is None:
        return
    try:
        current_size = path.stat().st_size
    except FileNotFoundError:
        return
    except OSError:
        return
    if current_size + incoming_bytes <= max_bytes:
        return
    rotated = path.with_name(f"{path.name}.1")
    with contextlib.suppress(OSError):
        rotated.unlink()
    with contextlib.suppress(OSError):
        path.replace(rotated)


def _append_sample_jsonl(
    path: str,
    *,
    root_pid: int,
    peak: Any | None,
    total: Any | None,
    violation: Any | None,
    max_bytes: int | None = None,
) -> None:
    sample_path = Path(path)
    if sample_path.parent:
        sample_path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "ts": time.time(),
        "root_pid": root_pid,
        "peak": _rss_record_payload(peak),
        "total": _rss_record_payload(total),
        "violation": _rss_record_payload(violation),
    }
    line = json.dumps(payload, sort_keys=True) + "\n"
    _rotate_jsonl_if_needed(sample_path, len(line.encode("utf-8")), max_bytes)
    with sample_path.open("a", encoding="utf-8") as handle:
        handle.write(line)


def _record_gb(record: object) -> str:
    if not isinstance(record, dict):
        return "-"
    value = record.get("rss_gb")
    if isinstance(value, (int, float)):
        return f"{value:.2f}GB"
    return "-"


def _format_sample_payload(payload: dict[str, object]) -> str:
    violation = payload.get("violation")
    if violation is not None:
        return f"memory_guard sample: TRIP violation={_record_gb(violation)}"
    return (
        "memory_guard sample: "
        f"peak={_record_gb(payload.get('peak'))} "
        f"total={_record_gb(payload.get('total'))}"
    )


def _stream_sample_payload(payload: dict[str, object], stream: str) -> None:
    if not stream:
        return
    target = sys.stdout if "stdout" in stream else sys.stderr
    try:
        if "json" in stream:
            print(json.dumps(payload, sort_keys=True), file=target, flush=True)
        else:
            print(_format_sample_payload(payload), file=target, flush=True)
    except Exception:
        return


def _record_sample(
    *,
    root_pid: int,
    peak: Any | None,
    total: Any | None,
    violation: Any | None,
    limits: ResolvedMemoryLimits | None = None,
    samples_jsonl: str | None,
    samples_jsonl_max_bytes: int | None,
    stream: str,
) -> None:
    payload = {
        "ts": time.time(),
        "root_pid": root_pid,
        "peak": _rss_record_payload(peak),
        "total": _rss_record_payload(total),
        "violation": _rss_record_payload(violation),
    }
    if limits is not None:
        payload["limits"] = memory_limits_payload(limits)
    if samples_jsonl is not None:
        sample_path = Path(samples_jsonl)
        if sample_path.parent:
            sample_path.parent.mkdir(parents=True, exist_ok=True)
        line = json.dumps(payload, sort_keys=True) + "\n"
        _rotate_jsonl_if_needed(
            sample_path,
            len(line.encode("utf-8")),
            samples_jsonl_max_bytes,
        )
        with sample_path.open("a", encoding="utf-8") as handle:
            handle.write(line)
    _stream_sample_payload(payload, stream)
