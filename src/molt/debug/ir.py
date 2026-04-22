from __future__ import annotations

import argparse
import ast
import hashlib
import json
import os
import time
from contextlib import contextmanager
from pathlib import Path
from typing import Any, Iterator

from molt.frontend import SimpleTIRGenerator


VALID_STAGES = ("pre-midend", "post-midend", "all")


def _ops_fingerprint(ops_json: list[dict[str, Any]]) -> str:
    canonical = json.dumps(ops_json, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()[:16]


def _format_ops_text(ops_json: list[dict[str, Any]], *, indent: int = 2) -> str:
    lines: list[str] = []
    prefix = " " * indent
    for index, op in enumerate(ops_json):
        kind = op.get("kind", "?")
        result = op.get("result")
        value = op.get("value")
        parts = [f"{prefix}{index:4d}  {kind}"]
        if result is not None:
            parts.append(f" -> {result}")
        if value is not None:
            value_text = str(value)
            if len(value_text) > 60:
                value_text = value_text[:57] + "..."
            parts.append(f"  [{value_text}]")
        lines.append("".join(parts))
    return "\n".join(lines)


def _display_function_name(raw_name: str) -> str:
    if "__" not in raw_name:
        return raw_name
    parts = [part for part in raw_name.split("__") if part]
    if not parts:
        return raw_name
    return parts[-1]


@contextmanager
def _temporary_env(name: str, value: str | None) -> Iterator[None]:
    previous = os.environ.get(name)
    if value is None:
        os.environ.pop(name, None)
    else:
        os.environ[name] = value
    try:
        yield
    finally:
        if previous is None:
            os.environ.pop(name, None)
        else:
            os.environ[name] = previous


def _normalize_module_name(module_filter: str | None) -> str | None:
    if module_filter is None:
        return None
    normalized = module_filter.strip()
    return normalized or None


def _module_matches(module_filter: str | None, source_path: Path | None) -> bool:
    normalized = _normalize_module_name(module_filter)
    if normalized is None or source_path is None:
        return True
    candidates = {
        normalized,
        normalized.replace("\\", "/"),
    }
    source_text = source_path.as_posix()
    stem = source_path.stem
    name = source_path.name
    return any(candidate in {source_text, stem, name} for candidate in candidates)


def _compile_snapshot(
    source: str,
    *,
    source_path: Path | None,
    disable_midend: bool,
) -> tuple[list[dict[str, Any]], dict[str, dict[str, Any]]]:
    filename = str(source_path) if source_path is not None else "<stdin>"
    tree = ast.parse(source, filename=filename)
    with _temporary_env("MOLT_MIDEND_DISABLE", "1" if disable_midend else None):
        generator = SimpleTIRGenerator(source_path=filename)
        generator.visit(tree)
        ir_json = generator.to_json()
    return (
        list(ir_json.get("functions", [])),
        dict(generator.midend_pass_stats_by_function),
    )


def _normalize_function_entry(
    function: dict[str, Any],
    *,
    pass_stats: dict[str, Any] | None,
) -> dict[str, Any]:
    ops = list(function.get("ops", []))
    normalized: dict[str, Any] = {
        "name": function.get("name", "?"),
        "op_count": len(ops),
        "fingerprint": _ops_fingerprint(ops),
        "ops": ops,
    }
    source_file = function.get("source_file")
    if isinstance(source_file, str) and source_file:
        normalized["source_file"] = source_file
    if pass_stats:
        normalized["pass_stats"] = {
            key: value for key, value in sorted(pass_stats.items())
        }
    return normalized


def _filter_functions(
    functions: list[dict[str, Any]],
    *,
    function_name: str | None,
    pass_name: str | None,
) -> list[dict[str, Any]]:
    filtered: list[dict[str, Any]] = []
    for function in functions:
        if function_name is not None:
            raw_name = str(function.get("name", ""))
            normalized_name = raw_name.split("__")[-1]
            if raw_name != function_name and normalized_name != function_name:
                continue
        if pass_name is not None:
            pass_stats = function.get("pass_stats")
            if not isinstance(pass_stats, dict) or pass_name not in pass_stats:
                continue
        filtered.append(function)
    return filtered


def capture_ir_snapshots(
    source: str,
    *,
    source_path: Path | None = None,
    stage: str = "all",
    function_name: str | None = None,
    module_name: str | None = None,
    pass_name: str | None = None,
) -> dict[str, Any]:
    if stage not in VALID_STAGES:
        raise ValueError(f"unsupported IR dump stage: {stage}")
    if not _module_matches(module_name, source_path):
        snapshots: list[dict[str, Any]] = []
        return {
            "source": str(source_path) if source_path is not None else "<stdin>",
            "requested_stage": stage,
            "timing_ms": 0.0,
            "snapshots": snapshots,
            "error": None,
        }

    snapshots: list[dict[str, Any]] = []
    start = time.perf_counter()
    if stage in {"pre-midend", "all"}:
        pre_functions, pre_pass_stats = _compile_snapshot(
            source,
            source_path=source_path,
            disable_midend=True,
        )
        normalized_functions = [
            _normalize_function_entry(
                function,
                pass_stats=pre_pass_stats.get(str(function.get("name"))),
            )
            for function in pre_functions
        ]
        snapshots.append(
            {
                "stage": "pre-midend",
                "functions": _filter_functions(
                    normalized_functions,
                    function_name=function_name,
                    pass_name=pass_name,
                ),
            }
        )
    if stage in {"post-midend", "all"}:
        post_functions, post_pass_stats = _compile_snapshot(
            source,
            source_path=source_path,
            disable_midend=False,
        )
        normalized_functions = [
            _normalize_function_entry(
                function,
                pass_stats=post_pass_stats.get(str(function.get("name"))),
            )
            for function in post_functions
        ]
        snapshots.append(
            {
                "stage": "post-midend",
                "functions": _filter_functions(
                    normalized_functions,
                    function_name=function_name,
                    pass_name=pass_name,
                ),
            }
        )

    return {
        "source": str(source_path) if source_path is not None else "<stdin>",
        "requested_stage": stage,
        "timing_ms": round((time.perf_counter() - start) * 1000.0, 3),
        "snapshots": snapshots,
        "error": None,
    }


def render_ir_json(result: dict[str, Any]) -> str:
    return json.dumps(result, indent=2, sort_keys=True) + "\n"


def render_ir_text(result: dict[str, Any]) -> str:
    lines = [
        f"=== IR Dump: {result.get('source', '<stdin>')} ({result.get('timing_ms', 0.0):.1f} ms) ===",
        "",
    ]
    for snapshot in result.get("snapshots", []):
        lines.append(f"--- Stage: {snapshot['stage']} ---")
        for function in snapshot.get("functions", []):
            display_name = _display_function_name(str(function.get("name", "?")))
            lines.append(
                "  Function: {name}  ({count} ops, fingerprint={fingerprint})".format(
                    name=display_name,
                    count=function.get("op_count", 0),
                    fingerprint=function.get("fingerprint", ""),
                )
            )
            lines.append(_format_ops_text(function.get("ops", [])))
            lines.append("")
        lines.append("")
    return "\n".join(lines)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Dump Molt IR snapshots.")
    parser.add_argument("source", help="Python source file to compile")
    parser.add_argument(
        "--stage",
        choices=sorted(VALID_STAGES),
        default="all",
        help="Which compilation stage(s) to dump.",
    )
    parser.add_argument("--function", help="Only emit the selected function.")
    parser.add_argument("--module", help="Only emit the selected module.")
    parser.add_argument(
        "--pass",
        dest="pass_name",
        help="Only emit functions touching the selected pass.",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        dest="json_output",
        help="Emit JSON instead of text.",
    )
    parser.add_argument(
        "--out-dir",
        type=Path,
        help="Optional directory to write JSON output into.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    parser = build_parser()
    args = parser.parse_args(argv)
    source_path = Path(args.source)
    if not source_path.is_file():
        parser.error(f"{source_path} is not a file")
    source = source_path.read_text(encoding="utf-8")
    result = capture_ir_snapshots(
        source,
        source_path=source_path,
        stage=args.stage,
        function_name=args.function,
        module_name=args.module,
        pass_name=args.pass_name,
    )
    if args.json_output:
        rendered = render_ir_json(result)
        if args.out_dir is not None:
            args.out_dir.mkdir(parents=True, exist_ok=True)
            out_path = args.out_dir / f"{source_path.stem}_ir_dump.json"
            out_path.write_text(rendered, encoding="utf-8")
            print(f"Wrote: {out_path}")
        else:
            print(rendered, end="")
    else:
        print(render_ir_text(result), end="")
    return 0
