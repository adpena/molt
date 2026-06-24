#!/usr/bin/env python3
from __future__ import annotations

import re
import sys
import tomllib
from pathlib import Path, PurePosixPath


ROOT = Path(__file__).resolve().parents[1]

COMPAT_START = "<!-- GENERATED:compat-summary:start -->"
COMPAT_END = "<!-- GENERATED:compat-summary:end -->"
BENCH_START = "<!-- GENERATED:bench-summary:start -->"
BENCH_END = "<!-- GENERATED:bench-summary:end -->"
AUTHORITY_MANIFEST_REL = "docs/design/foundation/authority_manifest.toml"
AUTHORITY_MANIFEST_DOC_REF = "design/foundation/authority_manifest.toml"
FOUNDATION_PORTFOLIO_RE = re.compile(r"^([5-9][0-9])_.*\.md$")
FOUNDATION_BLUEPRINT_META_RE = re.compile(
    r"\bFoundation blueprint\s+([0-9]{2})\b", re.IGNORECASE
)
FOUNDATION_DOC_META_RE = re.compile(r"^doc:\s*([0-9]{2})\s*$", re.IGNORECASE)


def _read_text(path: Path) -> str:
    return path.read_text(encoding="utf-8") if path.exists() else ""


def _is_safe_repo_rel_path(value: str) -> bool:
    path = PurePosixPath(value)
    return (
        bool(value)
        and not path.is_absolute()
        and "\\" not in value
        and ":" not in value
        and ".." not in path.parts
    )


def _load_authority_manifest(errors: list[str]) -> list[dict[str, object]]:
    manifest_path = ROOT / AUTHORITY_MANIFEST_REL
    if not manifest_path.exists():
        errors.append(
            "docs/design/foundation/authority_manifest.toml: missing planning authority manifest"
        )
        return []
    try:
        data = tomllib.loads(_read_text(manifest_path))
    except tomllib.TOMLDecodeError as exc:
        errors.append(
            "docs/design/foundation/authority_manifest.toml: invalid TOML: "
            f"{exc}"
        )
        return []
    entries = data.get("authority")
    if not isinstance(entries, list) or not entries:
        errors.append(
            "docs/design/foundation/authority_manifest.toml: missing [[authority]] entries"
        )
        return []
    valid_entries: list[dict[str, object]] = []
    seen_paths: set[str] = set()
    for index, entry in enumerate(entries, start=1):
        if not isinstance(entry, dict):
            errors.append(
                "docs/design/foundation/authority_manifest.toml: "
                f"authority entry {index} must be a table"
            )
            continue
        path = entry.get("path")
        if not isinstance(path, str) or not _is_safe_repo_rel_path(path):
            errors.append(
                "docs/design/foundation/authority_manifest.toml: "
                f"authority entry {index} has invalid path {path!r}"
            )
            continue
        if path in seen_paths:
            errors.append(
                "docs/design/foundation/authority_manifest.toml: "
                f"duplicate authority path {path}"
            )
            continue
        seen_paths.add(path)
        markers = entry.get("required_markers")
        if not isinstance(markers, list) or not markers or not all(
            isinstance(marker, str) and marker for marker in markers
        ):
            errors.append(
                "docs/design/foundation/authority_manifest.toml: "
                f"{path} missing non-empty string required_markers"
            )
            continue
        for key in ("index_ref", "canonicals_ref"):
            ref = entry.get(key)
            if ref is not None and (
                not isinstance(ref, str) or not _is_safe_repo_rel_path(ref)
            ):
                errors.append(
                    "docs/design/foundation/authority_manifest.toml: "
                    f"{path} has invalid {key} {ref!r}"
                )
                continue
        valid_entries.append(entry)
    return valid_entries


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
            errors.append(
                f"README.md: contains banned stale section or phrase {banned!r}"
            )


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
            errors.append(
                f"ROADMAP.md: contains banned current-state phrase {banned!r}"
            )


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
            errors.append(
                f"SUPPORTED.md: contains banned secondary-contract phrase {banned!r}"
            )


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
            errors.append(
                f"{rel_path}: stale benchmark README updater reference '--update-readme'"
            )
        if "README Performance" in text or "README performance" in text:
            errors.append(f"{rel_path}: stale README benchmark ownership reference")
        if "summarized in `README.md`" in text:
            errors.append(
                f"{rel_path}: stale README benchmark summary ownership reference"
            )


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


def _check_long_horizon_routing(errors: list[str]) -> None:
    manifest = _load_authority_manifest(errors)
    index_text = _read_text(ROOT / "docs/INDEX.md")
    if AUTHORITY_MANIFEST_DOC_REF not in index_text:
        errors.append(
            f"docs/INDEX.md: missing planning authority manifest ref {AUTHORITY_MANIFEST_DOC_REF}"
        )
    seen_index_refs: set[str] = set()
    for entry in manifest:
        ref = entry.get("index_ref")
        if not isinstance(ref, str):
            continue
        if ref in seen_index_refs:
            errors.append(
                "docs/design/foundation/authority_manifest.toml: "
                f"duplicate index_ref {ref}"
            )
            continue
        seen_index_refs.add(ref)
        if ref not in index_text:
            errors.append(f"docs/INDEX.md: missing long-horizon planning ref {ref}")

    canonicals_text = _read_text(ROOT / "docs/CANONICALS.md")
    if AUTHORITY_MANIFEST_DOC_REF not in canonicals_text:
        errors.append(
            f"docs/CANONICALS.md: missing planning authority manifest ref {AUTHORITY_MANIFEST_DOC_REF}"
        )
    seen_canonicals_refs: set[str] = set()
    for entry in manifest:
        ref = entry.get("canonicals_ref")
        if not isinstance(ref, str):
            continue
        if ref in seen_canonicals_refs:
            errors.append(
                "docs/design/foundation/authority_manifest.toml: "
                f"duplicate canonicals_ref {ref}"
            )
            continue
        seen_canonicals_refs.add(ref)
        if ref not in canonicals_text:
            errors.append(f"docs/CANONICALS.md: missing canonical doctrine ref {ref}")

    for entry in manifest:
        rel_path = entry.get("path")
        assert isinstance(rel_path, str)
        markers = entry.get("required_markers")
        assert isinstance(markers, list)
        text = _read_text(ROOT / rel_path)
        if not text:
            errors.append(f"{rel_path}: missing authority document")
            continue
        for marker in markers:
            if marker not in text:
                errors.append(f"{rel_path}: missing authority marker {marker!r}")


def _first_markdown_heading(text: str) -> str | None:
    for line in text.splitlines():
        if line.startswith("# "):
            return line
    return None


def _check_foundation_portfolio_numbering(errors: list[str]) -> None:
    foundation_root = ROOT / "docs/design/foundation"
    if not foundation_root.exists():
        return
    for path in sorted(foundation_root.glob("[5-9][0-9]_*.md")):
        match = FOUNDATION_PORTFOLIO_RE.match(path.name)
        if not match:
            continue
        number = match.group(1)
        rel_path = path.relative_to(ROOT).as_posix()
        text = _read_text(path)
        heading = _first_markdown_heading(text)
        if heading is None:
            errors.append(f"{rel_path}: missing top-level markdown heading")
        elif not re.search(rf"\b{re.escape(number)}\b", heading):
            errors.append(
                f"{rel_path}: heading number must match filename prefix {number}"
            )
        front_matter = "\n".join(text.splitlines()[:25])
        for meta_match in FOUNDATION_BLUEPRINT_META_RE.finditer(front_matter):
            if meta_match.group(1) != number:
                errors.append(
                    f"{rel_path}: Foundation blueprint metadata must match filename prefix {number}"
                )
        for line in front_matter.splitlines():
            meta_match = FOUNDATION_DOC_META_RE.match(line.strip())
            if meta_match and meta_match.group(1) != number:
                errors.append(
                    f"{rel_path}: doc metadata must match filename prefix {number}"
                )


def check_repo() -> list[str]:
    errors: list[str] = []
    _check_readme(errors)
    _check_status(errors)
    _check_roadmap(errors)
    _check_supported(errors)
    _check_benchmarking_docs(errors)
    _check_support_story_refs(errors)
    _check_long_horizon_routing(errors)
    _check_foundation_portfolio_numbering(errors)
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
