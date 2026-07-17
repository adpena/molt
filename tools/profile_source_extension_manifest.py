"""Profile exact source-extension manifest compaction, parsing, and hashing."""

from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import statistics
import threading
import time
import tracemalloc
import os
from collections.abc import Callable
from pathlib import Path
from typing import Any

from molt.cli.source_extension_manifest_codec import (
    _compact_source_extension_manifest,
    _manifest_sequence,
    _validate_compact_source_extension_manifest,
)


def _rss_bytes() -> int:
    if os.name == "nt":

        class ProcessMemoryCounters(ctypes.Structure):
            _fields_ = [
                ("cb", ctypes.c_ulong),
                ("PageFaultCount", ctypes.c_ulong),
                ("PeakWorkingSetSize", ctypes.c_size_t),
                ("WorkingSetSize", ctypes.c_size_t),
                ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPagedPoolUsage", ctypes.c_size_t),
                ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
                ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
                ("PagefileUsage", ctypes.c_size_t),
                ("PeakPagefileUsage", ctypes.c_size_t),
            ]

        counters = ProcessMemoryCounters()
        counters.cb = ctypes.sizeof(counters)
        get_current_process = ctypes.windll.kernel32.GetCurrentProcess
        get_current_process.restype = ctypes.c_void_p
        get_process_memory_info = ctypes.windll.psapi.GetProcessMemoryInfo
        get_process_memory_info.argtypes = (
            ctypes.c_void_p,
            ctypes.POINTER(ProcessMemoryCounters),
            ctypes.c_ulong,
        )
        get_process_memory_info.restype = ctypes.c_int
        handle = get_current_process()
        if not get_process_memory_info(handle, ctypes.byref(counters), counters.cb):
            raise OSError("GetProcessMemoryInfo failed")
        return int(counters.WorkingSetSize)
    statm = Path("/proc/self/statm")
    if statm.is_file():
        resident_pages = int(statm.read_text(encoding="ascii").split()[1])
        return resident_pages * os.sysconf("SC_PAGE_SIZE")
    import resource

    scale = 1 if os.uname().sysname == "Darwin" else 1024
    return int(resource.getrusage(resource.RUSAGE_SELF).ru_maxrss) * scale


def _measure(operation: Callable[[], Any], iterations: int) -> dict[str, float | int]:
    elapsed_ms: list[float] = []
    allocation_peaks: list[int] = []
    baseline_rss = _rss_bytes()
    peak_rss = baseline_rss
    stop = threading.Event()

    def sample() -> None:
        nonlocal peak_rss
        while not stop.wait(0.001):
            peak_rss = max(peak_rss, _rss_bytes())

    sampler = threading.Thread(target=sample, daemon=True)
    sampler.start()
    try:
        for _index in range(iterations):
            tracemalloc.start()
            started = time.perf_counter()
            operation()
            elapsed_ms.append((time.perf_counter() - started) * 1000.0)
            allocation_peaks.append(tracemalloc.get_traced_memory()[1])
            tracemalloc.stop()
    finally:
        stop.set()
        sampler.join()
    return {
        "iterations": iterations,
        "wall_ms_median": statistics.median(elapsed_ms),
        "wall_ms_min": min(elapsed_ms),
        "tracemalloc_peak_bytes": max(allocation_peaks),
        "rss_baseline_bytes": baseline_rss,
        "rss_peak_bytes": peak_rss,
        "rss_peak_delta_bytes": peak_rss - baseline_rss,
    }


def profile(path: Path, *, iterations: int) -> dict[str, Any]:
    raw = path.read_bytes()
    baseline_parse = _measure(lambda: json.loads(raw), iterations)
    manifest = json.loads(raw)
    closure = manifest.get("object_closure")
    objects = closure.get("objects") if isinstance(closure, dict) else None
    if not isinstance(objects, list):
        raise ValueError("manifest has no object closure")
    original_commands = [tuple(item["compile_command"]) for item in objects]
    compact_measurement = _measure(
        lambda: _compact_source_extension_manifest(manifest), 1
    )
    _validate_compact_source_extension_manifest(manifest)
    compact_objects = manifest["object_closure"]["objects"]
    reconstructed = [
        tuple(_manifest_sequence(manifest, item, "compile_command") or ())
        for item in compact_objects
    ]
    if reconstructed != original_commands:
        raise ValueError("manifest compaction changed exact compile argv")
    compact = json.dumps(manifest, sort_keys=True, indent=2).encode("utf-8") + b"\n"
    compact_parse = _measure(lambda: json.loads(compact), iterations)
    baseline_hash = _measure(lambda: hashlib.sha256(raw).digest(), iterations)
    compact_hash = _measure(lambda: hashlib.sha256(compact).digest(), iterations)
    authorities = manifest["build_authorities"]
    return {
        "schema_version": 1,
        "kind": "source-extension-manifest-profile",
        "path": str(path.resolve()),
        "object_count": len(compact_objects),
        "before_bytes": len(raw),
        "after_bytes": len(compact),
        "byte_ratio": len(compact) / len(raw),
        "bytes_removed": len(raw) - len(compact),
        "string_count": len(authorities["strings"]),
        "sequence_count": len(authorities["sequences"]),
        "baseline_parse": baseline_parse,
        "compaction": compact_measurement,
        "compact_parse": compact_parse,
        "baseline_sha256": baseline_hash,
        "compact_sha256": compact_hash,
        "exact_argv_reconstructed": True,
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--iterations", type=int, default=7)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.iterations <= 0:
        parser.error("--iterations must be positive")
    result = profile(args.manifest.expanduser().resolve(), iterations=args.iterations)
    encoded = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output is not None:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
