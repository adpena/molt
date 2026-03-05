from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from datetime import UTC, datetime
from pathlib import Path


ALLOW_MARKER = "secret-guard: allow"
PRIVATE_KEY_RE = re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")
SENSITIVE_ASSIGN_RE = re.compile(
    r"(?i)\b(api[_-]?key|token|secret|password)\b\s*[:=]\s*([\"']?)([^\"'\s#]+)\2"
)
BEARER_TOKEN_RE = re.compile(r"(?i)\bbearer\s+([a-z0-9_\-]{24,})\b")
HIGH_CONFIDENCE_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("Linear API key", re.compile(r"\blin_api_[A-Za-z0-9]{20,}\b")),
    ("OpenAI-style key", re.compile(r"\bsk-[A-Za-z0-9]{20,}\b")),
    ("GitHub token", re.compile(r"\b(?:ghp|github_pat)_[A-Za-z0-9_]{20,}\b")),
    ("Slack token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
)
PLACEHOLDER_TOKENS = (
    "changeme",
    "replace",
    "example",
    "your",
    "placeholder",
    "dummy",
    "sample",
    "demo",
    "test",
    "abc123",
    "token",
    "secret",
    "none",
    "null",
)
ALLOW_PATH_PREFIXES = (
    "vendor/rustpython-parser/",
)


@dataclass(frozen=True, slots=True)
class AddedLine:
    path: str
    line_no: int
    text: str


@dataclass(frozen=True, slots=True)
class Finding:
    path: str
    line_no: int
    reason: str
    text: str


def _run(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(cmd, check=False, capture_output=True, text=True)


def _is_placeholder_value(value: str) -> bool:
    normalized = value.strip().strip("\"'").lower()
    if not normalized:
        return True
    if normalized.startswith(("$", "${", "<", ">", "{", "[")):
        return True
    if all(ch in {"x", "*", "-", "_", "."} for ch in normalized):
        return True
    return any(token in normalized for token in PLACEHOLDER_TOKENS)


def _looks_like_sensitive_assignment(line: str) -> bool:
    for match in SENSITIVE_ASSIGN_RE.finditer(line):
        value = match.group(3).strip()
        if len(value) < 20:
            continue
        if _is_placeholder_value(value):
            continue
        return True
    return False


def _scan_line(path: str, line_no: int, line: str) -> list[Finding]:
    if any(path.startswith(prefix) for prefix in ALLOW_PATH_PREFIXES):
        return []
    if ALLOW_MARKER in line:
        return []
    findings: list[Finding] = []
    if PRIVATE_KEY_RE.search(line):
        findings.append(
            Finding(
                path=path, line_no=line_no, reason="Private key material", text=line
            )
        )
    for reason, pattern in HIGH_CONFIDENCE_PATTERNS:
        if pattern.search(line):
            findings.append(
                Finding(path=path, line_no=line_no, reason=reason, text=line)
            )
    bearer_match = BEARER_TOKEN_RE.search(line)
    if bearer_match and not _is_placeholder_value(bearer_match.group(1)):
        findings.append(
            Finding(path=path, line_no=line_no, reason="Bearer token", text=line)
        )
    if _looks_like_sensitive_assignment(line):
        findings.append(
            Finding(
                path=path,
                line_no=line_no,
                reason="Sensitive assignment value",
                text=line,
            )
        )
    return findings


def iter_added_lines(diff_text: str) -> list[AddedLine]:
    lines: list[AddedLine] = []
    current_path: str | None = None
    current_new_line = 0
    hunk_re = re.compile(r"^@@ -\d+(?:,\d+)? \+(\d+)(?:,\d+)? @@")
    for raw in diff_text.splitlines():
        if raw.startswith("+++ "):
            if raw == "+++ /dev/null":
                current_path = None
            elif raw.startswith("+++ b/"):
                current_path = raw[6:]
            else:
                current_path = None
            continue
        if raw.startswith("@@ "):
            match = hunk_re.match(raw)
            current_new_line = int(match.group(1)) if match else 0
            continue
        if current_path is None or current_new_line <= 0:
            continue
        if raw.startswith("+") and not raw.startswith("+++"):
            lines.append(
                AddedLine(path=current_path, line_no=current_new_line, text=raw[1:])
            )
            current_new_line += 1
            continue
        if raw.startswith(" ") and not raw.startswith("+++"):
            current_new_line += 1
    return lines


def scan_diff_text(diff_text: str) -> list[Finding]:
    findings: list[Finding] = []
    for line in iter_added_lines(diff_text):
        findings.extend(_scan_line(line.path, line.line_no, line.text))
    unique: dict[tuple[str, int, str], Finding] = {}
    for finding in findings:
        key = (finding.path, finding.line_no, finding.reason)
        unique[key] = finding
    return list(unique.values())


def _staged_diff_text() -> str:
    proc = _run(["git", "diff", "--cached", "--no-color", "--unified=0"])
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "failed to read staged git diff")
    return proc.stdout


def _security_events_file() -> Path:
    configured = str(os.environ.get("MOLT_REMOVED_SECURITY_EVENTS_FILE") or "").strip()
    if configured:
        path = Path(configured).expanduser()
    else:
        ext_root = Path(
            str(os.environ.get("MOLT_EXT_ROOT") or "/Volumes/APDataStore/Molt")
        ).expanduser()
        path = ext_root / "logs" / "orchestration" / "security" / "events.jsonl"
    if not path.is_absolute():
        path = (Path.cwd() / path).resolve()
    return path


def _emit_security_event(*, kind: str, payload: dict[str, object]) -> None:
    event = {
        "at": datetime.now(UTC).isoformat().replace("+00:00", "Z"),
        "kind": kind,
        **payload,
    }
    path = _security_events_file()
    try:
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a", encoding="utf-8") as handle:
            handle.write(json.dumps(event, ensure_ascii=True) + "\n")
    except OSError:
        return


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Block commits that stage likely secret/token material."
    )
    parser.add_argument(
        "--staged",
        action="store_true",
        help="Scan staged changes from git diff --cached.",
    )
    parser.add_argument(
        "--diff-file",
        default=None,
        help="Optional diff file path for testing/debugging.",
    )
    return parser


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if not args.staged and not args.diff_file:
        raise RuntimeError("Specify --staged or --diff-file <path>.")
    if args.diff_file:
        with open(args.diff_file, "r", encoding="utf-8") as handle:
            diff_text = handle.read()
    else:
        diff_text = _staged_diff_text()
    findings = scan_diff_text(diff_text)
    if not findings:
        return 0
    _emit_security_event(
        kind="secret_guard_blocked",
        payload={
            "finding_count": len(findings),
            "paths": sorted({finding.path for finding in findings})[:32],
        },
    )
    print(
        "secret-guard blocked commit: detected likely secret material in staged additions.",
        file=sys.stderr,
    )
    print(
        "Remove or rotate these values before commit. For safe test fixtures, append "
        "'# secret-guard: allow' on that exact line.",
        file=sys.stderr,
    )
    for finding in findings:
        snippet = finding.text.strip()
        if len(snippet) > 180:
            snippet = snippet[:177] + "..."
        print(
            f"  - {finding.path}:{finding.line_no} [{finding.reason}] {snippet}",
            file=sys.stderr,
        )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
