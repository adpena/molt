#!/usr/bin/env python3
"""Audit a dirty-tree replay before pushing it to main.

This tool answers one narrow question that is otherwise easy to discover too
late in a manual merge:

    Did every owned dirty path from the source checkout appear in the landed
    commit range?

It deliberately audits path coverage, not byte equality. Conflict resolution,
mainline drift, and generated authority can make the landed content differ from
the dirty source while still preserving the signal. Missing paths are the hard
failure: they mean a source hunk, new file, or deletion did not make it into the
range being pushed.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Sequence


REPO_ROOT = Path(__file__).resolve().parents[1]


class LandingAuditError(RuntimeError):
    pass


@dataclass(frozen=True)
class LandingAuditReport:
    source_root: str
    landed_root: str
    base_ref: str
    head_ref: str
    owned: tuple[str, ...]
    ignored: tuple[str, ...]
    source_dirty_paths: tuple[str, ...]
    landed_range_paths: tuple[str, ...]
    source_only: tuple[str, ...]
    landed_only: tuple[str, ...]
    ok: bool

    def to_json(self) -> dict[str, object]:
        return {
            "ok": self.ok,
            "source_root": self.source_root,
            "landed_root": self.landed_root,
            "base_ref": self.base_ref,
            "head_ref": self.head_ref,
            "owned": list(self.owned),
            "ignored": list(self.ignored),
            "counts": {
                "source_dirty_paths": len(self.source_dirty_paths),
                "landed_range_paths": len(self.landed_range_paths),
                "source_only": len(self.source_only),
                "landed_only": len(self.landed_only),
            },
            "source_dirty_paths": list(self.source_dirty_paths),
            "landed_range_paths": list(self.landed_range_paths),
            "source_only": list(self.source_only),
            "landed_only": list(self.landed_only),
        }


def _run_git(root: Path, args: Sequence[str]) -> str:
    proc = subprocess.run(
        ["git", *args],
        cwd=str(root),
        capture_output=True,
        text=True,
        check=False,
    )
    if proc.returncode != 0:
        detail = proc.stderr.strip() or proc.stdout.strip()
        raise LandingAuditError(
            f"git {' '.join(args)} failed in {root} (rc={proc.returncode}): {detail}"
        )
    return proc.stdout


def _normalize_path(path: str) -> str:
    return path.replace("\\", "/").strip("/")


def _git_lines(root: Path, args: Sequence[str]) -> list[str]:
    return [
        _normalize_path(line)
        for line in _run_git(root, args).splitlines()
        if line.strip()
    ]


def dirty_source_paths(root: Path) -> tuple[str, ...]:
    tracked: set[str] = set()
    for line in _run_git(root, ["diff", "--name-status", "--no-renames"]).splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        if len(parts) < 2:
            raise LandingAuditError(f"unexpected git diff --name-status line: {line!r}")
        tracked.add(_normalize_path(parts[-1]))

    untracked = set(
        _git_lines(root, ["ls-files", "--others", "--exclude-standard"])
    )
    return tuple(sorted(tracked | untracked))


def landed_range_paths(root: Path, base_ref: str, head_ref: str) -> tuple[str, ...]:
    paths = _git_lines(
        root,
        ["diff", "--name-only", "--no-renames", f"{base_ref}..{head_ref}"],
    )
    return tuple(sorted(set(paths)))


def _path_matches(pattern: str, path: str) -> bool:
    pattern = _normalize_path(pattern)
    path = _normalize_path(path)
    if not pattern:
        return False
    if any(char in pattern for char in "*?[]"):
        return fnmatch.fnmatchcase(path, pattern)
    if pattern.endswith("/"):
        return path.startswith(pattern)
    return path == pattern or path.startswith(f"{pattern}/")


def filter_paths(
    paths: Sequence[str],
    *,
    owned: Sequence[str] = (),
    ignored: Sequence[str] = (),
) -> tuple[str, ...]:
    normalized = [_normalize_path(path) for path in paths]
    if owned:
        normalized = [
            path
            for path in normalized
            if any(_path_matches(pattern, path) for pattern in owned)
        ]
    if ignored:
        normalized = [
            path
            for path in normalized
            if not any(_path_matches(pattern, path) for pattern in ignored)
        ]
    return tuple(sorted(set(normalized)))


def build_report(
    *,
    source_root: Path,
    landed_root: Path,
    base_ref: str,
    head_ref: str,
    source_paths: Sequence[str],
    landed_paths: Sequence[str],
    owned: Sequence[str] = (),
    ignored: Sequence[str] = (),
    fail_on_landed_only: bool = False,
) -> LandingAuditReport:
    filtered_source = filter_paths(source_paths, owned=owned, ignored=ignored)
    filtered_landed = filter_paths(landed_paths, owned=owned, ignored=ignored)
    source_set = set(filtered_source)
    landed_set = set(filtered_landed)
    source_only = tuple(sorted(source_set - landed_set))
    landed_only = tuple(sorted(landed_set - source_set))
    ok = not source_only and (not landed_only if fail_on_landed_only else True)
    return LandingAuditReport(
        source_root=str(source_root),
        landed_root=str(landed_root),
        base_ref=base_ref,
        head_ref=head_ref,
        owned=tuple(_normalize_path(pattern) for pattern in owned),
        ignored=tuple(_normalize_path(pattern) for pattern in ignored),
        source_dirty_paths=filtered_source,
        landed_range_paths=filtered_landed,
        source_only=source_only,
        landed_only=landed_only,
        ok=ok,
    )


def audit_dirty_landing(
    *,
    source_root: Path,
    landed_root: Path,
    base_ref: str,
    head_ref: str,
    owned: Sequence[str] = (),
    ignored: Sequence[str] = (),
    fail_on_landed_only: bool = False,
) -> LandingAuditReport:
    source_paths = dirty_source_paths(source_root)
    landed_paths = landed_range_paths(landed_root, base_ref, head_ref)
    return build_report(
        source_root=source_root,
        landed_root=landed_root,
        base_ref=base_ref,
        head_ref=head_ref,
        source_paths=source_paths,
        landed_paths=landed_paths,
        owned=owned,
        ignored=ignored,
        fail_on_landed_only=fail_on_landed_only,
    )


def render_text(report: LandingAuditReport) -> str:
    status = "PASS" if report.ok else "FAIL"
    lines = [
        f"dirty landing audit: {status}",
        f"source root: {report.source_root}",
        f"landed root: {report.landed_root}",
        f"range: {report.base_ref}..{report.head_ref}",
        f"source dirty paths: {len(report.source_dirty_paths)}",
        f"landed range paths: {len(report.landed_range_paths)}",
        f"source-only paths: {len(report.source_only)}",
        f"landed-only paths: {len(report.landed_only)}",
    ]
    if report.owned:
        lines.append("owned filters:")
        lines.extend(f"  {pattern}" for pattern in report.owned)
    if report.ignored:
        lines.append("ignored filters:")
        lines.extend(f"  {pattern}" for pattern in report.ignored)
    if report.source_only:
        lines.append("source-only:")
        lines.extend(f"  {path}" for path in report.source_only)
    if report.landed_only:
        lines.append("landed-only:")
        lines.extend(f"  {path}" for path in report.landed_only)
    return "\n".join(lines) + "\n"


def parse_args(argv: Sequence[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description="Audit dirty-source path coverage in a landed commit range.",
    )
    parser.add_argument(
        "--source-root",
        type=Path,
        required=True,
        help="Dirty checkout whose tracked/untracked paths are the replay source.",
    )
    parser.add_argument(
        "--landed-root",
        type=Path,
        default=REPO_ROOT,
        help="Checkout containing the landed range (default: this repo).",
    )
    parser.add_argument(
        "--base-ref",
        required=True,
        help="Base ref for the landed range, for example origin/main.",
    )
    parser.add_argument(
        "--head-ref",
        default="HEAD",
        help="Head ref for the landed range (default: HEAD).",
    )
    parser.add_argument(
        "--owned",
        action="append",
        default=[],
        help="Only audit matching path prefixes/globs; repeatable.",
    )
    parser.add_argument(
        "--ignore",
        action="append",
        default=[],
        help="Exclude matching path prefixes/globs from both sides; repeatable.",
    )
    parser.add_argument(
        "--fail-on-landed-only",
        action="store_true",
        help="Also fail when the landed range changed paths outside the source set.",
    )
    parser.add_argument("--json", action="store_true", help="Emit JSON to stdout.")
    parser.add_argument(
        "--json-output",
        type=Path,
        help="Write the JSON report to this path in addition to stdout.",
    )
    return parser.parse_args(argv)


def main(argv: Sequence[str] | None = None) -> int:
    args = parse_args(argv)
    try:
        report = audit_dirty_landing(
            source_root=args.source_root.resolve(),
            landed_root=args.landed_root.resolve(),
            base_ref=args.base_ref,
            head_ref=args.head_ref,
            owned=args.owned,
            ignored=args.ignore,
            fail_on_landed_only=args.fail_on_landed_only,
        )
    except LandingAuditError as exc:
        print(f"dirty landing audit: ERROR: {exc}", file=sys.stderr)
        return 2

    payload = report.to_json()
    if args.json_output is not None:
        args.json_output.parent.mkdir(parents=True, exist_ok=True)
        args.json_output.write_text(
            json.dumps(payload, indent=2, sort_keys=True) + "\n",
            encoding="utf-8",
        )
    if args.json:
        print(json.dumps(payload, indent=2, sort_keys=True))
    else:
        print(render_text(report), end="")
    return 0 if report.ok else 1


if __name__ == "__main__":
    raise SystemExit(main())
