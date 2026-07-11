from __future__ import annotations

import argparse
import ctypes
import json
import statistics
import subprocess
import time
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "tools" / "native" / "deterministic_correlate1d.c"


def _compile(output: Path) -> None:
    subprocess.run(
        [
            "clang",
            "-O3",
            "-shared",
            "-fno-fast-math",
            "-ffp-contract=off",
            "-msse2",
            str(SOURCE),
            "-o",
            str(output),
        ],
        check=True,
    )


def _load_kernel(path: Path):
    library = ctypes.CDLL(str(path))
    signature = [
        np.ctypeslib.ndpointer(np.float64, flags="C_CONTIGUOUS"),
        ctypes.c_size_t,
        np.ctypeslib.ndpointer(np.float64, flags="C_CONTIGUOUS"),
        ctypes.c_size_t,
        np.ctypeslib.ndpointer(np.float32, flags="C_CONTIGUOUS"),
    ]
    for name in ("molt_correlate1d_scalar", "molt_correlate1d_sse2"):
        function = getattr(library, name)
        function.argtypes = signature
        function.restype = None
    row_signature = [
        np.ctypeslib.ndpointer(np.float64, flags="C_CONTIGUOUS"),
        ctypes.c_size_t,
        ctypes.c_size_t,
        ctypes.c_size_t,
        np.ctypeslib.ndpointer(np.float64, flags="C_CONTIGUOUS"),
        ctypes.c_size_t,
        np.ctypeslib.ndpointer(np.float32, flags="C_CONTIGUOUS"),
    ]
    for name in ("molt_correlate1d_scalar_rows", "molt_correlate1d_sse2_rows"):
        function = getattr(library, name)
        function.argtypes = row_signature
        function.restype = None
    return library


def _correlate_axis(
    array: np.ndarray, weights: np.ndarray, axis: int, function
) -> np.ndarray:
    radius = weights.size // 2
    moved = np.moveaxis(np.asarray(array, np.float32), axis, -1)
    rows = moved.reshape(-1, moved.shape[-1])
    result = np.empty(rows.shape, dtype=np.float32)
    padded = np.ascontiguousarray(
        np.pad(rows, ((0, 0), (radius, radius)), mode="symmetric"),
        dtype=np.float64,
    )
    function(
        padded,
        rows.shape[0],
        padded.shape[1],
        rows.shape[1],
        weights,
        radius,
        result,
    )
    return np.moveaxis(result.reshape(moved.shape), -1, axis)


def _gaussian(array: np.ndarray, weights: np.ndarray, function) -> np.ndarray:
    first = _correlate_axis(array, weights, 0, function)
    return _correlate_axis(first, weights, 1, function)


def _median_seconds(function, repeats: int) -> float:
    samples = []
    for _ in range(repeats):
        start = time.perf_counter()
        function()
        samples.append(time.perf_counter() - start)
    return statistics.median(samples)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--stages", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--repeats", type=int, default=11)
    args = parser.parse_args()

    from scipy import ndimage

    stages = np.load(args.stages, allow_pickle=False)
    source = np.asarray(stages["m12"], np.float32)
    weights = np.ascontiguousarray(stages["w_gauss_s2"], dtype=np.float64)
    args.output.parent.mkdir(parents=True, exist_ok=True)
    library_path = args.output.with_suffix(".dll")
    _compile(library_path)
    library = _load_kernel(library_path)

    scalar = _gaussian(source, weights, library.molt_correlate1d_scalar_rows)
    simd = _gaussian(source, weights, library.molt_correlate1d_sse2_rows)
    scipy_output = ndimage.gaussian_filter(source, sigma=2.0)
    if not np.array_equal(scalar.view(np.uint32), scipy_output.view(np.uint32)):
        raise SystemExit(
            "scalar reference is not bit-identical to scipy gaussian_filter"
        )
    if not np.array_equal(simd.view(np.uint32), scipy_output.view(np.uint32)):
        raise SystemExit("SIMD result is not bit-identical to scipy gaussian_filter")

    scalar_seconds = _median_seconds(
        lambda: _gaussian(source, weights, library.molt_correlate1d_scalar_rows),
        args.repeats,
    )
    simd_seconds = _median_seconds(
        lambda: _gaussian(source, weights, library.molt_correlate1d_sse2_rows),
        args.repeats,
    )
    payload = {
        "schema_version": 1,
        "fixture": str(args.stages),
        "shape": list(source.shape),
        "dtype": str(source.dtype),
        "radius": weights.size // 2,
        "repeats": args.repeats,
        "scalar_median_seconds": scalar_seconds,
        "simd_median_seconds": simd_seconds,
        "speedup": scalar_seconds / simd_seconds,
        "bit_identical_to_scipy": True,
        "compiler_flags": ["-O3", "-fno-fast-math", "-ffp-contract=off", "-msse2"],
    }
    args.output.write_text(
        json.dumps(payload, indent=2) + "\n", encoding="utf-8", newline="\n"
    )
    print(json.dumps(payload, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
