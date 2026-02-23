#!/usr/bin/env python3
from __future__ import annotations

import argparse
import importlib.util
import json
import re
import sys
import time
import types
from dataclasses import dataclass
from pathlib import Path
from types import ModuleType


SMOKE_BENCHMARKS = ("nbody", "fannkuch")
SUPPORTED_BENCHMARKS = frozenset(SMOKE_BENCHMARKS)
MANIFEST_REL_PATH = Path("pyperformance") / "data-files" / "benchmarks" / "MANIFEST"
BENCHMARK_ROOT_REL_PATH = Path("pyperformance") / "data-files" / "benchmarks"
GROUP_HEADER_RE = re.compile(r"^\[group ([^\]]+)\]\s*$")


@dataclass(frozen=True)
class ManifestCatalog:
    benchmark_names: tuple[str, ...]
    groups: tuple[str, ...]

    @property
    def benchmark_count(self) -> int:
        return len(self.benchmark_names)


def _manifest_path(suite_root: Path) -> Path:
    return suite_root / MANIFEST_REL_PATH


def _benchmark_root(suite_root: Path) -> Path:
    return suite_root / BENCHMARK_ROOT_REL_PATH


def _parse_manifest(path: Path) -> ManifestCatalog:
    if not path.exists():
        raise FileNotFoundError(f"pyperformance manifest not found: {path}")

    benchmark_names: list[str] = []
    groups: list[str] = []
    section: str | None = None

    for raw_line in path.read_text(encoding="utf-8").splitlines():
        line = raw_line.strip()
        if not line or line.startswith("#"):
            continue

        if line == "[benchmarks]":
            section = "benchmarks"
            continue

        group_match = GROUP_HEADER_RE.match(line)
        if group_match is not None:
            section = "group"
            groups.append(group_match.group(1))
            continue

        if line.startswith("["):
            section = None
            continue

        if section != "benchmarks":
            continue
        if line.startswith("name") and "metafile" in line:
            continue

        parts = line.split()
        if not parts:
            continue
        benchmark_names.append(parts[0])

    return ManifestCatalog(
        benchmark_names=tuple(benchmark_names),
        groups=tuple(sorted(set(groups))),
    )


def _install_pyperf_shim() -> None:
    if "pyperf" in sys.modules:
        return

    module = types.ModuleType("pyperf")
    module.perf_counter = time.perf_counter

    class _Runner:
        def __init__(self, *args: object, **kwargs: object) -> None:
            self.args = args
            self.kwargs = kwargs
            self.metadata: dict[str, object] = {}

        def parse_args(self) -> object:
            raise RuntimeError("pyperf.Runner shim does not support parse_args()")

        def bench_func(self, *args: object, **kwargs: object) -> None:
            raise RuntimeError("pyperf.Runner shim does not support bench_func()")

        def bench_time_func(self, *args: object, **kwargs: object) -> None:
            raise RuntimeError("pyperf.Runner shim does not support bench_time_func()")

    module.Runner = _Runner
    sys.modules["pyperf"] = module


def _load_benchmark_module(suite_root: Path, benchmark: str) -> ModuleType:
    if benchmark not in SUPPORTED_BENCHMARKS:
        supported = ", ".join(sorted(SUPPORTED_BENCHMARKS))
        raise ValueError(f"unsupported benchmark {benchmark!r}; supported: {supported}")

    script = _benchmark_root(suite_root) / f"bm_{benchmark}" / "run_benchmark.py"
    if not script.exists():
        raise FileNotFoundError(f"benchmark script not found: {script}")

    _install_pyperf_shim()
    module_name = f"_molt_pyperformance_{benchmark}_{time.time_ns()}"
    spec = importlib.util.spec_from_file_location(module_name, script)
    if spec is None or spec.loader is None:
        raise RuntimeError(f"failed to load benchmark module spec: {script}")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def _run_nbody(module: ModuleType, rounds: int) -> tuple[float, str]:
    elapsed_start = time.perf_counter()
    fingerprints: list[str] = []
    for _ in range(rounds):
        reference = module.BODIES[module.DEFAULT_REFERENCE]
        module.offset_momentum(reference)
        energy_start = module.report_energy()
        module.advance(0.01, 250)
        energy_end = module.report_energy()
        fingerprints.append(f"{energy_start:.9f}:{energy_end:.9f}")
    elapsed_s = time.perf_counter() - elapsed_start
    return elapsed_s, "|".join(fingerprints)


def _run_fannkuch(module: ModuleType, rounds: int) -> tuple[float, str]:
    elapsed_start = time.perf_counter()
    final_value: int | None = None
    for _ in range(rounds):
        final_value = module.fannkuch(8)
    elapsed_s = time.perf_counter() - elapsed_start
    return elapsed_s, str(final_value)


def _run_benchmark_once(
    suite_root: Path, benchmark: str, rounds: int
) -> dict[str, object]:
    module = _load_benchmark_module(suite_root, benchmark)
    if benchmark == "nbody":
        elapsed_s, fingerprint = _run_nbody(module, rounds)
    elif benchmark == "fannkuch":
        elapsed_s, fingerprint = _run_fannkuch(module, rounds)
    else:
        raise ValueError(f"unsupported benchmark {benchmark!r}")
    return {
        "benchmark": benchmark,
        "elapsed_s": elapsed_s,
        "result_fingerprint": fingerprint,
    }


def _parse_benchmark_csv(raw: str) -> tuple[str, ...]:
    values = [value.strip() for value in raw.split(",")]
    cleaned = [value for value in values if value]
    if not cleaned:
        return SMOKE_BENCHMARKS
    return tuple(cleaned)


def run_subset(
    suite_root: Path,
    *,
    benchmarks: tuple[str, ...],
    rounds: int,
) -> dict[str, object]:
    if rounds <= 0:
        raise ValueError("rounds must be positive")
    suite_root = suite_root.resolve()
    results = [
        _run_benchmark_once(suite_root, benchmark, rounds) for benchmark in benchmarks
    ]
    total_elapsed_s = sum(
        float(item["elapsed_s"])
        for item in results
        if isinstance(item["elapsed_s"], float)
    )
    return {
        "suite_root": str(suite_root),
        "benchmarks": list(benchmarks),
        "rounds": rounds,
        "results": results,
        "total_elapsed_s": total_elapsed_s,
    }


def catalog_suite(suite_root: Path) -> dict[str, object]:
    suite_root = suite_root.resolve()
    catalog = _parse_manifest(_manifest_path(suite_root))
    available = set(catalog.benchmark_names)
    return {
        "suite_root": str(suite_root),
        "benchmark_count": catalog.benchmark_count,
        "groups": list(catalog.groups),
        "smoke_benchmarks": list(SMOKE_BENCHMARKS),
        "smoke_available": [
            benchmark for benchmark in SMOKE_BENCHMARKS if benchmark in available
        ],
    }


def _cmd_catalog(args: argparse.Namespace) -> int:
    suite_root = Path(args.suite_root).expanduser().resolve()
    payload = catalog_suite(suite_root)
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        print(f"suite_root={suite_root}")
        print(f"benchmark_count={int(payload['benchmark_count'])}")
        print("groups=" + ",".join(payload["groups"]))
        print("smoke_available=" + ",".join(payload["smoke_available"]))
    return 0


def _cmd_run_subset(args: argparse.Namespace) -> int:
    suite_root = Path(args.suite_root).expanduser().resolve()
    benchmarks = _parse_benchmark_csv(args.benchmarks)
    payload = run_subset(
        suite_root,
        benchmarks=benchmarks,
        rounds=args.rounds,
    )
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        for item in payload["results"]:
            print(
                "benchmark={benchmark} elapsed_s={elapsed:.9f} fingerprint={fingerprint}".format(
                    benchmark=item["benchmark"],
                    elapsed=float(item["elapsed_s"]),
                    fingerprint=item["result_fingerprint"],
                )
            )
        print(f"total_elapsed_s={float(payload['total_elapsed_s']):.9f}")
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="PyPerformance smoke adapter for differential and friend benchmark lanes."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    catalog = subparsers.add_parser(
        "catalog",
        help="Inspect pyperformance MANIFEST and emit benchmark/group catalog.",
    )
    catalog.add_argument(
        "--suite-root", required=True, help="Path to pyperformance repo root."
    )
    catalog.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON catalog.",
    )
    catalog.set_defaults(func=_cmd_catalog)

    run_subset_cmd = subparsers.add_parser(
        "run-subset",
        help="Run the curated pyperformance smoke subset directly in-process.",
    )
    run_subset_cmd.add_argument(
        "--suite-root",
        required=True,
        help="Path to pyperformance repo (or compatible fixture) root.",
    )
    run_subset_cmd.add_argument(
        "--benchmarks",
        default=",".join(SMOKE_BENCHMARKS),
        help="Comma-separated benchmark names (default: smoke subset).",
    )
    run_subset_cmd.add_argument(
        "--rounds",
        type=int,
        default=1,
        help="Workload rounds per benchmark (default: 1).",
    )
    run_subset_cmd.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON run summary.",
    )
    run_subset_cmd.set_defaults(func=_cmd_run_subset)

    return parser


def main() -> int:
    parser = _build_parser()
    args = parser.parse_args()
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
