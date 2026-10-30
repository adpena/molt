#!/usr/bin/env python3
from __future__ import annotations

import enum
import os
import re
import subprocess
import sys
import tomllib
from collections.abc import Iterable
from pathlib import Path, PurePosixPath
from urllib.parse import unquote, urlsplit

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

from markdown_it import MarkdownIt  # noqa: E402

from tools import finding_status  # noqa: E402
from tools.command_execution import CommandExecutor  # noqa: E402
from molt.source_admission import repository_source_paths  # noqa: E402


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
LOCAL_LINK_AUTHORITY_DOCS = (
    "README.md",
    "docs/INDEX.md",
    "docs/CANONICALS.md",
    "docs/spec/README.md",
)
ROADMAP_REL = "ROADMAP.md"
SESSION_PLAN_TREE = ("docs", "superpowers")
DATED_PLAN_NAME_RE = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}-.*plan.*\.md$", re.IGNORECASE
)
DOCUMENT_SUFFIXES = frozenset({".md", ".rst", ".txt"})
_COMMANDS = CommandExecutor.for_file(__file__)


class PlanningDocumentRefusal(enum.Enum):
    """Why a repository path may not hold a planning document."""

    SESSION_PLAN_TREE = (
        "session plans and specs do not belong in docs/superpowers/; fold durable "
        "design into the owning design doc and open work into the findings ledger"
    )
    DATED_PLAN = (
        "dated plan documents go stale; fold durable design into the owning design "
        "doc and open work into the findings ledger"
    )
    SECOND_ROADMAP = (
        "ROADMAP.md is the one forward plan; link to it instead of adding a roadmap"
    )


def planning_document_refusal(rel_path: str) -> PlanningDocumentRefusal | None:
    """Classify one repository-relative POSIX path; None admits it."""
    path = PurePosixPath(rel_path)
    if path.parts[: len(SESSION_PLAN_TREE)] == SESSION_PLAN_TREE:
        return PlanningDocumentRefusal.SESSION_PLAN_TREE
    if DATED_PLAN_NAME_RE.match(path.name):
        return PlanningDocumentRefusal.DATED_PLAN
    if (
        rel_path != ROADMAP_REL
        and path.suffix.lower() in DOCUMENT_SUFFIXES
        and "roadmap" in path.name.lower()
    ):
        return PlanningDocumentRefusal.SECOND_ROADMAP
    return None


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
            f"docs/design/foundation/authority_manifest.toml: invalid TOML: {exc}"
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
        if (
            not isinstance(markers, list)
            or not markers
            or not all(isinstance(marker, str) and marker for marker in markers)
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
    owners_by_number: dict[str, list[Path]] = {}
    for path in sorted(foundation_root.glob("[5-9][0-9]_*.md")):
        match = FOUNDATION_PORTFOLIO_RE.match(path.name)
        if not match:
            continue
        number = match.group(1)
        owners_by_number.setdefault(number, []).append(path)
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
    for number, owners in sorted(owners_by_number.items()):
        if len(owners) < 2:
            continue
        rel_paths = ", ".join(path.relative_to(ROOT).as_posix() for path in owners)
        errors.append(
            f"foundation portfolio number {number} has multiple authorities: "
            f"{rel_paths}"
        )


def _markdown_link_targets(text: str) -> list[tuple[int, str]]:
    targets: list[tuple[int, str]] = []
    for block in MarkdownIt("commonmark").parse(text):
        if block.children is None:
            continue
        line = block.map[0] + 1 if block.map is not None else 1
        for token in block.children:
            attribute = "href" if token.type == "link_open" else "src"
            if token.type not in {"link_open", "image"}:
                continue
            target = token.attrGet(attribute)
            if isinstance(target, str) and target:
                targets.append((line, target))
    return targets


def _has_exact_path_case(
    path: Path,
    *,
    root: Path,
    directory_entries: dict[Path, frozenset[str]],
) -> bool:
    current = root
    for part in path.relative_to(root).parts:
        entries = directory_entries.get(current)
        if entries is None:
            try:
                entries = frozenset(entry.name for entry in current.iterdir())
            except OSError:
                return False
            directory_entries[current] = entries
        if part not in entries:
            return False
        current /= part
    return True


def _check_local_markdown_links(errors: list[str]) -> None:
    root = ROOT.resolve()
    directory_entries: dict[Path, frozenset[str]] = {}
    for rel_path in LOCAL_LINK_AUTHORITY_DOCS:
        source = root / rel_path
        if not source.is_file():
            errors.append(f"{rel_path}: missing Markdown link authority")
            continue
        for line, target in _markdown_link_targets(_read_text(source)):
            if "\\" in target:
                errors.append(
                    f"{rel_path}:{line}: local Markdown target must use '/' separators: "
                    f"{target!r}"
                )
                continue
            try:
                parsed = urlsplit(target)
            except ValueError as exc:
                errors.append(
                    f"{rel_path}:{line}: malformed Markdown target {target!r}: {exc}"
                )
                continue
            if target.startswith(("#", "/")) or parsed.scheme or parsed.netloc:
                continue
            local_path = unquote(parsed.path)
            if not local_path:
                continue
            try:
                unresolved = Path(os.path.abspath(source.parent / local_path))
                unresolved.relative_to(root)
                resolved = unresolved.resolve()
                resolved_rel = resolved.relative_to(root).as_posix()
            except (OSError, RuntimeError, ValueError):
                errors.append(
                    f"{rel_path}:{line}: local Markdown target escapes repository: "
                    f"{target!r}"
                )
                continue
            if not resolved.exists():
                errors.append(
                    f"{rel_path}:{line}: broken local Markdown target {target!r}; "
                    f"expected {resolved_rel}"
                )
            elif not _has_exact_path_case(
                unresolved, root=root, directory_entries=directory_entries
            ):
                errors.append(
                    f"{rel_path}:{line}: local Markdown target has a case mismatch: "
                    f"{target!r}"
                )


def _check_handoff_ledger(errors: list[str]) -> None:
    """Report the structural defects of the findings ledger projection."""
    path = ROOT / finding_status.LEDGER_PATH
    if not path.exists():
        return
    errors.extend(
        f"{finding_status.LEDGER_PATH}: {problem}"
        for problem in finding_status.read_ledger(ROOT).problems
    )


def _read_git(cwd: Path, arguments: list[str]) -> bytes:
    return _COMMANDS.run(
        ["git", *arguments], cwd=cwd, capture_output=True, timeout=30, check=True
    ).stdout


def _repository_paths() -> list[str]:
    """Tracked and untracked-but-unignored paths: Git owns the boundary."""
    root = ROOT.resolve()
    return [
        path.relative_to(root).as_posix()
        for path in repository_source_paths(
            root, run_git=_read_git, include_untracked=True
        )
    ]


def _check_planning_documents(
    errors: list[str], repository_paths: Iterable[str]
) -> None:
    for rel_path in repository_paths:
        refusal = planning_document_refusal(rel_path)
        if refusal is not None:
            errors.append(f"{rel_path}: {refusal.value}")


def check_repo() -> list[str]:
    errors: list[str] = []
    try:
        repository_paths = _repository_paths()
    except (OSError, ValueError, subprocess.SubprocessError) as exc:
        errors.append(f"cannot list repository paths through Git: {exc}")
    else:
        _check_planning_documents(errors, repository_paths)
    _check_readme(errors)
    _check_status(errors)
    _check_roadmap(errors)
    _check_supported(errors)
    _check_benchmarking_docs(errors)
    _check_support_story_refs(errors)
    _check_long_horizon_routing(errors)
    _check_foundation_portfolio_numbering(errors)
    _check_handoff_ledger(errors)
    _check_local_markdown_links(errors)
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
