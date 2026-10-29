#!/usr/bin/env python3
"""Audit normalized runtime ABI facts that closed operation types cannot prove.

Rust kind membership, mapper/classifier domains and backend routing are closed
OpKind matches checked by rustc. Their former source scrapers are retired.
Python producers and wire emission use generated Literal types checked by ty.
The frontend producer census remains in tools/op_kinds/census.py.
"""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import dataclass
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = Path(__file__).resolve().parents[1]


def llvm_boxed_runtime_abi_mismatches() -> list[str]:
    from tools.llvm_runtime_abi_audit import boxed_runtime_export_issues

    return [
        f"{issue.problem}:{issue.symbol}/{issue.classified_arity}:"
        f"expected={issue.expected}:actual={issue.actual}"
        for issue in boxed_runtime_export_issues()
    ]


@dataclass
class AuditResult:
    llvm_boxed_runtime_abi_mismatch: list[str]

    def dangerous(self) -> dict[str, list[str]]:
        return {
            "llvm_boxed_runtime_abi_mismatch": sorted(
                self.llvm_boxed_runtime_abi_mismatch
            ),
        }


def run_audit() -> AuditResult:
    return AuditResult(llvm_boxed_runtime_abi_mismatches())


def main(argv: list[str]) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    result = run_audit()
    report = result.dangerous()
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        print(
            "op-kind normalized ABI audit: "
            + ("FAILED" if any(report.values()) else "OK")
        )
        for category, findings in report.items():
            for finding in findings:
                print(f"  {category}: {finding}")
    return int(any(report.values()))


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
