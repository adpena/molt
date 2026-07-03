#!/usr/bin/env python3
"""Profile frontend lowering hotspots over a deterministic source corpus."""

from __future__ import annotations

import argparse
import ast
import datetime as dt
import glob
import hashlib
import json
import sys
import time
import tokenize
from collections import Counter
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
SRC_ROOT = ROOT / "src"
TOOLS_ROOT = ROOT / "tools"
DEFAULT_MANIFEST = ROOT / "tests" / "differential" / "basic" / "CORE_TESTS.txt"


def _path_entry_resolves_to(path_entry: str, target: Path) -> bool:
    try:
        return Path(path_entry).resolve() == target.resolve()
    except OSError:
        return False


def _import_stdlib_profilers() -> tuple[Any, Any]:
    """Import cProfile without letting tools/profile.py shadow stdlib profile."""
    removed_entries = [
        entry for entry in sys.path if _path_entry_resolves_to(entry, TOOLS_ROOT)
    ]
    for entry in removed_entries:
        sys.path.remove(entry)
    previous_profile = sys.modules.get("profile")
    if previous_profile is not None:
        profile_file = getattr(previous_profile, "__file__", "")
        if profile_file and _path_entry_resolves_to(
            str(Path(profile_file).parent), TOOLS_ROOT
        ):
            sys.modules.pop("profile", None)
    try:
        import cProfile as cprofile_module
        import pstats as pstats_module
    finally:
        for entry in reversed(removed_entries):
            if entry not in sys.path:
                sys.path.insert(0, entry)
        if previous_profile is not None and "profile" not in sys.modules:
            sys.modules["profile"] = previous_profile
    return cprofile_module, pstats_module


cProfile, pstats = _import_stdlib_profilers()

if str(SRC_ROOT) not in sys.path:
    sys.path.insert(0, str(SRC_ROOT))

from molt.frontend import SimpleTIRGenerator, _ic_counter  # noqa: E402


def _utc_stamp() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y%m%dT%H%M%SZ")


def _repo_rel(path: Path) -> str:
    resolved = path.resolve()
    try:
        return resolved.relative_to(ROOT).as_posix()
    except ValueError:
        return resolved.as_posix()


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _read_python_source(path: Path) -> str:
    with tokenize.open(path) as handle:
        return handle.read()


def _git_rev() -> str | None:
    import subprocess

    try:
        result = subprocess.run(
            ["git", "rev-parse", "HEAD"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=False,
        )
    except OSError:
        return None
    if result.returncode != 0:
        return None
    return result.stdout.strip() or None


def _manifest_paths(path: Path) -> list[Path]:
    if not path.is_file():
        raise FileNotFoundError(f"frontend profile manifest not found: {path}")
    out: list[Path] = []
    for lineno, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        candidate = (ROOT / stripped).resolve()
        if not candidate.is_file():
            raise FileNotFoundError(
                f"{path}:{lineno}: manifest source not found: {stripped}"
            )
        out.append(candidate)
    return out


def _has_glob_magic(text: str) -> bool:
    return any(ch in text for ch in "*?[")


def _resolve_source_arg(raw: str) -> list[Path]:
    if _has_glob_magic(raw):
        pattern = raw if Path(raw).is_absolute() else str(ROOT / raw)
        matches = sorted(Path(match) for match in glob.glob(pattern))
        files = [path.resolve() for path in matches if path.is_file()]
        if not files:
            raise FileNotFoundError(f"frontend profile glob matched no files: {raw}")
        return files
    path = Path(raw)
    if not path.is_absolute():
        path = ROOT / path
    path = path.resolve()
    if path.is_file():
        return [path]
    if path.is_dir():
        files = sorted(
            child.resolve() for child in path.glob("*.py") if child.is_file()
        )
        if not files:
            raise FileNotFoundError(
                f"frontend profile directory has no .py files: {raw}"
            )
        return files
    raise FileNotFoundError(f"frontend profile source not found: {raw}")


def resolve_sources(
    *,
    manifest: Path | None,
    sources: Sequence[str],
    limit: int | None,
) -> list[Path]:
    selected: list[Path] = []
    if manifest is not None:
        selected.extend(_manifest_paths(manifest.resolve()))
    if sources:
        for raw in sources:
            selected.extend(_resolve_source_arg(raw))
    if not selected:
        selected.extend(_manifest_paths(DEFAULT_MANIFEST))

    deduped: list[Path] = []
    seen: set[str] = set()
    for path in selected:
        key = str(path.resolve())
        if key in seen:
            continue
        seen.add(key)
        deduped.append(path)
    if limit is not None:
        deduped = deduped[: max(0, limit)]
    if not deduped:
        raise ValueError("frontend profile corpus is empty")
    return deduped


def _module_name_for_path(path: Path) -> str:
    rel = _repo_rel(path)
    if rel.endswith(".py"):
        rel = rel[:-3]
    return rel.replace("/", ".").replace("\\", ".")


def _frontend_cprofile_rows(
    profiler: cProfile.Profile, *, limit: int
) -> list[dict[str, Any]]:
    stats = pstats.Stats(profiler)
    rows: list[dict[str, Any]] = []
    for (filename, line, function), values in stats.stats.items():
        _primitive_calls, total_calls, self_s, cumulative_s, _callers = values
        if not filename or filename.startswith("<"):
            continue
        path = Path(filename)
        try:
            resolved = path.resolve()
        except OSError:
            continue
        try:
            rel = resolved.relative_to(SRC_ROOT).as_posix()
        except ValueError:
            continue
        if not rel.startswith("molt/frontend/"):
            continue
        rows.append(
            {
                "file": rel,
                "line": int(line),
                "function": function,
                "calls": int(total_calls),
                "self_ms": round(self_s * 1000.0, 6),
                "cumulative_ms": round(cumulative_s * 1000.0, 6),
            }
        )
    rows.sort(
        key=lambda item: (
            float(item["cumulative_ms"]),
            float(item["self_ms"]),
            str(item["file"]),
            int(item["line"]),
        ),
        reverse=True,
    )
    return rows[:limit]


def _aggregate_cprofile(
    aggregate: dict[tuple[str, int, str], dict[str, Any]],
    rows: Iterable[Mapping[str, Any]],
) -> None:
    for row in rows:
        key = (str(row["file"]), int(row["line"]), str(row["function"]))
        bucket = aggregate.setdefault(
            key,
            {
                "file": key[0],
                "line": key[1],
                "function": key[2],
                "calls": 0,
                "self_ms": 0.0,
                "cumulative_ms": 0.0,
                "sources": 0,
            },
        )
        bucket["calls"] = int(bucket["calls"]) + int(row.get("calls", 0))
        bucket["self_ms"] = float(bucket["self_ms"]) + float(row.get("self_ms", 0.0))
        bucket["cumulative_ms"] = float(bucket["cumulative_ms"]) + float(
            row.get("cumulative_ms", 0.0)
        )
        bucket["sources"] = int(bucket["sources"]) + 1


def _p95(values: Sequence[float]) -> float:
    if not values:
        return 0.0
    ordered = sorted(values)
    idx = max(0, min(len(ordered) - 1, int((len(ordered) - 1) * 0.95)))
    return float(ordered[idx])


def _record_pass_aggregates(
    aggregate: dict[str, dict[str, Any]],
    *,
    source: str,
    pass_stats_by_function: Mapping[str, Mapping[str, Mapping[str, Any]]],
) -> None:
    for function_name, pass_stats in pass_stats_by_function.items():
        for pass_name, stats in pass_stats.items():
            bucket = aggregate.setdefault(
                pass_name,
                {
                    "pass": pass_name,
                    "total_ms": 0.0,
                    "max_ms": 0.0,
                    "attempted": 0,
                    "accepted": 0,
                    "degraded": 0,
                    "samples_ms": [],
                    "functions": set(),
                    "sources": set(),
                },
            )
            total_ms = float(stats.get("ms_total", 0.0) or 0.0)
            bucket["total_ms"] = float(bucket["total_ms"]) + total_ms
            bucket["max_ms"] = max(
                float(bucket["max_ms"]), float(stats.get("ms_max", 0.0) or 0.0)
            )
            bucket["attempted"] = int(bucket["attempted"]) + int(
                stats.get("attempted", 0) or 0
            )
            bucket["accepted"] = int(bucket["accepted"]) + int(
                stats.get("accepted", 0) or 0
            )
            bucket["degraded"] = int(bucket["degraded"]) + int(
                stats.get("degraded", 0) or 0
            )
            samples = bucket["samples_ms"]
            if isinstance(samples, list):
                samples.extend(
                    float(sample)
                    for sample in stats.get("samples_ms", [])
                    if isinstance(sample, int | float)
                )
            bucket["functions"].add(str(function_name))
            bucket["sources"].add(source)


def _rank_passes(
    aggregate: Mapping[str, Mapping[str, Any]], *, limit: int
) -> list[dict[str, Any]]:
    rows: list[dict[str, Any]] = []
    for pass_name, bucket in aggregate.items():
        samples = [
            float(sample)
            for sample in bucket.get("samples_ms", [])
            if isinstance(sample, int | float)
        ]
        functions = bucket.get("functions", set())
        sources = bucket.get("sources", set())
        rows.append(
            {
                "pass": pass_name,
                "total_ms": round(float(bucket.get("total_ms", 0.0)), 6),
                "p95_ms": round(_p95(samples), 6),
                "max_ms": round(float(bucket.get("max_ms", 0.0)), 6),
                "samples": len(samples),
                "attempted": int(bucket.get("attempted", 0)),
                "accepted": int(bucket.get("accepted", 0)),
                "degraded": int(bucket.get("degraded", 0)),
                "function_count": len(functions) if isinstance(functions, set) else 0,
                "source_count": len(sources) if isinstance(sources, set) else 0,
            }
        )
    rows.sort(
        key=lambda item: (
            float(item["total_ms"]),
            float(item["p95_ms"]),
            str(item["pass"]),
        ),
        reverse=True,
    )
    return rows[:limit]


def _rank_cprofile(
    aggregate: Mapping[tuple[str, int, str], Mapping[str, Any]],
    *,
    limit: int,
) -> list[dict[str, Any]]:
    rows = [
        {
            "file": str(bucket["file"]),
            "line": int(bucket["line"]),
            "function": str(bucket["function"]),
            "calls": int(bucket["calls"]),
            "self_ms": round(float(bucket["self_ms"]), 6),
            "cumulative_ms": round(float(bucket["cumulative_ms"]), 6),
            "source_hits": int(bucket["sources"]),
        }
        for bucket in aggregate.values()
    ]
    rows.sort(
        key=lambda item: (
            float(item["cumulative_ms"]),
            float(item["self_ms"]),
            str(item["file"]),
            int(item["line"]),
        ),
        reverse=True,
    )
    return rows[:limit]


def profile_one(
    path: Path,
    *,
    optimization_profile: str,
    top_functions: int,
) -> tuple[dict[str, Any], list[dict[str, Any]]]:
    rel = _repo_rel(path)
    source = _read_python_source(path)
    source_hash = _sha256_text(source)
    profiler = cProfile.Profile()
    start = time.perf_counter()
    parse_ms = 0.0
    visit_ms = 0.0
    serialize_ms = 0.0
    try:
        _ic_counter[0] = 0
        profiler.enable()
        parse_start = time.perf_counter()
        tree = ast.parse(source, filename=str(path))
        parse_ms = (time.perf_counter() - parse_start) * 1000.0
        gen = SimpleTIRGenerator(
            optimization_profile=optimization_profile,
            module_name=_module_name_for_path(path),
            source_path=str(path),
        )
        visit_start = time.perf_counter()
        gen.visit(tree)
        visit_ms = (time.perf_counter() - visit_start) * 1000.0
        serialize_start = time.perf_counter()
        ir = gen.to_json()
        serialize_ms = (time.perf_counter() - serialize_start) * 1000.0
        profiler.disable()
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        function_count = len(ir.get("functions", [])) if isinstance(ir, dict) else 0
        op_count = 0
        if isinstance(ir, dict):
            for function in ir.get("functions", []):
                if isinstance(function, dict):
                    ops = function.get("ops", [])
                    if isinstance(ops, list):
                        op_count += len(ops)
        result = {
            "path": rel,
            "sha256": source_hash,
            "status": "pass",
            "elapsed_ms": round(elapsed_ms, 6),
            "parse_ms": round(parse_ms, 6),
            "visit_ms": round(visit_ms, 6),
            "serialize_ms": round(serialize_ms, 6),
            "function_count": function_count,
            "op_count": op_count,
            "midend_pass_stats_by_function": gen.midend_pass_stats_by_function,
            "midend_policy_outcomes_by_function": gen.midend_policy_outcomes_by_function,
        }
    except Exception as exc:  # noqa: BLE001 - this is a profiling census.
        profiler.disable()
        elapsed_ms = (time.perf_counter() - start) * 1000.0
        result = {
            "path": rel,
            "sha256": source_hash,
            "status": "error",
            "elapsed_ms": round(elapsed_ms, 6),
            "parse_ms": round(parse_ms, 6),
            "visit_ms": round(visit_ms, 6),
            "serialize_ms": round(serialize_ms, 6),
            "error_type": type(exc).__name__,
            "error": str(exc),
            "midend_pass_stats_by_function": {},
            "midend_policy_outcomes_by_function": {},
        }
    return result, _frontend_cprofile_rows(profiler, limit=top_functions)


def profile_sources(
    sources: Sequence[Path],
    *,
    optimization_profile: str,
    top: int,
) -> dict[str, Any]:
    pass_aggregate: dict[str, dict[str, Any]] = {}
    cprofile_aggregate: dict[tuple[str, int, str], dict[str, Any]] = {}
    source_results: list[dict[str, Any]] = []
    for path in sources:
        result, cprofile_rows = profile_one(
            path,
            optimization_profile=optimization_profile,
            top_functions=max(1, top),
        )
        source_results.append(result)
        _record_pass_aggregates(
            pass_aggregate,
            source=str(result["path"]),
            pass_stats_by_function=result["midend_pass_stats_by_function"],
        )
        _aggregate_cprofile(cprofile_aggregate, cprofile_rows)
    status_counts = Counter(
        str(result.get("status", "unknown")) for result in source_results
    )
    return {
        "schema_version": "1.0",
        "tool": "frontend_hot_pass_profile",
        "generated_at_utc": _utc_stamp(),
        "git_rev": _git_rev(),
        "optimization_profile": optimization_profile,
        "source_count": len(source_results),
        "status_counts": dict(sorted(status_counts.items())),
        "total_elapsed_ms": round(
            sum(float(result.get("elapsed_ms", 0.0)) for result in source_results), 6
        ),
        "sources": source_results,
        "ranked_midend_passes": _rank_passes(pass_aggregate, limit=max(1, top)),
        "ranked_frontend_functions": _rank_cprofile(
            cprofile_aggregate,
            limit=max(1, top),
        ),
    }


def _markdown_table(
    rows: Sequence[Mapping[str, Any]], columns: Sequence[str]
) -> list[str]:
    out = ["| " + " | ".join(columns) + " |"]
    out.append("| " + " | ".join("---" for _ in columns) + " |")
    for index, row in enumerate(rows, start=1):
        values: list[str] = []
        for column in columns:
            if column == "rank":
                values.append(str(index))
            else:
                values.append(str(row.get(column, "")))
        out.append("| " + " | ".join(values) + " |")
    return out


def format_markdown(report: Mapping[str, Any]) -> str:
    lines = [
        "# Frontend Hot-Pass Profile",
        "",
        f"- Generated: {report.get('generated_at_utc')}",
        f"- Git revision: {report.get('git_rev')}",
        f"- Optimization profile: {report.get('optimization_profile')}",
        f"- Sources: {report.get('source_count')} ({report.get('status_counts')})",
        f"- Total frontend elapsed: {report.get('total_elapsed_ms')} ms",
        "",
        "## Ranked Midend Passes",
        "",
    ]
    lines.extend(
        _markdown_table(
            report.get("ranked_midend_passes", []),
            (
                "rank",
                "pass",
                "total_ms",
                "p95_ms",
                "attempted",
                "accepted",
                "degraded",
                "source_count",
            ),
        )
    )
    lines.extend(["", "## Ranked Frontend Functions", ""])
    lines.extend(
        _markdown_table(
            report.get("ranked_frontend_functions", []),
            (
                "rank",
                "file",
                "line",
                "function",
                "cumulative_ms",
                "self_ms",
                "calls",
            ),
        )
    )
    errors = [
        source for source in report.get("sources", []) if source.get("status") != "pass"
    ]
    if errors:
        lines.extend(["", "## Errors", ""])
        for source in errors:
            lines.append(
                f"- {source.get('path')}: {source.get('error_type')}: {source.get('error')}"
            )
    lines.append("")
    return "\n".join(lines)


def _parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Profile Molt frontend hot passes over a deterministic corpus."
    )
    parser.add_argument(
        "--manifest",
        type=Path,
        default=None,
        help=(
            "Repo-relative source manifest. Defaults to "
            "tests/differential/basic/CORE_TESTS.txt when no sources are given."
        ),
    )
    parser.add_argument(
        "--source",
        action="append",
        default=[],
        help="Source file, directory of .py files, or repo-relative glob. May repeat.",
    )
    parser.add_argument("--limit", type=int, default=None)
    parser.add_argument("--top", type=int, default=12)
    parser.add_argument(
        "--optimization-profile",
        choices=("dev", "release"),
        default="release",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=None,
        help="Output directory. Defaults to logs/frontend_profile/<timestamp>.",
    )
    parser.add_argument("--fail-on-error", action="store_true")
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = _parse_args(argv)
    sources = resolve_sources(
        manifest=args.manifest,
        sources=args.source,
        limit=args.limit,
    )
    report = profile_sources(
        sources,
        optimization_profile=args.optimization_profile,
        top=args.top,
    )
    stamp = report["generated_at_utc"]
    out_dir = args.out_dir or (ROOT / "logs" / "frontend_profile" / f"profile_{stamp}")
    out_dir.mkdir(parents=True, exist_ok=True)
    json_path = out_dir / "frontend_hot_pass_profile.json"
    md_path = out_dir / "frontend_hot_pass_profile.md"
    json_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    md_path.write_text(format_markdown(report), encoding="utf-8")
    print(
        "frontend-hot-pass-profile "
        f"rc=0 sources={report['source_count']} statuses={report['status_counts']} "
        f"json={_repo_rel(json_path)} md={_repo_rel(md_path)}"
    )
    if args.fail_on_error and report["status_counts"].get("error", 0):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
