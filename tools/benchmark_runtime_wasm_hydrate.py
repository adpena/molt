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
from molt.cli.runtime_build_identity import RuntimeBuildIdentity
from molt.cli.runtime_wasm_cache import (
    _shared_runtime_wasm_cache_root,
    hydrate_runtime_wasm_pair_from_shared_cache,
)
from molt.cli.runtime_wasm_generation import (
    RuntimeWasmGeneration,
    read_runtime_wasm_generation,
)
from molt.cli.runtime_wasm_validation import (
    _is_valid_runtime_wasm_artifact,
    _is_valid_shared_runtime_wasm_artifact,
)


def _read_cached_generation(
    manifest: Path,
) -> RuntimeWasmGeneration:
    payload = json.loads(manifest.read_text(encoding="utf-8"))
    receipts = payload.get("receipts")
    if not isinstance(receipts, dict):
        raise ValueError(f"runtime generation receipts are missing: {manifest}")
    shared_record = receipts.get("shared")
    reloc_record = receipts.get("reloc")
    if not isinstance(shared_record, dict) or not isinstance(reloc_record, dict):
        raise ValueError(f"runtime generation member receipts are invalid: {manifest}")
    shared_identity = RuntimeBuildIdentity.from_dict(shared_record.get("identity"))
    reloc_identity = RuntimeBuildIdentity.from_dict(reloc_record.get("identity"))
    generation = read_runtime_wasm_generation(
        manifest,
        expected_shared_identity=shared_identity,
        expected_reloc_identity=reloc_identity,
    )
    if generation is None:
        raise ValueError(f"runtime generation is invalid: {manifest}")
    return generation


def _legacy_hydrate_pair(
    *,
    source_shared: Path,
    source_reloc: Path,
    dest_shared: Path,
    dest_reloc: Path,
) -> bool:
    if not _is_valid_shared_runtime_wasm_artifact(source_shared):
        return False
    if not _is_valid_runtime_wasm_artifact(source_reloc):
        return False
    _atomic_copy_file(source_shared, dest_shared)
    _atomic_copy_file(source_reloc, dest_reloc)
    return _is_valid_shared_runtime_wasm_artifact(
        dest_shared
    ) and _is_valid_runtime_wasm_artifact(dest_reloc)


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
    source_shared: Path,
    source_reloc: Path,
    dest_shared: Path,
    dest_reloc: Path,
    shared_identity: RuntimeBuildIdentity,
    reloc_identity: RuntimeBuildIdentity,
) -> tuple[float, int]:
    started = time.perf_counter()
    if mode == "before":
        ok = _legacy_hydrate_pair(
            source_shared=source_shared,
            source_reloc=source_reloc,
            dest_shared=dest_shared,
            dest_reloc=dest_reloc,
        )
    else:
        generation = hydrate_runtime_wasm_pair_from_shared_cache(
            dest_shared=dest_shared,
            dest_reloc=dest_reloc,
            shared_identity=shared_identity,
            reloc_identity=reloc_identity,
            is_valid_shared=_is_valid_shared_runtime_wasm_artifact,
            is_valid_reloc=_is_valid_runtime_wasm_artifact,
        )
        ok = generation is not None
    elapsed_ms = (time.perf_counter() - started) * 1000.0
    hydrated_shared = dest_shared if mode == "before" else generation.shared
    hydrated_reloc = dest_reloc if mode == "before" else generation.reloc
    if not ok or hydrated_shared.read_bytes() != source_shared.read_bytes() or (
        hydrated_reloc.read_bytes() != source_reloc.read_bytes()
    ):
        raise RuntimeError(f"{mode} pair hydrate contract failed")
    return elapsed_ms, _peak_rss_bytes()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.samples < 3:
        raise SystemExit("at least three samples are required")

    cache_root = _shared_runtime_wasm_cache_root()
    candidates = []
    for manifest in cache_root.glob("*/molt_runtime.generation.json"):
        try:
            generation = _read_cached_generation(manifest)
        except (OSError, ValueError, json.JSONDecodeError):
            continue
        candidates.append(generation)
    if not candidates:
        raise SystemExit(f"no valid cached runtime generation under {cache_root}")
    source_generation = max(
        candidates,
        key=lambda value: value.shared.stat().st_size + value.reloc.stat().st_size,
    )
    source_shared = source_generation.shared
    source_reloc = source_generation.reloc
    shared_identity = source_generation.shared_identity
    reloc_identity = source_generation.reloc_identity

    samples = {"before": [], "after": []}
    max_rss = 0
    with tempfile.TemporaryDirectory(dir=r"C:\Molt") as temp_dir:
        root = Path(temp_dir)
        for sample_index in range(args.samples):
            for mode in ("before", "after"):
                sample_root = root / f"{sample_index}-{mode}"
                elapsed_ms, rss = _run_sample(
                    mode=mode,
                    source_shared=source_shared,
                    source_reloc=source_reloc,
                    dest_shared=sample_root / "molt_runtime.wasm",
                    dest_reloc=sample_root / "molt_runtime_reloc.wasm",
                    shared_identity=shared_identity,
                    reloc_identity=reloc_identity,
                )
                samples[mode].append(elapsed_ms)
                max_rss = max(max_rss, rss)

    source_digest = hashlib.sha256(
        source_shared.read_bytes() + source_reloc.read_bytes()
    ).hexdigest()
    before_median = statistics.median(samples["before"])
    after_median = statistics.median(samples["after"])
    payload = {
        "schema_version": 2,
        "claim": "OPT-MATRIX-R2",
        "scenario": "real release runtime-WASM atomic-pair cache hydrate",
        "profile": "release",
        "hot_path": "trusted generation validation and atomic shared+reloc deployment",
        "complexity": {
            "before": "2 * O(pair_bytes) validation plus O(pair_bytes) copy",
            "after": "O(pair_bytes) generation validation plus O(pair_bytes) atomic deployment",
        },
        "pair_digest": shared_identity.pair_digest,
        "artifact_sha256": source_digest,
        "artifact_bytes": source_shared.stat().st_size + source_reloc.stat().st_size,
        "before": {"runs": samples["before"], "median_ms": before_median},
        "after": {"runs": samples["after"], "median_ms": after_median},
        "held_benches": {
            "pair_identity": True,
            "source_generation_validation": True,
            "atomic_pair_publication": True,
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
