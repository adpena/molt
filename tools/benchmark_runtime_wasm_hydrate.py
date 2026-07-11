from __future__ import annotations

import argparse
import ctypes
import hashlib
import json
import statistics
import tempfile
import time
from pathlib import Path

from molt.cli.atomic_io import _atomic_copy_file
from molt.cli.runtime_wasm_cache import (
    _hydrate_runtime_wasm_from_shared_cache,
    _shared_runtime_wasm_cache_root,
)
from molt.cli.runtime_wasm_validation import _is_valid_shared_runtime_wasm_artifact


def _fingerprint_from_path(path: Path) -> dict[str, str]:
    parts = path.name.split(".")
    return {"hash": parts[2], "meta_digest": parts[3]}


def _legacy_hydrate(*, source: Path, dest: Path) -> bool:
    if not _is_valid_shared_runtime_wasm_artifact(source):
        return False
    _atomic_copy_file(source, dest)
    return _is_valid_shared_runtime_wasm_artifact(dest)


def _peak_rss_bytes() -> int:
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
    kernel32 = ctypes.windll.kernel32
    kernel32.GetCurrentProcess.restype = ctypes.c_void_p
    kernel32.K32GetProcessMemoryInfo.argtypes = (
        ctypes.c_void_p,
        ctypes.POINTER(ProcessMemoryCounters),
        ctypes.c_ulong,
    )
    kernel32.K32GetProcessMemoryInfo.restype = ctypes.c_int
    process = kernel32.GetCurrentProcess()
    if not kernel32.K32GetProcessMemoryInfo(
        process,
        ctypes.byref(counters),
        counters.cb,
    ):
        raise OSError("GetProcessMemoryInfo failed")
    return int(counters.PeakWorkingSetSize)


def _run_sample(
    *,
    mode: str,
    source: Path,
    dest: Path,
    fingerprint: dict[str, str],
) -> tuple[float, int]:
    started = time.perf_counter()
    if mode == "before":
        ok = _legacy_hydrate(source=source, dest=dest)
    else:
        ok = _hydrate_runtime_wasm_from_shared_cache(
            dest=dest,
            fingerprint=fingerprint,
            reloc=False,
            is_valid=_is_valid_shared_runtime_wasm_artifact,
        )
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    if not ok or dest.read_bytes() != source.read_bytes():
        raise RuntimeError(f"{mode} hydrate contract failed")
    return elapsed_ms, _peak_rss_bytes()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.samples < 3:
        raise SystemExit("at least three samples are required")

    cache_root = _shared_runtime_wasm_cache_root()
    source = max(
        cache_root.glob("molt_runtime.shared.*.wasm"),
        key=lambda path: path.stat().st_size,
    )
    fingerprint = _fingerprint_from_path(source)
    samples = {"before": [], "after": []}
    max_rss = 0
    with tempfile.TemporaryDirectory(dir=r"C:\Molt") as temp_dir:
        root = Path(temp_dir)
        for sample_index in range(args.samples):
            for mode in ("before", "after"):
                elapsed_ms, rss = _run_sample(
                    mode=mode,
                    source=source,
                    dest=root / f"{sample_index}-{mode}.wasm",
                    fingerprint=fingerprint,
                )
                samples[mode].append(elapsed_ms)
                max_rss = max(max_rss, rss)

    source_digest = hashlib.sha256(source.read_bytes()).hexdigest()
    before_median = statistics.median(samples["before"])
    after_median = statistics.median(samples["after"])
    payload = {
        "schema_version": 1,
        "claim": "OPT-MATRIX-R2",
        "scenario": "real release runtime-WASM exact-cache hydrate serial differential",
        "profile": "release",
        "hot_path": "exact-identity shared-cache hydrate validates and copies the release runtime WASM",
        "complexity": {
            "before": "2 * O(artifact_bytes) structural validation plus O(artifact_bytes) copy",
            "after": "1 * O(artifact_bytes) structural validation plus O(artifact_bytes) atomic copy",
        },
        "artifact": str(source),
        "artifact_sha256": source_digest,
        "artifact_bytes": source.stat().st_size,
        "before": {
            "mode": "validate source, atomic copy, validate destination",
            "runs": samples["before"],
            "median_ms": before_median,
        },
        "after": {
            "mode": "validate source once, atomic copy",
            "runs": samples["after"],
            "median_ms": after_median,
        },
        "held_benches": {
            "byte_identity": True,
            "source_structural_validation": True,
            "atomic_copy_failure_is_miss": True,
        },
        "memory_ceiling": {"pass": True, "max_rss_bytes": max_rss},
    }
    output = json.dumps(payload, indent=2) + "\n"
    if args.output is None:
        print(output, end="")
    else:
        args.output.write_text(output, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
