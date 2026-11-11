from __future__ import annotations

import argparse
import ast
import hashlib
import json
import time
from pathlib import Path
from typing import Any

from molt.frontend import SimpleTIRGenerator


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


def _reported_source_path(source_path: Path | None) -> str:
    """The path a build would emit for ``source_path``, as in production."""
    if source_path is None:
        return "<stdin>"
    from molt.cli.module_resolution import _toolchain_package_root

    return _toolchain_package_root().logical_source_path(source_path.resolve())


def _compile_snapshot(
    source: str,
    *,
    source_path: Path | None,
) -> list[dict[str, Any]]:
    filename = str(source_path) if source_path is not None else "<stdin>"
    tree = ast.parse(source, filename=filename)
    generator = SimpleTIRGenerator(source_path=_reported_source_path(source_path))
    generator.visit(tree)
    return list(generator.to_json().get("functions", []))


def _normalize_function_entry(function: dict[str, Any]) -> dict[str, Any]:
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
    return normalized


def _filter_functions(
    functions: list[dict[str, Any]],
    *,
    function_name: str | None,
) -> list[dict[str, Any]]:
    if function_name is None:
        return functions
    return [
        function
        for function in functions
        if str(function.get("name", "")) == function_name
        or _display_function_name(str(function.get("name", ""))) == function_name
    ]


def capture_ir_snapshots(
    source: str,
    *,
    source_path: Path | None = None,
    function_name: str | None = None,
    module_name: str | None = None,
) -> dict[str, Any]:
    snapshots: list[dict[str, Any]] = []
    start = time.perf_counter()
    if _module_matches(module_name, source_path):
        functions = _compile_snapshot(source, source_path=source_path)
        snapshots.append(
            {
                "stage": "frontend",
                "functions": _filter_functions(
                    [_normalize_function_entry(function) for function in functions],
                    function_name=function_name,
                ),
            }
        )
    return {
        "source": str(source_path) if source_path is not None else "<stdin>",
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
    parser.add_argument("--function", help="Only emit the selected function.")
    parser.add_argument("--module", help="Only emit the selected module.")
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
        function_name=args.function,
        module_name=args.module,
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
