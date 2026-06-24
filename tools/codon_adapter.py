#!/usr/bin/env python3
"""Drive Codon's `bench/codon/` kernels across the CPython / molt / codon lanes.

This is suite adapter ``S3`` of
``docs/design/foundation/69_benchmark_corpus_union_and_dynamic_calibration.md``,
seeded by Appendix ``69A`` §A.5 (the Codon AOT/native-reference axis). It mirrors
the off-the-shelf custody posture of ``tools/tinygrad_off_shelf_adapter.py`` and
``tools/numpy_off_shelf_adapter.py``:

  * The Codon ``bench/codon/`` Python sources are run **unmodified** from a pinned
    upstream checkout (``--suite-root <codon repo>``). The adapter never vendors,
    patches, or translates them — it is a benchmark *driver*, not a Codon fork.
  * Each kernel is measured on three lanes via the SAME child-process measurement
    path so the wall-clock numbers are directly comparable:
        - ``cpython`` : ``<python> bench/codon/<kernel>.py <args>``
        - ``molt``    : ``molt run --profile dev bench/codon/<kernel>.py -- <args>``
        - ``codon``   : ``codon build -release <kernel src>`` then run the binary.
    Lane selection is ``--lane``; the harness invokes the adapter once per lane
    with the lane's own runner ``env``/interpreter (see
    ``bench/friends/manifest.toml``), so the adapter resolves which concrete
    command a lane maps to and executes it.

Time AND cross-platform peak RSS come from
``tools/perf_calibration.run_and_measure`` (the C4 substrate that fixes the
Windows ``RSS=0`` gap), so every lane reports identically on every host molt
targets.

Semantic honesty (69A §A.0.2 / §A.5 — turn-blocking):
  Codon is NOT semantically equivalent on three zones — (a) arbitrary-precision
  integers (Codon ``int`` is 64-bit and wraps silently), (b) Unicode (Codon
  ``str`` is ASCII-only), (c) C-semantics numerics (negative ``//``/``%`` truncate
  toward zero, overflow wraps) — plus dict-insertion-order and dynamic dispatch.
  Every kernel carries an explicit ``codon_equivalent`` flag and a
  ``non_equivalent_reason``. A ``codon`` lane result on a non-equivalent kernel is
  reported but MUST NEVER be scored as a Codon win/loss; the adapter sets
  ``"scored_against_codon": false`` on those so the scoreboard cannot count them.

This adapter does not download the genomics/TAQ data corpus (run.sh ``get_data``).
Kernels that need an external data file (``taq``, ``word_count``) are
``data_gated``: their command requires ``--data <path>`` and they are skipped
(reported, not failed) when no data file is supplied.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path
from typing import Any, Sequence

# Import the just-landed cross-platform measurement substrate (time + peak RSS).
_TOOLS_DIR = Path(__file__).resolve().parent
if str(_TOOLS_DIR) not in sys.path:
    sys.path.insert(0, str(_TOOLS_DIR))

import perf_calibration  # noqa: E402


LANES = ("cpython", "molt", "codon")
BENCH_REL = Path("bench") / "codon"


# ---------------------------------------------------------------------------
# Kernel catalog — the runnable `bench/codon/` set (69A §A.5), with the exact
# input args from `bench/run.sh` + `bench/README.md`, and the per-kernel Codon
# semantic-equivalence verdict from `bench/corpus/registry.toml` / 69A §A.0.2.
#
#   id                : canonical id (matches registry.toml [[benchmark]].id)
#   py_source         : the Python source filename run by cpython/molt lanes
#   codon_source      : the source compiled by `codon build` (a `.codon` variant
#                       when one exists — it adds @par/@gpu but takes identical
#                       argv — else the shared `.py`)
#   args              : argv passed to BOTH the Python and the compiled binary,
#                       verbatim from run.sh
#   codon_equivalent  : True iff a codon-lane time is a legitimate win/loss
#                       (semantics match CPython on the measured input)
#   non_equivalent_reason : REQUIRED when codon_equivalent is False
#   data_gated        : True iff the kernel needs an external data file (the
#                       run.sh `get_data` corpus) supplied via --data
#   notes             : Codon-specific build flavor / fidelity caveats
# ---------------------------------------------------------------------------
KERNELS: dict[str, dict[str, Any]] = {
    # --- FP / numeric kernels: Codon's strongest EQUIVALENT turf (IEEE double,
    #     64-bit-safe int ranges). These are the bake-off cells. ---
    "nbody": {
        "py_source": "nbody.py",
        "codon_source": "nbody.py",  # no .codon variant; CLBG all-pairs FP loop
        "args": ["10000000"],  # run.sh: 10M steps (~6s)
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": "CLBG n-body; scalar FP. run.sh compiles the .py directly.",
    },
    "spectral_norm": {
        "py_source": "spectral_norm.py",
        "codon_source": "spectral_norm.py",
        "args": [],  # loops=100 hard-coded; DEFAULT_N=260
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": "CLBG spectral-norm; FP + nested loops + implicit sqrt.",
    },
    "float": {
        "py_source": "float.py",
        "codon_source": "float.py",
        "args": [],  # POINTS=10_000_000 hard-coded
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": "Unladen/Factor float kernel; 10M Point objects (sin/cos/sqrt).",
    },
    "chaos": {
        "py_source": "chaos.py",
        "codon_source": "chaos.codon",  # identical argv; .codon is the bench entry
        "args": [os.devnull],  # run.sh: `/dev/null` output-image path (argv[1])
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": (
            "Chaosgame fractal FP. argv[1] is the PPM output path; run.sh writes "
            "to /dev/null. Uses random.seed(1234) for reproducibility."
        ),
    },
    "mandelbrot": {
        "py_source": "mandelbrot.py",
        # NOTE: run.sh has `bench codon/mandelbrot.codon` commented out (the
        # .codon variant adds @par(gpu=True) and needs a GPU). The .py form is a
        # pure scalar CPU kernel — run THAT on all three lanes for an apples-to-
        # apples CPU bake-off (the GPU variant is a separate, non-comparable axis).
        "codon_source": "mandelbrot.py",
        "args": [],  # N=4096, MAX=1000 hard-coded
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": (
            "Scalar Mandelbrot (complex arithmetic). The bench's mandelbrot.codon "
            "is GPU (@par(gpu=True)) and is disabled in run.sh; the CPU .py form "
            "is the comparable kernel. complex() is IEEE-double — equivalent."
        ),
    },
    "go": {
        "py_source": "go.py",
        "codon_source": "go.codon",  # identical: no argv, __main__ guard
        "args": [],
        "codon_equivalent": True,
        "non_equivalent_reason": (
            "equivalent only insofar as the Go board dict use is order-"
            "independent; Codon dict does not preserve insertion order"
        ),
        "data_gated": False,
        "notes": (
            "Unladen Go-board AI; dict churn + Zobrist hashing. Order-independent "
            "dict use keeps it equivalent. random.seed per game for determinism."
        ),
    },
    "set_partition": {
        "py_source": "set_partition.py",
        "codon_source": "set_partition.py",
        "args": ["15"],  # run.sh: 15 (~15s)
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": "Set-partition enumeration; recursion + list building, 64-bit ints.",
    },
    "primes": {
        "py_source": "primes.py",
        "codon_source": "primes.codon",  # adds @par(schedule='dynamic'); identical argv
        "args": ["100000"],  # run.sh: 100000 (~3s)
        "codon_equivalent": True,
        "non_equivalent_reason": (
            "equivalent within 64-bit int range (the measured limit stays in "
            "64-bit); the .codon variant is multithreaded via @par but the "
            "single-thread headline is the comparable cell"
        ),
        "data_gated": False,
        "notes": (
            "Trial-division prime count below limit. primes.codon adds "
            "@par(schedule='dynamic') (multithreaded) — that is a separate "
            "parallel axis; compile WITHOUT -fopenmp for the single-thread bar."
        ),
    },
    "binary_trees": {
        "py_source": "binary_trees.py",
        "codon_source": "binary_trees.codon",  # identical argv (depth)
        "args": ["20"],  # run.sh: 20 (~6s)
        "codon_equivalent": True,
        "non_equivalent_reason": None,
        "data_gated": False,
        "notes": (
            "Boehm GCBench binary trees — the alloc/GC-stress kernel (the molt "
            "RC-overhead exposer). Codon uses Boehm GC; pools/arenas prohibited."
        ),
    },
    "bench_sum": {
        "py_source": "sum.py",
        "codon_source": "sum.py",
        "args": [],  # sum 1..50_000_000 hard-coded
        # 50M partial sums peak near 1.25e15 < 2**63 (~9.2e18): the bench input
        # is 64-bit-SAFE, so the Codon `sum` kernel IS equivalent. The registry's
        # `bench_sum` "stress" preset (sum crossing 2**63) is non-equivalent, but
        # that preset is NOT this Codon bench source — it is a separate molt-stress
        # program. This kernel runs only the 64-bit-safe Codon `sum`.
        "codon_equivalent": True,
        "non_equivalent_reason": (
            "the 1..50M Codon `sum` is 64-bit-safe (peak ~1.25e15 < 2**63) and IS "
            "equivalent; only the registry's separate molt-stress large-int "
            "variant (sum crossing 2**63) would overflow Codon's 64-bit int"
        ),
        "data_gated": False,
        "notes": (
            "Registry id is `bench_sum` (canonical), source is Codon `sum.py`. "
            "Accumulate 0..50_000_000."
        ),
    },
    # --- fannkuch: CLBG int-array kernel. The .codon variant adds @par(dynamic). ---
    "fannkuch": {
        "py_source": "fannkuch.py",
        "codon_source": "fannkuch.codon",  # adds @par(schedule='dynamic'); identical argv
        "args": ["11"],  # run.sh: 11 (~6s)
        "codon_equivalent": True,
        "non_equivalent_reason": (
            "int + array + in-place reverse, all 64-bit-safe — equivalent; the "
            ".codon variant is multithreaded via @par (separate parallel axis)"
        ),
        "data_gated": False,
        "notes": (
            "FANNKUCH permutation/flip kernel. fannkuch.codon adds "
            "@par(schedule='dynamic'); single-thread is the comparable cell."
        ),
    },
    # --- Data-gated real-world kernels (need the run.sh get_data corpus) ---
    "taq": {
        "py_source": "taq.py",
        "codon_source": "taq.py",
        # taq.py reads the file from argv[1] (NOT the DATA_TAQ env var the README
        # mentions — the README documents the wrapper convention; the source reads
        # argv). Supplied via --data; first 10M lines of an NYSE TAQ NBBO file.
        "args": [],  # data path is injected as the sole arg from --data
        "data_arg_position": "argv1",
        "codon_equivalent": True,
        "non_equivalent_reason": (
            "numeric/tabular peak detection within Codon's typed model "
            "(timestamps/prices fit 64-bit); ASCII parsing — equivalent"
        ),
        "data_gated": True,
        "notes": (
            "NYSE TAQ volume-peak detection. Needs first ~10M lines of an "
            "EQY_US_ALL_NBBO file (run.sh get_data). Path passed as argv[1]."
        ),
    },
    "word_count": {
        "py_source": "word_count.py",
        "codon_source": "word_count.py",
        # word_count.py reads argv[-1] as the filename.
        "args": [],  # data path is injected as the sole arg from --data
        "data_arg_position": "argv_last",
        "codon_equivalent": True,
        "non_equivalent_reason": (
            "dict frequency count is order-independent and the corpus is ASCII — "
            "equivalent; an order-observing or Unicode variant would not be"
        ),
        "data_gated": True,
        "notes": (
            "Word-frequency dict count. Needs a text corpus (run.sh uses the TAQ "
            "file). Path passed as argv[-1]."
        ),
    },
}


def _selected_kernels(name: str) -> list[str]:
    if name == "all":
        return sorted(KERNELS)
    if name not in KERNELS:
        raise ValueError(f"unknown kernel {name!r}; expected one of {sorted(KERNELS)}")
    return [name]


def _resolve_kernel_args(kernel: dict[str, Any], data_path: Path | None) -> list[str]:
    """Compute the concrete argv for a kernel, injecting the data file when gated."""
    args = list(kernel["args"])
    if not kernel["data_gated"]:
        return args
    if data_path is None:
        raise RuntimeError("data_gated kernel resolved without a data path")
    # Both data-gated kernels take exactly one path arg (argv1 / argv_last); with
    # no other args the position is equivalent — append the resolved path.
    return args + [str(data_path)]


def _cpython_cmd(python: str, py_path: Path, args: Sequence[str]) -> list[str]:
    return [python, str(py_path), *args]


def _molt_cmd(
    molt: Sequence[str], py_path: Path, args: Sequence[str], profile: str
) -> list[str]:
    # `molt run --profile <p> <script> -- <args>` mirrors the tinygrad/pyperf lanes.
    cmd = [*molt, "run", "--profile", profile, str(py_path)]
    if args:
        cmd.append("--")
        cmd.extend(args)
    return cmd


def _codon_run_binary(
    binary: Path, args: Sequence[str]
) -> list[str]:
    return [str(binary), *args]


def _build_codon_binary(
    codon: str,
    src_path: Path,
    out_path: Path,
    *,
    timeout: float | None,
    env: dict[str, str] | None,
) -> perf_calibration.RunMeasurement:
    # `codon build -release <src> -o <out>` (run.sh: `codon build -release`).
    # Compile time is captured separately so the run lane is pure execution.
    out_path.parent.mkdir(parents=True, exist_ok=True)
    cmd = [codon, "build", "-release", str(src_path), "-o", str(out_path)]
    return perf_calibration.run_and_measure(cmd, timeout=timeout, env=env)


def _measure_lane(
    lane: str,
    kernel_id: str,
    kernel: dict[str, Any],
    *,
    suite_root: Path,
    iterations: int,
    timeout: float | None,
    python: str,
    molt: Sequence[str],
    codon: str,
    profile: str,
    data_path: Path | None,
    codon_build_dir: Path,
) -> dict[str, Any]:
    """Run one kernel on one lane `iterations` times; return the lane record."""
    bench_dir = suite_root / BENCH_REL
    py_path = bench_dir / kernel["py_source"]
    codon_src = bench_dir / kernel["codon_source"]

    record: dict[str, Any] = {
        "lane": lane,
        "kernel": kernel_id,
        "codon_equivalent": kernel["codon_equivalent"],
        "non_equivalent_reason": kernel["non_equivalent_reason"],
        # A codon-lane time on a non-equivalent kernel is reported but NEVER scored.
        "scored_against_codon": (
            kernel["codon_equivalent"] if lane == "codon" else True
        ),
        "data_gated": kernel["data_gated"],
    }

    if lane != "codon" and not py_path.exists():
        record["status"] = "missing_source"
        record["detail"] = f"python source not found: {py_path}"
        return record
    if lane == "codon" and not codon_src.exists():
        record["status"] = "missing_source"
        record["detail"] = f"codon source not found: {codon_src}"
        return record

    if kernel["data_gated"] and data_path is None:
        record["status"] = "skipped"
        record["detail"] = (
            f"kernel {kernel_id!r} needs an external data file (run.sh get_data "
            "corpus); supply --data <path> to enable"
        )
        return record

    args = _resolve_kernel_args(kernel, data_path)
    record["args"] = args

    # Resolve the concrete command for this lane.
    build_info: dict[str, Any] | None = None
    if lane == "cpython":
        if shutil.which(python) is None and not Path(python).exists():
            record["status"] = "unavailable"
            record["detail"] = f"python interpreter not found: {python}"
            return record
        run_cmd = _cpython_cmd(python, py_path, args)
    elif lane == "molt":
        run_cmd = _molt_cmd(molt, py_path, args, profile)
    elif lane == "codon":
        if shutil.which(codon) is None and not Path(codon).exists():
            record["status"] = "unavailable"
            record["detail"] = (
                f"codon compiler not found: {codon} (install via exaloop.io/install.sh)"
            )
            return record
        binary = codon_build_dir / f"{kernel_id}.exe"
        build = _build_codon_binary(
            codon, codon_src, binary, timeout=timeout, env=None
        )
        build_info = {
            "cmd": [codon, "build", "-release", str(codon_src), "-o", str(binary)],
            "returncode": build.returncode,
            "elapsed_s": build.elapsed_s,
            "peak_rss_bytes": build.peak_rss_bytes,
            "timed_out": build.timed_out,
        }
        if build.returncode != 0 or build.timed_out:
            record["status"] = "build_failed"
            record["detail"] = (build.stderr or build.stdout or "").strip()[-2000:]
            record["build"] = build_info
            return record
        run_cmd = _codon_run_binary(binary, args)
    else:
        raise ValueError(f"unknown lane {lane!r}")

    record["cmd"] = run_cmd
    if build_info is not None:
        record["build"] = build_info

    samples_s: list[float] = []
    peak_rss: list[int] = []
    last_stdout = ""
    for _ in range(iterations):
        m = perf_calibration.run_and_measure(run_cmd, timeout=timeout, env=None)
        if m.returncode != 0 or m.timed_out:
            record["status"] = "run_failed"
            record["returncode"] = m.returncode
            record["timed_out"] = m.timed_out
            record["detail"] = (m.stderr or m.stdout or "").strip()[-2000:]
            return record
        samples_s.append(m.elapsed_s)
        if m.peak_rss_bytes:
            peak_rss.append(m.peak_rss_bytes)
        last_stdout = m.stdout

    stats = perf_calibration._summarize(samples_s)
    record["status"] = "ok"
    record["iterations"] = iterations
    # `elapsed_s` is the field bench_friends._extract_structured_elapsed reads
    # (median wall time across iterations) — keep it the median for stability.
    record["elapsed_s"] = stats.median
    record["wall_s_median"] = stats.median
    record["wall_s_mean"] = stats.mean
    record["wall_s_min"] = min(samples_s)
    record["wall_s_samples"] = samples_s
    record["wall_s_cv"] = stats.cv
    record["peak_rss_bytes_max"] = max(peak_rss) if peak_rss else None
    # The kernels print their own internal timing on the last line + a result on
    # the line before; keep a short fingerprint for differential cross-checking.
    record["stdout_tail"] = last_stdout.strip()[-400:]
    return record


def run_suite(
    *,
    suite_root: Path,
    lane: str,
    kernel: str,
    iterations: int,
    timeout: float | None,
    python: str,
    molt: Sequence[str],
    codon: str,
    profile: str,
    data_path: Path | None,
    codon_build_dir: Path,
) -> dict[str, Any]:
    suite_root = suite_root.resolve()
    bench_dir = suite_root / BENCH_REL
    if not bench_dir.is_dir():
        raise FileNotFoundError(
            f"codon bench dir not found: {bench_dir} (expected a pinned exaloop/"
            "codon checkout via --suite-root)"
        )
    selected = _selected_kernels(kernel)
    fp = perf_calibration.host_fingerprint()
    results: dict[str, Any] = {}
    for kid in selected:
        results[kid] = {
            "iterations": iterations,
            **_measure_lane(
                lane,
                kid,
                KERNELS[kid],
                suite_root=suite_root,
                iterations=iterations,
                timeout=timeout,
                python=python,
                molt=molt,
                codon=codon,
                profile=profile,
                data_path=data_path,
                codon_build_dir=codon_build_dir,
            ),
        }
    # `status: ok` unless a workload hit a hard error (missing source / run/build
    # failure). `skipped`/`unavailable` are honest non-errors (data-gated kernel
    # with no corpus, or codon/python not installed) — they keep the suite ok so
    # the available lanes still report (bench_friends treats non-ok JSON status as
    # a failure).
    hard_fail = any(
        entry.get("status") in {"missing_source", "run_failed", "build_failed"}
        for entry in results.values()
    )
    return {
        "status": "error" if hard_fail else "ok",
        "adapter": "codon_adapter",
        "suite": "codon_benchmarks",
        "lane": lane,
        "suite_root": str(suite_root),
        "host_fingerprint": {
            **perf_calibration.asdict(fp),
            "key": fp.key(),
        },
        "data_path": str(data_path) if data_path else None,
        "workloads": results,
    }


def _parse_molt(raw: str) -> list[str]:
    parts = raw.split()
    if not parts:
        raise ValueError("--molt must be a non-empty command")
    return parts


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=(
            "Run Codon bench/codon kernels across cpython/molt/codon lanes "
            "(doc 69 S3 adapter)."
        )
    )
    parser.add_argument(
        "--suite-root",
        type=Path,
        help="Path to a pinned exaloop/codon checkout (contains bench/codon/). "
        "Required for every command except --list-kernels.",
    )
    parser.add_argument(
        "--lane",
        choices=LANES,
        default="cpython",
        help="Which engine lane to measure (the harness drives one lane per call).",
    )
    parser.add_argument("--kernel", default="all", help="Kernel id or 'all'.")
    parser.add_argument(
        "--iterations",
        type=int,
        default=5,
        help="Repeat count per kernel (median wall time is reported).",
    )
    parser.add_argument(
        "--timeout",
        type=float,
        default=1800.0,
        help="Per-process wall-clock cap (seconds); 0 disables.",
    )
    parser.add_argument(
        "--python",
        default=sys.executable,
        help="CPython interpreter for the cpython lane.",
    )
    parser.add_argument(
        "--molt",
        default="molt",
        help="molt CLI command for the molt lane (space-split, e.g. 'molt' or "
        "'python -m molt.cli').",
    )
    parser.add_argument(
        "--codon",
        default="codon",
        help="codon compiler command for the codon lane.",
    )
    parser.add_argument(
        "--profile",
        default="dev",
        help="molt run profile for the molt lane (default: dev).",
    )
    parser.add_argument(
        "--data",
        type=Path,
        help="External data file for data-gated kernels (taq, word_count).",
    )
    parser.add_argument(
        "--codon-build-dir",
        type=Path,
        help="Directory for compiled codon binaries (default: <suite-root>/"
        "run/exe under a temp-safe path).",
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON to stdout.")
    parser.add_argument(
        "--list-kernels", action="store_true", help="List kernel ids and exit."
    )
    args = parser.parse_args(argv)

    if args.list_kernels:
        payload = {
            "kernels": {
                kid: {
                    "codon_equivalent": k["codon_equivalent"],
                    "data_gated": k["data_gated"],
                    "args": k["args"],
                    "non_equivalent_reason": k["non_equivalent_reason"],
                }
                for kid, k in sorted(KERNELS.items())
            }
        }
        print(json.dumps(payload, indent=2, sort_keys=True))
        return 0
    if args.suite_root is None:
        parser.error("--suite-root is required (except for --list-kernels)")
    if args.iterations <= 0:
        raise ValueError("--iterations must be positive")

    timeout = args.timeout if args.timeout and args.timeout > 0 else None
    codon_build_dir = (
        args.codon_build_dir
        if args.codon_build_dir is not None
        else args.suite_root.resolve() / "run" / "exe"
    )
    payload = run_suite(
        suite_root=args.suite_root,
        lane=args.lane,
        kernel=args.kernel,
        iterations=args.iterations,
        timeout=timeout,
        python=args.python,
        molt=_parse_molt(args.molt),
        codon=args.codon,
        profile=args.profile,
        data_path=args.data,
        codon_build_dir=codon_build_dir,
    )
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        for kid, entry in payload["workloads"].items():
            status = entry.get("status", "?")
            if status == "ok":
                rss = entry.get("peak_rss_bytes_max")
                rss_s = f" peak_rss={rss}" if rss else ""
                print(
                    f"{args.lane}/{kid}: {entry['elapsed_s']:.6f}s "
                    f"(median of {entry['iterations']}){rss_s}"
                )
            else:
                print(f"{args.lane}/{kid}: {status} ({entry.get('detail', '')})")
    return 0 if payload["status"] == "ok" else 1


if __name__ == "__main__":
    raise SystemExit(main())
