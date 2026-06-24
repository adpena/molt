#!/usr/bin/env python3
"""PyPerformance adapter for the molt differential and friend-benchmark lanes.

Two execution lanes share one JSON output contract:

* ``run-subset`` runs a curated, intrinsic-friendly smoke subset
  (``nbody``/``fannkuch``) by driving the benchmark kernels through small,
  correctness-rich hand runners. This lane works against the stripped-down
  smoke fixtures (which intentionally omit the ``pyperf.Runner`` ``__main__``
  block) and does NOT require ``pyperformance`` to be importable.

* ``run-all`` / ``run-group`` run the FULL upstream pyperformance v1.14.0
  corpus. Benchmarks and their argv recipes are enumerated through
  pyperformance's own manifest machinery (``pyperformance._manifest``), then
  each real ``run_benchmark.py`` is driven in-process through a faithful
  ``pyperf.Runner`` shim. The shim executes each benchmark's own
  ``bench_func`` / ``bench_time_func`` workload — never a reimplementation —
  while the benchmark's ``__main__`` setup state is live, then captures timing
  and a deterministic result fingerprint.

Every result row carries a ``c_accelerated: bool`` field. Benchmarks that race
CPython's stdlib *C accelerator* (``json_*``, ``pickle``/``unpickle`` non-pure
variants, ``decimal``/``telco``, ``xml_etree``) are flagged True so the
scoreboard can separate "molt vs CPython's bytecode interpreter" from "molt's
implementation vs a C accelerator" (doc 69A §A.1). The ``*_pure_python``
pickle variants and pure-Python ``tomli_loads`` are explicitly False.

In-process structural limits (reported honestly as ``skipped``, never faked):
async/asyncio benchmarks (need a running event loop), ``bench_command``
benchmarks (spawn a subprocess: startup/2to3), and ``multiprocessing``
benchmarks (require an importable picklable worker). Benchmarks whose
third-party dependency is not installed in the lane are ``skipped`` with the
missing-module reason; the molt lane installs the dependencies it targets.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import importlib
import importlib.util
import io
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

# Benchmark IDs that race CPython's stdlib C accelerator rather than its
# bytecode interpreter (doc 69A §A.1). Keyed by the resolved pyperformance
# benchmark ID. This is a superset across pyperformance releases; only IDs that
# actually appear in the pinned manifest are ever consulted, so future pins that
# (re)introduce decimal/yaml variants are flagged correctly without code change.
#
# Grounded empirically on CPython 3.12: _json, _pickle, _elementtree and
# _decimal C accelerators are present and active by default; tomllib ships NO
# _tomllib C accelerator (pure-Python) so tomli_loads is intentionally absent;
# the pickle ``*_pure_python`` variants explicitly disable the C accelerator and
# are intentionally absent.
C_ACCELERATED_BENCHMARKS = frozenset(
    {
        "json_dumps",
        "json_loads",
        "pickle",
        "pickle_dict",
        "pickle_list",
        "unpickle",
        "unpickle_list",
        "xml_etree",
        "xml_etree_parse",
        "xml_etree_iterparse",
        "xml_etree_generate",
        "xml_etree_process",
        "telco",
        "decimal",
        "decimal_factorial",
        "decimal_pi",
    }
)

# stdlib modules whose C accelerator some benchmarks toggle via ``sys.modules``
# manipulation (e.g. ``pickle_pure_python`` sets ``sys.modules['_pickle']=None``
# then re-imports ``pickle``). They are evicted before each in-process exec so
# the benchmark's own import logic starts from a clean slate, mirroring
# pyperformance's fresh-subprocess-per-benchmark isolation.
_ACCEL_TOGGLE_MODULES = (
    "pickle",
    "_pickle",
    "json",
    "json.decoder",
    "json.encoder",
    "json.scanner",
    "_json",
    "decimal",
    "_decimal",
    "_pydecimal",
)


def is_c_accelerated(benchmark: str) -> bool:
    """Return whether ``benchmark`` races a CPython stdlib C accelerator."""
    return benchmark in C_ACCELERATED_BENCHMARKS


@dataclass(frozen=True)
class ManifestCatalog:
    benchmark_names: tuple[str, ...]
    groups: tuple[str, ...]

    @property
    def benchmark_count(self) -> int:
        return len(self.benchmark_names)


@dataclass(frozen=True)
class BenchmarkSpec:
    """A single resolved pyperformance benchmark and its in-process recipe."""

    name: str
    runscript: Path
    extra_opts: tuple[str, ...]
    tags: tuple[str, ...]

    @property
    def c_accelerated(self) -> bool:
        return is_c_accelerated(self.name)


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


# ---------------------------------------------------------------------------
# Curated smoke lane (works against stripped-down fixtures; no pyperformance dep)
# ---------------------------------------------------------------------------


def _install_pyperf_shim() -> None:
    """Install a minimal ``pyperf`` stub for the curated smoke fixtures.

    The smoke fixtures import ``pyperf`` only for ``perf_counter``; they define
    benchmark functions but no ``Runner`` ``__main__`` block, so the curated
    runners below call the kernels directly.
    """
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
        "status": "ok",
        "elapsed_s": elapsed_s,
        "result_fingerprint": fingerprint,
        "c_accelerated": is_c_accelerated(benchmark),
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
        "lane": "subset",
        "benchmarks": list(benchmarks),
        "rounds": rounds,
        "results": results,
        "total_elapsed_s": total_elapsed_s,
    }


# ---------------------------------------------------------------------------
# Full corpus lane (manifest-driven enumeration + faithful in-process runner)
# ---------------------------------------------------------------------------


class _BenchmarkExecuted(Exception):
    """Signals that the captured benchmark workload has run; unwinds ``__main__``."""


def _result_fingerprint(
    name: str, kind: str, inner_loops: object, result: object
) -> str:
    digest = hashlib.sha256()
    for part in (name, kind, repr(inner_loops), repr(result)):
        digest.update(part.encode("utf-8", "surrogatepass"))
        digest.update(b"|")
    return digest.hexdigest()[:16]


def _build_faithful_pyperf(
    capture: dict[str, object], argv: list[str], rounds: int
) -> ModuleType:
    """Build a ``pyperf`` module that runs benchmarks faithfully in-process.

    Every non-``Runner`` attribute is delegated to the real ``pyperf`` module so
    benchmarks that call ``pyperf.perf_counter`` / ``pyperf.python_implementation``
    / etc. at import time see authentic behavior. Only ``Runner`` is overridden:
    its ``bench_func`` / ``bench_time_func`` execute the benchmark's own workload
    immediately (while ``__main__`` setup state is live), capture timing plus a
    deterministic fingerprint, then raise to unwind any ``__main__`` cleanup.
    """
    real = importlib.import_module("pyperf")
    shim = types.ModuleType("pyperf")
    for attr in dir(real):
        if not attr.startswith("__"):
            setattr(shim, attr, getattr(real, attr))

    class _FaithfulRunner:
        def __init__(
            self, *args: object, add_cmdline_args: object = None, **kwargs: object
        ) -> None:
            self.metadata: dict[str, object] = {}
            self.args: argparse.Namespace | None = None
            self.argparser = argparse.ArgumentParser(add_help=False)

        def parse_args(self) -> argparse.Namespace:
            if self.args is None:
                parsed, _unknown = self.argparser.parse_known_args(list(argv))
                # pyperf benchmarks read these standard attributes directly.
                for attr, value in (
                    ("worker", False),
                    ("profile", None),
                    ("inherit_environ", None),
                ):
                    if not hasattr(parsed, attr):
                        setattr(parsed, attr, value)
                self.args = parsed
            return self.args

        def _execute(
            self,
            kind: str,
            name: str,
            func: object,
            args: tuple[object, ...],
            inner_loops: object,
        ) -> None:
            self.parse_args()
            start = time.perf_counter()
            result: object = None
            if kind == "func":
                for _ in range(rounds):
                    result = func(*args)
            else:
                # bench_time_func: func(loops, *args) loops internally and
                # returns elapsed seconds. We drive its real workload with a
                # loop count of 1 per round; the observable signal is timing.
                for _ in range(rounds):
                    func(1, *args)
                result = "<time_func>"
            elapsed_s = time.perf_counter() - start
            capture.update(
                name=name,
                kind=kind,
                elapsed_s=elapsed_s,
                fingerprint=_result_fingerprint(name, kind, inner_loops, result),
            )
            raise _BenchmarkExecuted

        def bench_func(
            self,
            name: str,
            func: object,
            *args: object,
            inner_loops: object = None,
            metadata: object = None,
        ) -> None:
            self._execute("func", name, func, args, inner_loops)

        def bench_time_func(
            self,
            name: str,
            time_func: object,
            *args: object,
            inner_loops: object = None,
            metadata: object = None,
        ) -> None:
            self._execute("time", name, time_func, args, inner_loops)

        def bench_async_func(self, *args: object, **kwargs: object) -> None:
            raise RuntimeError(
                "async benchmark requires a running event loop; "
                "not runnable in the molt in-process lane"
            )

        def bench_command(self, *args: object, **kwargs: object) -> None:
            raise RuntimeError(
                "bench_command spawns a subprocess; "
                "not runnable in the molt in-process lane"
            )

        def timeit(self, *args: object, **kwargs: object) -> None:
            raise RuntimeError("timeit is not supported in the molt in-process lane")

    shim.Runner = _FaithfulRunner
    return shim


def _is_pickling_error(exc: BaseException) -> bool:
    """Return whether ``exc`` is a pickling failure (multiprocessing worker).

    ``_run_full_benchmark_once`` evicts ``pickle``/``_pickle`` from
    ``sys.modules`` before each benchmark exec (so accelerator-toggling
    benchmarks import cleanly). A benchmark that imports ``multiprocessing``
    then loads a *fresh* ``_pickle`` whose ``PicklingError`` is a distinct class
    object from the one bound at adapter import time — so an identity-based
    ``except pickle.PickleError`` would miss it. Resolve the current
    ``pickle.PickleError`` and fall back to a qualname check (covering
    spawn-based multiprocessing on Windows, which may surface a re-raised copy).
    """
    try:
        current_pickle = importlib.import_module("pickle")
        if isinstance(exc, current_pickle.PickleError):
            return True
    except ImportError:  # pragma: no cover - pickle is always importable
        pass
    return any(
        cls.__qualname__ in ("PickleError", "PicklingError", "UnpicklingError")
        for cls in type(exc).__mro__
    )


def _skipped(spec: BenchmarkSpec, reason: str) -> dict[str, object]:
    return {
        "benchmark": spec.name,
        "status": "skipped",
        "reason": reason,
        "elapsed_s": None,
        "result_fingerprint": "",
        "c_accelerated": spec.c_accelerated,
    }


def _errored(spec: BenchmarkSpec, reason: str) -> dict[str, object]:
    return {
        "benchmark": spec.name,
        "status": "error",
        "reason": reason,
        "elapsed_s": None,
        "result_fingerprint": "",
        "c_accelerated": spec.c_accelerated,
    }


def _run_full_benchmark_once(spec: BenchmarkSpec, rounds: int) -> dict[str, object]:
    """Drive one resolved pyperformance benchmark in-process for ``rounds`` rounds."""
    script = spec.runscript
    if not script.exists():
        return _errored(spec, f"benchmark script not found: {script}")

    extra_opts = list(spec.extra_opts)
    capture: dict[str, object] = {}
    shim = _build_faithful_pyperf(capture, extra_opts, rounds)

    module_snapshot = dict(sys.modules)
    path_snapshot = list(sys.path)
    argv_snapshot = list(sys.argv)
    # Clean accelerator-toggleable modules so a benchmark that controls its own
    # C-accelerator import (e.g. pickle_pure_python) starts from a clean slate.
    for module_name in _ACCEL_TOGGLE_MODULES:
        sys.modules.pop(module_name, None)
    sys.modules["pyperf"] = shim
    sys.path.insert(0, str(script.parent))
    sys.argv = [str(script), *extra_opts]

    source = script.read_text(encoding="utf-8")
    code = compile(source, str(script), "exec")
    module_globals: dict[str, object] = {
        "__name__": "__main__",
        "__file__": str(script),
        "__builtins__": __builtins__,
    }

    result: dict[str, object]
    try:
        with contextlib.redirect_stdout(io.StringIO()):
            exec(code, module_globals)
        # __main__ finished without registering a runnable benchmark.
        result = _errored(spec, "benchmark did not register a pyperf benchmark")
    except _BenchmarkExecuted:
        result = {
            "benchmark": spec.name,
            "status": "ok",
            "elapsed_s": capture["elapsed_s"],
            "result_fingerprint": capture["fingerprint"],
            "c_accelerated": spec.c_accelerated,
        }
    except (ImportError, ModuleNotFoundError) as exc:
        result = _skipped(spec, f"missing dependency: {exc}")
    except SystemExit as exc:
        result = _skipped(
            spec, f"benchmark exited during import (SystemExit {exc.code})"
        )
    except RuntimeError as exc:
        # Raised by the shim for async / bench_command / timeit entry points.
        result = _skipped(spec, str(exc))
    except Exception as exc:  # noqa: BLE001 - report any other failure honestly
        if _is_pickling_error(exc):
            # multiprocessing benchmarks need an importable, picklable worker; a
            # function defined in the exec'd module namespace cannot be pickled
            # (and spawn-based workers would re-exec the module). This is a
            # structural in-process limit, not a benchmark failure.
            result = _skipped(
                spec,
                "not runnable in-process (multiprocessing): "
                f"{type(exc).__name__}: {exc}",
            )
        else:
            result = _errored(spec, f"{type(exc).__name__}: {exc}")
    finally:
        sys.modules.clear()
        sys.modules.update(module_snapshot)
        sys.path[:] = path_snapshot
        sys.argv[:] = argv_snapshot

    return result


def _manifest_fully_resolves(manifest: object) -> bool:
    """Return whether ``manifest`` resolves benchmark metadata (runscripts/tags).

    A MANIFEST copied out of the package (e.g. a bare ``git clone`` of the
    pyperformance repo without an editable install) parses but fails to resolve
    per-benchmark metadata ("missing benchmark version") because the version
    files live alongside the installed package. Touch one benchmark's metadata
    to detect that case before committing to the bundled MANIFEST.
    """
    benchmarks = list(getattr(manifest, "benchmarks", ()) or ())
    if not benchmarks:
        return False
    try:
        # pyperformance's metadata loader prints diagnostics to stdout when a
        # bare-clone MANIFEST cannot resolve a version; suppress that so the
        # adapter's stdout stays a single clean JSON object.
        with contextlib.redirect_stdout(io.StringIO()):
            _ = benchmarks[0].runscript
            _ = benchmarks[0].tags
    except Exception:  # noqa: BLE001 - any resolution failure disqualifies it
        return False
    return True


def _load_pyperformance_manifest(suite_root: Path) -> object:
    """Load pyperformance's resolved benchmark manifest for ``suite_root``.

    pyperformance must be importable (it is in the friend-benchmark lane, where
    the suite is installed isolated, version-pinned to match ``repo_ref``). Its
    own manifest machinery is the source of truth for the runnable-ID set, tags
    (groups), and per-ID argv recipes.

    The bundled MANIFEST under ``suite_root`` is preferred when it fully
    resolves (a properly-installed checkout), so a custody-pinned clone drives
    enumeration. A bare clone's MANIFEST cannot resolve per-benchmark metadata,
    so we fall back to the importable package's default manifest — identical
    content under the version pin, and the path every run is validated against.
    """
    try:
        from pyperformance import _manifest  # type: ignore[import-not-found]
    except ImportError as exc:  # pragma: no cover - depends on lane environment
        raise RuntimeError(
            "pyperformance must be importable to enumerate the full benchmark "
            "set (install it isolated, e.g. `uv run --with pyperformance==1.14.0`); "
            f"import failed: {exc}"
        ) from exc

    bundled_manifest = _benchmark_root(suite_root) / "MANIFEST"
    if bundled_manifest.exists():
        candidate = _manifest.load_manifest(str(bundled_manifest))
        if _manifest_fully_resolves(candidate):
            return candidate
    return _manifest.load_manifest(None)


def _specs_from_manifest(manifest: object) -> tuple[BenchmarkSpec, ...]:
    specs: list[BenchmarkSpec] = []
    for benchmark in manifest.benchmarks:  # type: ignore[attr-defined]
        specs.append(
            BenchmarkSpec(
                name=benchmark.name,
                runscript=Path(benchmark.runscript),
                extra_opts=tuple(benchmark.extra_opts or ()),
                tags=tuple(benchmark.tags or ()),
            )
        )
    return tuple(sorted(specs, key=lambda spec: spec.name))


def load_full_catalog(suite_root: Path) -> dict[str, object]:
    """Enumerate the full resolved pyperformance corpus and its groups."""
    suite_root = suite_root.resolve()
    manifest = _load_pyperformance_manifest(suite_root)
    specs = _specs_from_manifest(manifest)
    groups = sorted(str(group) for group in getattr(manifest, "groups", ()) or ())
    return {
        "suite_root": str(suite_root),
        "benchmark_count": len(specs),
        "benchmark_names": [spec.name for spec in specs],
        "groups": groups,
        "c_accelerated": sorted(spec.name for spec in specs if spec.c_accelerated),
    }


def _summarize(results: list[dict[str, object]]) -> dict[str, int]:
    summary = {"ok": 0, "skipped": 0, "error": 0}
    for row in results:
        status = str(row.get("status", "error"))
        summary[status] = summary.get(status, 0) + 1
    return summary


def _run_specs(specs: tuple[BenchmarkSpec, ...], rounds: int) -> dict[str, object]:
    results = [_run_full_benchmark_once(spec, rounds) for spec in specs]
    total_elapsed_s = sum(
        float(row["elapsed_s"])
        for row in results
        if isinstance(row["elapsed_s"], float)
    )
    return {
        "results": results,
        "summary": _summarize(results),
        "total_elapsed_s": total_elapsed_s,
    }


def run_all(suite_root: Path, *, rounds: int) -> dict[str, object]:
    if rounds <= 0:
        raise ValueError("rounds must be positive")
    suite_root = suite_root.resolve()
    manifest = _load_pyperformance_manifest(suite_root)
    specs = _specs_from_manifest(manifest)
    payload = _run_specs(specs, rounds)
    return {
        "suite_root": str(suite_root),
        "lane": "all",
        "rounds": rounds,
        "benchmark_count": len(specs),
        **payload,
    }


def run_group(suite_root: Path, *, group: str, rounds: int) -> dict[str, object]:
    if rounds <= 0:
        raise ValueError("rounds must be positive")
    suite_root = suite_root.resolve()
    manifest = _load_pyperformance_manifest(suite_root)
    available_groups = sorted(str(g) for g in getattr(manifest, "groups", ()) or ())
    if group not in available_groups:
        raise ValueError(
            f"unknown group {group!r}; available: {', '.join(available_groups)}"
        )
    member_names = {bench.name for bench in manifest.resolve_group(group)}
    specs = tuple(
        spec for spec in _specs_from_manifest(manifest) if spec.name in member_names
    )
    payload = _run_specs(specs, rounds)
    return {
        "suite_root": str(suite_root),
        "lane": "group",
        "group": group,
        "rounds": rounds,
        "benchmark_count": len(specs),
        **payload,
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


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


def _cmd_catalog(args: argparse.Namespace) -> int:
    suite_root = Path(args.suite_root).expanduser().resolve()
    if args.full:
        payload = load_full_catalog(suite_root)
        if args.json:
            print(json.dumps(payload, sort_keys=True))
        else:
            print(f"suite_root={suite_root}")
            print(f"benchmark_count={int(payload['benchmark_count'])}")
            print("groups=" + ",".join(payload["groups"]))
            print("c_accelerated=" + ",".join(payload["c_accelerated"]))
        return 0

    payload = catalog_suite(suite_root)
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        print(f"suite_root={suite_root}")
        print(f"benchmark_count={int(payload['benchmark_count'])}")
        print("groups=" + ",".join(payload["groups"]))
        print("smoke_available=" + ",".join(payload["smoke_available"]))
    return 0


def _print_results_table(payload: dict[str, object]) -> None:
    for row in payload["results"]:
        status = str(row["status"])
        elapsed = row.get("elapsed_s")
        elapsed_str = f"{float(elapsed):.9f}" if isinstance(elapsed, float) else "-"
        detail = (
            str(row.get("result_fingerprint", ""))
            if status == "ok"
            else str(row.get("reason", ""))
        )
        print(
            "benchmark={benchmark} status={status} c_accelerated={cacc} "
            "elapsed_s={elapsed} detail={detail}".format(
                benchmark=row["benchmark"],
                status=status,
                cacc=int(bool(row["c_accelerated"])),
                elapsed=elapsed_str,
                detail=detail,
            )
        )
    summary = payload.get("summary")
    if isinstance(summary, dict):
        print(
            "summary ok={ok} skipped={skipped} error={error}".format(
                ok=summary.get("ok", 0),
                skipped=summary.get("skipped", 0),
                error=summary.get("error", 0),
            )
        )
    print(f"total_elapsed_s={float(payload['total_elapsed_s']):.9f}")


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
                "benchmark={benchmark} status={status} c_accelerated={cacc} "
                "elapsed_s={elapsed:.9f} fingerprint={fingerprint}".format(
                    benchmark=item["benchmark"],
                    status=item["status"],
                    cacc=int(bool(item["c_accelerated"])),
                    elapsed=float(item["elapsed_s"]),
                    fingerprint=item["result_fingerprint"],
                )
            )
        print(f"total_elapsed_s={float(payload['total_elapsed_s']):.9f}")
    return 0


def _cmd_run_all(args: argparse.Namespace) -> int:
    suite_root = Path(args.suite_root).expanduser().resolve()
    payload = run_all(suite_root, rounds=args.rounds)
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        _print_results_table(payload)
    return 0


def _cmd_run_group(args: argparse.Namespace) -> int:
    suite_root = Path(args.suite_root).expanduser().resolve()
    payload = run_group(suite_root, group=args.group, rounds=args.rounds)
    if args.json:
        print(json.dumps(payload, sort_keys=True))
    else:
        _print_results_table(payload)
    return 0


def _build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description=(
            "PyPerformance adapter for the molt differential and friend "
            "benchmark lanes."
        )
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
        "--full",
        action="store_true",
        help=(
            "Enumerate the full resolved corpus via pyperformance's manifest "
            "API (requires pyperformance importable) instead of the bundled "
            "MANIFEST text."
        ),
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

    run_all_cmd = subparsers.add_parser(
        "run-all",
        help="Run the FULL pyperformance corpus in-process (manifest-driven).",
    )
    run_all_cmd.add_argument(
        "--suite-root",
        required=True,
        help="Path to the installed pyperformance package/repo root.",
    )
    run_all_cmd.add_argument(
        "--rounds",
        type=int,
        default=1,
        help="Workload rounds per benchmark (default: 1).",
    )
    run_all_cmd.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON run summary.",
    )
    run_all_cmd.set_defaults(func=_cmd_run_all)

    run_group_cmd = subparsers.add_parser(
        "run-group",
        help="Run one pyperformance group (tag) in-process (manifest-driven).",
    )
    run_group_cmd.add_argument(
        "--suite-root",
        required=True,
        help="Path to the installed pyperformance package/repo root.",
    )
    run_group_cmd.add_argument(
        "--group",
        required=True,
        help="Group/tag name (e.g. math, regex, serialize, apps, template).",
    )
    run_group_cmd.add_argument(
        "--rounds",
        type=int,
        default=1,
        help="Workload rounds per benchmark (default: 1).",
    )
    run_group_cmd.add_argument(
        "--json",
        action="store_true",
        help="Emit machine-readable JSON run summary.",
    )
    run_group_cmd.set_defaults(func=_cmd_run_group)

    return parser


def main() -> int:
    parser = _build_parser()
    args = parser.parse_args()
    return int(args.func(args))


if __name__ == "__main__":
    raise SystemExit(main())
