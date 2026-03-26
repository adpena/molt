from __future__ import annotations

import argparse
from concurrent import futures
from collections import Counter
import json
import os
import re
from pathlib import Path
from typing import Any


TODO_RE = re.compile(
    r"^\s*(?:(?:#|//|--|/\*+|[-*])\s+)?TODO\((?P<area>[^,]+),\s*owner:(?P<owner>[^,]+),\s*milestone:(?P<milestone>[^,]+),\s*priority:(?P<priority>P[0-3]),\s*status:(?P<status>[^)]+)\):\s*(?P<body>.*?)\s*$"
)

DOC_SOURCE_FILES = [
    "ROADMAP.md",
    "docs/spec/STATUS.md",
    "OPTIMIZATIONS_PLAN.md",
    "AGENTS.md",
]
SOURCE_ROOTS = [
    "src",
    "runtime",
    "tools",
    "tests",
    "formal",
    "demo",
]
SOURCE_SUFFIXES = {".py", ".rs", ".md", ".lean"}
SOURCE_MODES = ("codebase", "all")

PRIORITY_TO_LINEAR = {
    "P0": 1,
    "P1": 2,
    "P2": 3,
    "P3": 4,
}
LINEAR_TO_PRIORITY = {value: key for key, value in PRIORITY_TO_LINEAR.items()}
STATUS_SEVERITY = {
    "missing": 0,
    "partial": 1,
    "planned": 2,
    "todo": 2,
    "in-progress": 3,
    "done": 4,
}
PROJECT_SLUGS = {
    "Compiler & Frontend": "compiler-and-frontend",
    "Runtime & Intrinsics": "runtime-and-intrinsics",
    "WASM Parity": "wasm-parity",
    "Performance & Benchmarking": "performance-and-benchmarking",
    "Testing & Differential": "testing-and-differential",
    "Tooling & DevEx": "tooling-and-devex",
    "Security & Supply Chain": "security-and-supply-chain",
    "Offload & Data Ecosystem": "offload-and-data-ecosystem",
}
GROUP_FAMILY_LABELS = {
    "compiler-optimization-and-lowering": "compiler optimization and lowering",
    "compiler-coverage-and-correctness": "compiler coverage and correctness",
    "compiler-language-surface-parity": "compiler language surface parity",
    "runtime-async-and-concurrency": "runtime async and concurrency",
    "stdlib-intrinsic-migration": "stdlib intrinsic migration",
    "runtime-core-module-parity": "runtime core module parity",
    "offload-database-and-dataframe": "database and dataframe offload",
    "offload-network-and-services": "network and service offload",
    "offload-transpiler-and-acceleration": "transpiler and acceleration",
    "tooling-build-throughput-and-caching": "build throughput and caching",
    "tooling-extension-build-and-abi": "extension build and ABI",
    "tooling-workflow-and-automation": "workflow and automation",
    "performance-and-benchmarking": "performance and benchmarking",
    "formal-methods-and-verification": "formal methods and verification",
    "differential-and-test-infra": "differential and test infrastructure",
    "wasm-host-and-io-parity": "wasm host and I/O parity",
    "wasm-runtime-and-compat": "wasm runtime and compatibility",
    "security-and-supply-chain": "security and supply chain",
}
DOC_SOURCE_PREFIXES = (
    "roadmap.md:",
    "docs/spec/status.md:",
    "docs/",
)

MIN_BODY_CHARS = 20
IGNORED_BODY_SNIPPETS = (
    "test fixture",
    "example",
    "todo contracts",
)
SEED_HEADER = "Auto-seeded from Molt codebase TODO contracts."
LEGACY_SEED_HEADERS = (
    SEED_HEADER,
    "Auto-seeded from Molt roadmap/status TODO contracts.",
)


def _display_source_path(path: Path, repo_root: Path | None) -> str:
    if repo_root is None:
        return str(path)
    try:
        return str(path.resolve().relative_to(repo_root.resolve()))
    except ValueError:
        return str(path)


def extract_todos(
    path: Path, *, repo_root: Path | None = None
) -> list[dict[str, Any]]:
    todos: list[dict[str, Any]] = []
    source_path = _display_source_path(path, repo_root)
    lines = path.read_text(encoding="utf-8").splitlines()
    for index, line in enumerate(lines):
        match = TODO_RE.search(line)
        if not match:
            continue
        entry = {k: v.strip() for k, v in match.groupdict().items()}
        if not entry["body"]:
            entry["body"] = _collect_continuation_body(lines, start=index + 1)
        entry["source_file"] = source_path
        entry["source_line"] = index + 1
        todos.append(entry)
    return todos


def _extract_todos_worker(payload: tuple[str, str | None]) -> list[dict[str, Any]]:
    path_text, repo_root_text = payload
    repo_root = Path(repo_root_text) if repo_root_text else None
    return extract_todos(Path(path_text), repo_root=repo_root)


def normalize_issue(todo: dict[str, Any]) -> dict[str, Any]:
    area = todo["area"]
    owner = todo["owner"]
    milestone = todo["milestone"]
    priority = todo["priority"]
    status = todo["status"]
    body = _sanitize_body(todo["body"])
    if not _is_actionable_body(body):
        raise ValueError(f"non_actionable_todo_body:{body}")

    title = f"[{priority}][{milestone}] {body}"
    metadata = {
        "area": area,
        "owner": owner,
        "milestone": milestone,
        "priority": priority,
        "status": status,
        "source": f"{todo['source_file']}:{todo['source_line']}",
    }
    description = (
        f"{SEED_HEADER}\n\n"
        f"Original TODO: {body}\n"
        f"Area: {area}\n"
        f"Owner lane: {owner}\n"
        f"Milestone: {milestone}\n"
        f"Status tag: {status}"
    )
    return {
        "title": title,
        "description": description,
        "priority": PRIORITY_TO_LINEAR.get(priority, 3),
        "metadata": metadata,
    }


def dedupe(items: list[dict[str, Any]]) -> list[dict[str, Any]]:
    best_by_key: dict[str, dict[str, Any]] = {}
    for item in items:
        metadata = item.get("metadata", {})
        core = _canonicalize_for_dedupe(item["title"])
        key = "|".join(
            [
                str(metadata.get("area", "")).lower(),
                str(metadata.get("owner", "")).lower(),
                str(metadata.get("milestone", "")).lower(),
                str(metadata.get("priority", "")).lower(),
                core,
            ]
        )
        existing = best_by_key.get(key)
        if existing is None or _dedupe_rank(item) < _dedupe_rank(existing):
            best_by_key[key] = item
    return list(best_by_key.values())


def group_manifest_items(
    project: str, items: list[dict[str, Any]]
) -> list[dict[str, Any]]:
    groups: dict[str, list[dict[str, Any]]] = {}
    for item in items:
        group_key = _group_key(project, item)
        groups.setdefault(group_key, []).append(item)

    grouped: list[dict[str, Any]] = []
    for group_key, group_items in groups.items():
        grouped.append(_build_group_issue(project, group_key, group_items))

    grouped.sort(
        key=lambda item: (
            int(item.get("priority") or 5),
            -int((item.get("metadata") or {}).get("impact_score") or 0),
            str(item.get("title") or ""),
        )
    )
    return grouped


def _sanitize_body(raw: str) -> str:
    body = raw.replace("\\n", " ").replace("\n", " ").strip()
    body = re.sub(r"\s+", " ", body)
    body = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", body)
    body = body.strip("`'\"")
    body = re.sub(r"\s*\|\s*$", "", body)
    body = re.sub(r'"\s*$', "", body)
    body = re.sub(r"\s+\(TODO\([^)]*\):.*\)\s*$", "", body)
    body = re.sub(r",\s*$", "", body)
    body = re.sub(r"\.\)\s*$", ")", body)
    body = re.sub(r"\)\)\s*$", ")", body)
    body = body.rstrip(".,;: ")
    body = body.replace(").", ")")
    body = body.replace('",', "")
    body = body.replace('."', "")
    return body.strip()


def _collect_continuation_body(lines: list[str], *, start: int) -> str:
    body_lines: list[str] = []
    for raw in lines[start:]:
        stripped = raw.strip()
        if not stripped:
            if body_lines:
                break
            continue
        if "TODO(" in stripped:
            break
        cleaned = _strip_comment_prefix(stripped)
        if cleaned is None:
            break
        body_lines.append(cleaned)
    return " ".join(body_lines).strip()


def _strip_comment_prefix(line: str) -> str | None:
    for prefix in ("#", "//", "--", "/*", "*", "-"):
        if line.startswith(prefix):
            return line[len(prefix) :].strip()
    return None


def _is_actionable_body(body: str) -> bool:
    lowered = body.lower()
    if len(body) < MIN_BODY_CHARS:
        return False
    if len(body.split()) < 4:
        return False
    if any(token in lowered for token in IGNORED_BODY_SNIPPETS):
        return False
    return True


def _canonicalize_for_dedupe(text: str) -> str:
    lowered = text.lower()
    if " (" in lowered:
        lowered = lowered.split(" (", 1)[0]
    lowered = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", lowered)
    lowered = lowered.replace("\\n", " ")
    lowered = re.sub(r"[^a-z0-9]+", " ", lowered)
    lowered = re.sub(r"\s+", " ", lowered).strip()
    return lowered


def _group_key(project: str, item: dict[str, Any]) -> str:
    family = _group_family(project, item)
    project_slug = PROJECT_SLUGS.get(project, _slugify(project))
    return f"{project_slug}:{family}"


def _group_family(project: str, item: dict[str, Any]) -> str:
    metadata = item.get("metadata") or {}
    area = str(metadata.get("area") or "").strip().lower()
    owner = str(metadata.get("owner") or "").strip().lower()
    milestone = str(metadata.get("milestone") or "").strip().upper()
    title = str(item.get("title") or "").strip().lower()
    haystack = " ".join(part for part in [area, owner, milestone.lower(), title] if part)

    if project == "Compiler & Frontend":
        if milestone.startswith("LF") or any(
            token in haystack
            for token in ("sccp", "dce", "phi", "mid-end", "lowering", "guard", "deopt")
        ):
            return "compiler-optimization-and-lowering"
        if milestone.startswith("TC") or any(
            token in haystack
            for token in ("coverage", "async generator", "opcode", "kw_names")
        ):
            return "compiler-coverage-and-correctness"
        return "compiler-language-surface-parity"

    if project == "Runtime & Intrinsics":
        if "stdlib" in haystack or "top level stub" in haystack or "intrinsic" in haystack:
            return "stdlib-intrinsic-migration"
        if any(
            token in haystack
            for token in (
                "async",
                "socket",
                "thread",
                "cancellation",
                "event loop",
                "concurrency",
                "await",
            )
        ) or milestone.startswith("RT"):
            return "runtime-async-and-concurrency"
        return "runtime-core-module-parity"

    if project == "Offload & Data Ecosystem":
        if any(
            token in haystack
            for token in ("db", "sql", "sqlite", "postgres", "dataframe", "pandas")
        ):
            return "offload-database-and-dataframe"
        if any(
            token in haystack
            for token in ("http", "asgi", "websocket", "stream", "service")
        ):
            return "offload-network-and-services"
        return "offload-transpiler-and-acceleration"

    if project == "Tooling & DevEx":
        if any(
            token in haystack
            for token in ("extension", "abi", "libmolt", "headers")
        ):
            return "tooling-extension-build-and-abi"
        if any(
            token in haystack
            for token in (
                "cache",
                "daemon",
                "diff",
                "compile",
                "throughput",
                "build",
                "sccache",
            )
        ):
            return "tooling-build-throughput-and-caching"
        return "tooling-workflow-and-automation"

    if project == "Performance & Benchmarking":
        return "performance-and-benchmarking"

    if project == "Testing & Differential":
        if any(token in haystack for token in ("formal", "lean", "quint", "proof")):
            return "formal-methods-and-verification"
        return "differential-and-test-infra"

    if project == "WASM Parity":
        if any(
            token in haystack
            for token in ("browser", "host", "socket", "connect", "websocket", "db")
        ):
            return "wasm-host-and-io-parity"
        return "wasm-runtime-and-compat"

    if project == "Security & Supply Chain":
        return "security-and-supply-chain"

    return _slugify(owner or area or milestone or title or "misc")


def _build_group_issue(
    project: str, group_key: str, items: list[dict[str, Any]]
) -> dict[str, Any]:
    sorted_items = sorted(
        items,
        key=lambda item: (
            int(item.get("priority") or 5),
            str((item.get("metadata") or {}).get("milestone") or ""),
            str(item.get("title") or ""),
        ),
    )
    highest_priority = min(int(item.get("priority") or 5) for item in sorted_items)
    priority_tag = LINEAR_TO_PRIORITY.get(highest_priority, "P2")
    impact_score = _impact_score(sorted_items)
    impact_label = _impact_label(
        impact_score=impact_score,
        priority=highest_priority,
        leaf_count=len(sorted_items),
    )
    title = (
        f"[{priority_tag}][{impact_label}] {project}: "
        f"{_group_title_from_key(group_key)} backlog"
    )

    metadata_rows = [item.get("metadata") or {} for item in sorted_items]
    owners = _sorted_unique(str(row.get("owner") or "").strip() for row in metadata_rows)
    milestones = _sorted_unique(
        str(row.get("milestone") or "").strip() for row in metadata_rows
    )
    sources = _sorted_unique(str(row.get("source") or "").strip() for row in metadata_rows)
    areas = _sorted_unique(str(row.get("area") or "").strip() for row in metadata_rows)
    statuses = [str(row.get("status") or "").strip().lower() for row in metadata_rows]
    source_kinds = [_source_kind(str(row.get("source") or "").strip()) for row in metadata_rows]
    priority_counts = Counter(
        str(row.get("priority") or LINEAR_TO_PRIORITY.get(int(item.get("priority") or 3), "P2"))
        for row, item in zip(metadata_rows, sorted_items)
    )
    status_counts = Counter(status for status in statuses if status)
    source_kind_counts = Counter(source_kinds)
    codebacked_count = sum(
        source_kind_counts.get(kind, 0)
        for kind in ("code", "tool", "test", "formal")
    )
    docbacked_count = source_kind_counts.get("doc", 0)

    noncode_count = len(sorted_items) - codebacked_count
    description_lines = [
        SEED_HEADER,
        "",
        (
            f"Grouped from {len(sorted_items)} leaf items into one active category issue "
            f"for {project}."
        ),
        "",
        "Source of truth: codebase TODO contracts under src/, runtime/, tools/, tests/, formal/, and demo/.",
        f"Impact: {impact_label} (score {impact_score}).",
        (
            "Pressure: "
            f"P0: {priority_counts.get('P0', 0)}, "
            f"P1: {priority_counts.get('P1', 0)}, "
            f"P2: {priority_counts.get('P2', 0)}, "
            f"P3: {priority_counts.get('P3', 0)}."
        ),
        (
            "Status pressure: "
            f"missing: {status_counts.get('missing', 0)}, "
            f"partial: {status_counts.get('partial', 0)}, "
            f"planned: {status_counts.get('planned', 0)}."
        ),
        f"Codebase-backed pressure: {codebacked_count} leaf items.",
        f"Secondary non-code pressure: {noncode_count} leaf items.",
    ]
    if owners:
        description_lines.append(f"Owners: {', '.join(owners)}.")
    if milestones:
        description_lines.append(f"Milestones: {', '.join(milestones)}.")
    if sources:
        description_lines.append(
            f"Representative sources: {'; '.join(sources[:5])}."
        )
    description_lines.extend(
        [
            "",
            "Leaf inventory:",
            *[f"- {item['title']}" for item in sorted_items],
        ]
    )

    status = _worst_status(statuses)
    metadata = {
        "area": areas[0] if areas else "unknown",
        "codebase_source_of_truth": "true",
        "code_source_count": codebacked_count,
        "codebacked_leaf_count": codebacked_count,
        "doc_source_count": docbacked_count,
        "docbacked_leaf_count": docbacked_count,
        "group_key": group_key,
        "impact": impact_label,
        "impact_score": impact_score,
        "kind": "grouped",
        "leaf_count": len(sorted_items),
        "milestone": milestones[0] if milestones else "unknown",
        "milestones": ", ".join(milestones),
        "missing_count": status_counts.get("missing", 0),
        "owner": owners[0] if owners else "unknown",
        "owners": ", ".join(owners),
        "p0_count": priority_counts.get("P0", 0),
        "p1_count": priority_counts.get("P1", 0),
        "p2_count": priority_counts.get("P2", 0),
        "p3_count": priority_counts.get("P3", 0),
        "priority": priority_tag,
        "secondary_signal_count": noncode_count,
        "source": sources[0] if sources else "multiple",
        "sources": "; ".join(sources[:5]),
        "status": status,
    }
    return {
        "title": title,
        "description": "\n".join(description_lines).strip(),
        "priority": highest_priority,
        "metadata": metadata,
    }


def _group_title_from_key(group_key: str) -> str:
    family = group_key.split(":", 1)[1] if ":" in group_key else group_key
    label = GROUP_FAMILY_LABELS.get(family, family.replace("-", " "))
    return label


def _impact_score(items: list[dict[str, Any]]) -> int:
    score = 0
    for item in items:
        priority = int(item.get("priority") or 5)
        metadata = item.get("metadata") or {}
        status = str(metadata.get("status") or "").strip().lower()
        source_kind = _source_kind(str(metadata.get("source") or "").strip())
        if priority == 1:
            score += 40
        elif priority == 2:
            score += 18
        elif priority == 3:
            score += 8
        else:
            score += 3

        if status == "missing":
            score += 14
        elif status == "partial":
            score += 8
        elif status == "planned":
            score += 4
        elif source_kind == "doc":
            score += 0
        else:
            score += 0

        if source_kind == "code":
            score += 10
        elif source_kind == "formal":
            score += 9
        elif source_kind == "test":
            score += 8
        elif source_kind == "tool":
            score += 7
        elif source_kind == "doc":
            score += 0
        else:
            score += 1
    return score


def _impact_label(*, impact_score: int, priority: int, leaf_count: int) -> str:
    if priority == 1 and (impact_score >= 80 or leaf_count >= 3):
        return "Critical Impact"
    if priority <= 2 and (impact_score >= 35 or leaf_count >= 3):
        return "High Impact"
    if impact_score >= 16 or leaf_count >= 2:
        return "Medium Impact"
    return "Focused Impact"


def _worst_status(statuses: list[str]) -> str:
    filtered = [status for status in statuses if status]
    if not filtered:
        return "planned"
    return min(filtered, key=lambda status: STATUS_SEVERITY.get(status, 99))


def _sorted_unique(values: Any) -> list[str]:
    result = sorted({value for value in values if value})
    return result


def _slugify(text: str) -> str:
    lowered = text.strip().lower()
    lowered = re.sub(r"[^a-z0-9]+", "-", lowered)
    return lowered.strip("-") or "misc"


def _source_kind(source: str) -> str:
    lowered = source.strip().lower()
    if not lowered:
        return "unknown"
    if lowered.startswith(("src/", "runtime/")):
        return "code"
    if lowered.startswith("demo/"):
        return "code"
    if lowered.startswith("tools/"):
        return "tool"
    if lowered.startswith("tests/"):
        return "test"
    if lowered.startswith("formal/"):
        return "formal"
    if lowered.startswith(DOC_SOURCE_PREFIXES):
        return "doc"
    return "doc"


def _dedupe_rank(item: dict[str, Any]) -> tuple[int, str]:
    metadata = item.get("metadata") or {}
    source = str(metadata.get("source") or "").strip()
    kind = _source_kind(source)
    kind_rank = {
        "code": 0,
        "tool": 1,
        "test": 2,
        "doc": 3,
        "unknown": 4,
    }.get(kind, 4)
    return (kind_rank, source)


def build_seed_backlog(
    repo_root: Path,
    *,
    max_items: int | None = None,
    source_mode: str = "codebase",
) -> dict[str, Any]:
    root = repo_root.resolve()
    paths = _discover_source_paths(root, source_mode=source_mode)
    all_todos = _extract_all_todos(paths, repo_root=root)

    normalized: list[dict[str, Any]] = []
    skipped = 0
    for todo in all_todos:
        try:
            normalized.append(normalize_issue(todo))
        except ValueError:
            skipped += 1
    deduped = dedupe(normalized)
    deduped.sort(
        key=lambda item: (
            item["priority"],
            str(item["metadata"].get("milestone", "")),
            item["title"],
        )
    )

    if max_items is None or max_items <= 0:
        selected = deduped
    else:
        selected = deduped[: max_items]

    return {
        "source_mode": source_mode,
        "todo_count": len(all_todos),
        "normalized_count": len(normalized),
        "deduped_count": len(deduped),
        "skipped_count": skipped,
        "selected": selected,
    }


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Generate first-pass Linear seed backlog from repository TODO contracts"
    )
    parser.add_argument(
        "--repo-root",
        default=".",
        help="Repository root (default: current directory)",
    )
    parser.add_argument(
        "--output",
        default="ops/linear/seed_backlog.json",
        help="Output JSON manifest path",
    )
    parser.add_argument(
        "--max-items",
        type=int,
        default=80,
        help="Maximum issues to emit",
    )
    parser.add_argument(
        "--source-mode",
        choices=SOURCE_MODES,
        default="codebase",
        help="Harvest mode: codebase only (default) or include doc seed files",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = Path(args.repo_root).resolve()
    backlog = build_seed_backlog(
        root,
        max_items=max(args.max_items, 1),
        source_mode=str(args.source_mode),
    )
    selected = backlog["selected"]

    output_path = (root / args.output).resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(selected, indent=2), encoding="utf-8")

    print(
        f"Wrote {len(selected)} seed issues to {output_path} "
        f"(skipped_non_actionable={backlog['skipped_count']}, "
        f"deduped={backlog['normalized_count'] - backlog['deduped_count']})"
    )
    return 0


def _extract_all_todos(
    paths: list[Path], *, repo_root: Path | None = None
) -> list[dict[str, Any]]:
    if not paths:
        return []

    if len(paths) == 1:
        return extract_todos(paths[0], repo_root=repo_root)

    max_workers = min(len(paths), max(os.cpu_count() or 1, 1))
    repo_root_text = str(repo_root) if repo_root is not None else None
    path_values = [(str(path), repo_root_text) for path in paths]

    if _has_interpreter_pool_executor():
        try:
            with futures.InterpreterPoolExecutor(max_workers=max_workers) as executor:  # type: ignore[attr-defined]
                rows = executor.map(_extract_todos_worker, path_values)
                return [item for chunk in rows for item in chunk]
        except Exception:
            pass

    with futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        rows = executor.map(_extract_todos_worker, path_values)
        return [item for chunk in rows for item in chunk]


def _has_interpreter_pool_executor() -> bool:
    return hasattr(futures, "InterpreterPoolExecutor")


def _discover_source_paths(repo_root: Path, *, source_mode: str = "codebase") -> list[Path]:
    if source_mode not in SOURCE_MODES:
        raise RuntimeError(
            f"unsupported source mode: {source_mode} (expected one of {', '.join(SOURCE_MODES)})"
        )
    paths: list[Path] = []
    if source_mode == "all":
        for rel in DOC_SOURCE_FILES:
            path = repo_root / rel
            if path.exists() and path.is_file():
                paths.append(path)

    for rel in SOURCE_ROOTS:
        root = repo_root / rel
        if not root.exists() or not root.is_dir():
            continue
        for path in root.rglob("*"):
            if not path.is_file():
                continue
            if path.suffix not in SOURCE_SUFFIXES:
                continue
            paths.append(path)

    deduped: list[Path] = []
    seen: set[str] = set()
    for path in sorted(paths):
        key = str(path.resolve())
        if key in seen:
            continue
        seen.add(key)
        deduped.append(path)
    return deduped


if __name__ == "__main__":
    raise SystemExit(main())
