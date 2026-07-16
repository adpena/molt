#!/usr/bin/env python3
"""Validate Molt's compact, single-authority agent instruction hierarchy."""

from __future__ import annotations

import argparse
import json
import re
from dataclasses import dataclass, field
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
ROOT_AGENT_MAX_BYTES = 16 * 1024
ROOT_AGENT_MAX_LINES = 200
CLAUDE_IMPORT = "@AGENTS.md\n"
ARCHIVES = (
    "docs/agent/AGENTS.full.md",
    "docs/agent/CLAUDE.full.md",
)
ARCHIVE_MARKER = "<!-- INSTRUCTION_ARCHIVE: non-normative -->"

_CODE_SPAN_RE = re.compile(r"`([^`\r\n]+)`")
_REPO_POINTER_RE = re.compile(
    r"^(?:docs|runtime|src|tools|tests|config)/[^\s`]+"
    r"(?:\.md|\.toml|\.py|\.pyi|\.rs|\.json|\.yaml|\.yml)$"
)
_MACHINE_STATE_PATTERNS = (
    ("absolute Windows path", re.compile(r"(?i)(?<![A-Za-z0-9])[A-Z]:[\\/]")),
    ("UNC or extended Windows path", re.compile(r"\\\\(?:\?|\.|[^\\\s]+)\\")),
    ("absolute user or temporary path", re.compile(r"(?i)(?<!\w)/(?:users|home|tmp|var/tmp)/")),
    ("OneDrive workstation path", re.compile(r"(?i)\bonedrive\b")),
    ("concrete process id", re.compile(r"(?i)\b(?:pid|process[ -]?id)\s*(?:=|:|is)\s*\d+\b")),
)


@dataclass(frozen=True)
class AuditResult:
    failures: tuple[str, ...] = field(default_factory=tuple)

    @property
    def ok(self) -> bool:
        return not self.failures

    def as_dict(self) -> dict[str, object]:
        return {"ok": self.ok, "failures": list(self.failures)}


def _read(path: Path, failures: list[str]) -> str:
    try:
        return path.read_text(encoding="utf-8")
    except FileNotFoundError:
        failures.append(f"{path.name}: missing")
    except UnicodeDecodeError as exc:
        failures.append(f"{path.name}: not UTF-8: {exc}")
    return ""


def _root_pointers(text: str) -> tuple[str, ...]:
    return tuple(
        pointer
        for pointer in _CODE_SPAN_RE.findall(text)
        if _REPO_POINTER_RE.fullmatch(pointer)
    )


def audit(root: Path = ROOT) -> AuditResult:
    """Audit the repository instruction hierarchy without importing project code."""

    failures: list[str] = []
    agents_path = root / "AGENTS.md"
    agents_text = _read(agents_path, failures)
    if agents_text:
        byte_count = len(agents_text.encode("utf-8"))
        line_count = len(agents_text.splitlines())
        if byte_count > ROOT_AGENT_MAX_BYTES:
            failures.append(
                f"AGENTS.md: {byte_count} bytes exceeds {ROOT_AGENT_MAX_BYTES}-byte budget"
            )
        if line_count > ROOT_AGENT_MAX_LINES:
            failures.append(
                f"AGENTS.md: {line_count} lines exceeds {ROOT_AGENT_MAX_LINES}-line budget"
            )
        for label, pattern in _MACHINE_STATE_PATTERNS:
            if match := pattern.search(agents_text):
                failures.append(
                    f"AGENTS.md: contains prohibited {label}: {match.group(0)!r}"
                )
        pointers = _root_pointers(agents_text)
        if not pointers:
            failures.append("AGENTS.md: contains no repository instruction pointers")
        for pointer in pointers:
            if not (root / pointer).is_file():
                failures.append(f"AGENTS.md: referenced pointer does not exist: {pointer}")

    claude_text = _read(root / "CLAUDE.md", failures)
    if claude_text and claude_text != CLAUDE_IMPORT:
        failures.append("CLAUDE.md: must contain exactly '@AGENTS.md' and one newline")

    for relative in ARCHIVES:
        archive_path = root / relative
        archive_text = _read(archive_path, failures)
        if archive_text and not archive_text.startswith(ARCHIVE_MARKER + "\n"):
            failures.append(
                f"{relative}: must start with {ARCHIVE_MARKER!r}"
            )

    return AuditResult(tuple(failures))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--json", action="store_true", help="emit machine-readable output")
    args = parser.parse_args(argv)
    result = audit()
    if args.json:
        print(json.dumps(result.as_dict(), indent=2))
    elif result.ok:
        print("PASS: compact single-authority instruction hierarchy")
    else:
        print("FAIL: instruction hierarchy")
        for failure in result.failures:
            print(f"  - {failure}")
    return 0 if result.ok else 2


if __name__ == "__main__":
    raise SystemExit(main())
