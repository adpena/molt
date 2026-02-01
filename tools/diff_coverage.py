#!/usr/bin/env python3
"""Summarize differential test coverage metadata.

Produces an aggregate report from `# MOLT_META:` headers and light inference
based on filenames and directory layout. Intended for coverage tracking only;
it does not enforce correctness.
"""

from __future__ import annotations

import argparse
import json
import re
from collections import Counter
from pathlib import Path

PEP_RE = re.compile(r"pep(\d{3,4})", re.IGNORECASE)

MODULE_PREFIXES = {
    "concurrent_futures": "concurrent.futures",
    "http_client": "http.client",
    "http_server": "http.server",
    "http_cookiejar": "http.cookiejar",
    "http_cookies": "http.cookies",
    "importlib_metadata": "importlib.metadata",
    "importlib_resources": "importlib.resources",
    "importlib": "importlib",
    "os_path": "os.path",
    "test_support": "test.support",
    "urllib_parse": "urllib.parse",
    "urllib_request": "urllib.request",
    "urllib_error": "urllib.error",
    "urllib": "urllib",
    "wsgi": "wsgiref",
    "wsgiref": "wsgiref",
}


def collect_meta(file_path: Path) -> dict[str, list[str]]:
    meta: dict[str, list[str]] = {}
    try:
        text = file_path.read_text()
    except OSError:
        return meta
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_META:"):
            continue
        payload = stripped[len("# MOLT_META:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            values = [v for v in value.split(",") if v]
            if not values:
                values = [""]
            meta.setdefault(key, []).extend(values)
    return meta


def infer_pep(path: Path, meta: dict[str, list[str]]) -> list[str]:
    if "pep" in meta:
        return meta["pep"]
    match = PEP_RE.search(path.name)
    return [match.group(1)] if match else []


def infer_stdlib_module(path: Path, meta: dict[str, list[str]]) -> str | None:
    if "stdlib" in meta:
        return meta["stdlib"][0]
    stem = path.stem
    for prefix, module in MODULE_PREFIXES.items():
        if stem.startswith(prefix + "_") or stem == prefix:
            return module
    if "_" in stem:
        return stem.split("_", 1)[0]
    return None


def build_report(entries: list[dict]) -> str:
    total = len(entries)
    with_meta = sum(1 for e in entries if e["meta"])
    by_group = Counter(e["group"] for e in entries)
    by_pep = Counter(pep for e in entries for pep in e["peps"])
    by_stdlib = Counter(e["stdlib"] for e in entries if e["stdlib"])

    lines: list[str] = []
    lines.append("# Differential Coverage Report")
    lines.append("")
    lines.append(f"Total tests: {total}")
    lines.append(f"Tests with MOLT_META: {with_meta}")
    lines.append("")

    lines.append("## By group")
    lines.append("")
    lines.append("| Group | Count |")
    lines.append("| --- | --- |")
    for group, count in sorted(by_group.items()):
        lines.append(f"| {group} | {count} |")
    lines.append("")

    if by_pep:
        lines.append("## PEP coverage (inferred + metadata)")
        lines.append("")
        lines.append("| PEP | Count |")
        lines.append("| --- | --- |")
        for pep, count in sorted(by_pep.items(), key=lambda item: int(item[0])):
            lines.append(f"| {pep} | {count} |")
        lines.append("")

    if by_stdlib:
        lines.append("## Stdlib coverage (inferred + metadata)")
        lines.append("")
        lines.append("| Module | Count |")
        lines.append("| --- | --- |")
        for module, count in sorted(by_stdlib.items()):
            lines.append(f"| {module} | {count} |")
        lines.append("")

    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description="Summarize differential coverage.")
    parser.add_argument(
        "--root",
        default="tests/differential",
        help="Root directory for differential tests.",
    )
    parser.add_argument(
        "--json-output",
        help="Write raw metadata entries to JSON.",
    )
    parser.add_argument(
        "--report-output",
        default="tests/differential/COVERAGE_REPORT.md",
        help="Write Markdown summary report.",
    )
    args = parser.parse_args()

    root = Path(args.root)
    entries: list[dict] = []
    for file_path in sorted(root.rglob("*.py")):
        rel = file_path.relative_to(root)
        group = rel.parts[0] if rel.parts else "root"
        meta = collect_meta(file_path)
        peps = infer_pep(file_path, meta)
        stdlib = infer_stdlib_module(file_path, meta)
        entries.append(
            {
                "path": str(file_path),
                "group": group,
                "meta": meta,
                "peps": peps,
                "stdlib": stdlib,
            }
        )

    if args.json_output:
        Path(args.json_output).write_text(json.dumps(entries, indent=2, sort_keys=True))

    report = build_report(entries)
    if args.report_output:
        Path(args.report_output).write_text(report)
    else:
        print(report)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
