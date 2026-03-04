from __future__ import annotations

import argparse
from concurrent import futures
import json
import os
import re
from pathlib import Path
from typing import Any


TODO_RE = re.compile(
    r"^\s*(?:[-*]\s+)?TODO\((?P<area>[^,]+),\s*owner:(?P<owner>[^,]+),\s*milestone:(?P<milestone>[^,]+),\s*priority:(?P<priority>P[0-3]),\s*status:(?P<status>[^)]+)\):\s*(?P<body>.+?)\s*$"
)

SOURCE_FILES = [
    "ROADMAP.md",
    "docs/spec/STATUS.md",
    "OPTIMIZATIONS_PLAN.md",
    "AGENTS.md",
]

PRIORITY_TO_LINEAR = {
    "P0": 1,
    "P1": 2,
    "P2": 3,
    "P3": 4,
}

MIN_BODY_CHARS = 20
IGNORED_BODY_SNIPPETS = (
    "test fixture",
    "example",
    "todo contracts",
)


def extract_todos(path: Path) -> list[dict[str, Any]]:
    todos: list[dict[str, Any]] = []
    for lineno, line in enumerate(
        path.read_text(encoding="utf-8").splitlines(), start=1
    ):
        match = TODO_RE.search(line)
        if not match:
            continue
        entry = {k: v.strip() for k, v in match.groupdict().items()}
        entry["source_file"] = str(path)
        entry["source_line"] = lineno
        todos.append(entry)
    return todos


def _extract_todos_worker(path_text: str) -> list[dict[str, Any]]:
    return extract_todos(Path(path_text))


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
        "Auto-seeded from Molt roadmap/status TODO contracts.\n\n"
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
    seen: set[str] = set()
    result: list[dict[str, Any]] = []
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
        if key in seen:
            continue
        seen.add(key)
        result.append(item)
    return result


def _sanitize_body(raw: str) -> str:
    body = raw.replace("\\n", " ").replace("\n", " ").strip()
    body = re.sub(r"\s+", " ", body)
    body = re.sub(r"\[([^\]]+)\]\([^)]+\)", r"\1", body)
    body = body.strip("`'\"")
    body = body.rstrip(".,;: ")
    body = body.replace(").", ")")
    body = body.replace('",', "")
    body = body.replace('."', "")
    return body.strip()


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
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    root = Path(args.repo_root).resolve()

    paths = [root / rel for rel in SOURCE_FILES if (root / rel).exists()]
    all_todos = _extract_all_todos(paths)

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
    selected = deduped[: max(args.max_items, 1)]

    output_path = (root / args.output).resolve()
    output_path.parent.mkdir(parents=True, exist_ok=True)
    output_path.write_text(json.dumps(selected, indent=2), encoding="utf-8")

    print(
        f"Wrote {len(selected)} seed issues to {output_path} "
        f"(skipped_non_actionable={skipped}, deduped={len(normalized) - len(deduped)})"
    )
    return 0


def _extract_all_todos(paths: list[Path]) -> list[dict[str, Any]]:
    if not paths:
        return []

    if len(paths) == 1:
        return extract_todos(paths[0])

    max_workers = min(len(paths), max(os.cpu_count() or 1, 1))
    path_values = [str(path) for path in paths]

    if _has_interpreter_pool_executor():
        with futures.InterpreterPoolExecutor(max_workers=max_workers) as executor:  # type: ignore[attr-defined]
            rows = executor.map(_extract_todos_worker, path_values)
            return [item for chunk in rows for item in chunk]

    with futures.ThreadPoolExecutor(max_workers=max_workers) as executor:
        rows = executor.map(_extract_todos_worker, path_values)
        return [item for chunk in rows for item in chunk]


def _has_interpreter_pool_executor() -> bool:
    return hasattr(futures, "InterpreterPoolExecutor")


if __name__ == "__main__":
    raise SystemExit(main())
