#!/usr/bin/env python3
from __future__ import annotations

import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]

COMPAT_START = "<!-- GENERATED:compat-summary:start -->"
COMPAT_END = "<!-- GENERATED:compat-summary:end -->"
BENCH_START = "<!-- GENERATED:bench-summary:start -->"
BENCH_END = "<!-- GENERATED:bench-summary:end -->"


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8") if path.exists() else ""


def _check_readme(errors: list[str]) -> None:
    path = ROOT / "README.md"
    text = _read_text(path)
    if "docs/getting-started.md" not in text:
        errors.append("README.md: missing link to docs/getting-started.md")
    if "docs/spec/STATUS.md" not in text:
        errors.append("README.md: missing link to docs/spec/STATUS.md")
    for banned in (
        "Optimization Program Kickoff",
        "Capabilities (Current)",
        "Limitations (Current)",
        "--update-readme",
        "README and [ROADMAP.md](ROADMAP.md) are kept in sync",
        "README and ROADMAP are kept in sync",
    ):
        if banned in text:
            errors.append(f"README.md: contains banned stale section or phrase {banned!r}")


def _check_status(errors: list[str]) -> None:
    path = ROOT / "docs/spec/STATUS.md"
    text = _read_text(path)
    if COMPAT_START not in text or COMPAT_END not in text:
        errors.append("docs/spec/STATUS.md: missing compat-summary generated markers")
    if BENCH_START not in text or BENCH_END not in text:
        errors.append("docs/spec/STATUS.md: missing bench-summary generated markers")


def _check_roadmap(errors: list[str]) -> None:
    path = ROOT / "ROADMAP.md"
    text = _read_text(path)
    for banned in ("Last updated:", "Current Validation Note"):
        if banned in text:
            errors.append(f"ROADMAP.md: contains banned current-state phrase {banned!r}")


def _check_supported(errors: list[str]) -> None:
    path = ROOT / "SUPPORTED.md"
    if not path.exists():
        return
    text = _read_text(path)
    for banned in (
        "operator-facing support contract for Molt",
        "What Molt currently supports",
        "Last updated:",
    ):
        if banned in text:
            errors.append(f"SUPPORTED.md: contains banned secondary-contract phrase {banned!r}")


def _check_benchmarking_docs(errors: list[str]) -> None:
    for rel_path in (
        "docs/BENCHMARKING.md",
        "docs/DEVELOPER_GUIDE.md",
        "docs/spec/areas/perf/0008-benchmarking.md",
        "docs/spec/areas/perf/0603_BENCHMARKS.md",
        "docs/spec/areas/perf/0604_BINARY_SIZE_AND_COLD_START.md",
        "docs/spec/areas/perf/0512_ARCH_OPTIMIZATION_AND_SIMD.md",
        "docs/spec/areas/wasm/WASM_OPTIMIZATION_PLAN.md",
    ):
        text = _read_text(ROOT / rel_path)
        if "--update-readme" in text:
            errors.append(f"{rel_path}: stale benchmark README updater reference '--update-readme'")
        if "README Performance" in text or "README performance" in text:
            errors.append(f"{rel_path}: stale README benchmark ownership reference")
        if "summarized in `README.md`" in text:
            errors.append(f"{rel_path}: stale README benchmark summary ownership reference")


def _check_support_story_refs(errors: list[str]) -> None:
    agents_text = _read_text(ROOT / "AGENTS.md")
    if "README and [ROADMAP.md](ROADMAP.md) are kept in sync" in agents_text:
        errors.append(
            "AGENTS.md: contains stale sync language 'README and [ROADMAP.md](ROADMAP.md) are kept in sync'"
        )

    roadmap_90_text = _read_text(ROOT / "docs/ROADMAP_90_DAYS.md")
    if "stay aligned with both" in roadmap_90_text:
        errors.append(
            "docs/ROADMAP_90_DAYS.md: contains stale dual-truth language 'stay aligned with both'"
        )

    proof_text = _read_text(ROOT / "docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md")
    if "Support contract: [../../SUPPORTED.md]" in proof_text:
        errors.append(
            "docs/proofs/STANDALONE_BINARY_PROOF_WORKFLOW.md: stale SUPPORTED.md support-contract reference"
        )


def check_repo() -> list[str]:
    errors: list[str] = []
    _check_readme(errors)
    _check_status(errors)
    _check_roadmap(errors)
    _check_supported(errors)
    _check_benchmarking_docs(errors)
    _check_support_story_refs(errors)
    return errors


def main() -> int:
    errors = check_repo()
    if errors:
        for error in errors:
            print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
