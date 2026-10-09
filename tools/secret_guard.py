from __future__ import annotations

import argparse
from collections.abc import Iterator
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
ASSIGNMENT_HEAD_RE = re.compile(
    r"(?<![\w-])(?P<quote>[\"']?)(?P<name>[A-Za-z_][A-Za-z0-9_-]*)"
    r"(?P=quote)\s*(?:=(?!=)|:(?![:=]))\s*"
)
QUOTED_VALUE_RE = re.compile(r"(?i:(?:br|rb|r|u|b))?(?P<quote>[\"'])")
BARE_VALUE_RE = re.compile(r"""[^\s#"']+""")
ENV_REFERENCE_RE = re.compile(
    r"\$(?:[A-Za-z_][A-Za-z0-9_]*|\{[A-Za-z_][A-Za-z0-9_]*\})"
)
# RFC 6750 section 2.1 b64token alphabet; the existing length policy is below.
BEARER_TOKEN_RE = re.compile(r"(?i)\bbearer\s+([a-z0-9_.~+/\-]+=*)")
HIGH_CONFIDENCE_PATTERNS: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("Linear API key", re.compile(r"\blin_api_[A-Za-z0-9]{20,}\b")),
    ("OpenAI-style key", re.compile(r"\bsk-[A-Za-z0-9]{20,}\b")),
    ("GitHub token", re.compile(r"\b(?:ghp|github_pat)_[A-Za-z0-9_]{20,}\b")),
    ("Slack token", re.compile(r"\bxox[baprs]-[A-Za-z0-9-]{20,}\b")),
)
PLACEHOLDER_VALUES = frozenset(
    {
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
        "your-token",
        "your-api-key",
        "your-password",
        "your-secret",
        "replace-with-token",
        "replace-with-api-key",
        "replace-with-password",
        "replace-with-secret",
    }
)
# These languages distinguish a quoted literal from an unquoted expression.
# Other paths keep conservative bare-data scanning: an unknown file extension
# must not become a new exemption. This line scanner does not evaluate programs,
# templates, escaped runtime values, or multiline literals. Provider and
# private-key patterns inspect every added line independently of this policy.
SOURCE_SUFFIXES = frozenset(
    {
        ".py",
        ".pyi",
        ".rs",
        ".js",
        ".jsx",
        ".ts",
        ".tsx",
        ".c",
        ".h",
        ".cc",
        ".hh",
        ".cpp",
        ".hpp",
        ".cxx",
        ".hxx",
        ".go",
        ".java",
        ".kt",
        ".swift",
        ".cs",
        ".rb",
        ".lua",
        ".luau",
        ".lean",
        ".zig",
    }
)
# These generators publish public identifier maps, including inverse maps.
# Admit only complete entries whose value repeats the key with at most the fixed
# runtime namespace. Other literals in these files retain normal scanning.
PUBLIC_SYMBOL_MAP_PATHS = frozenset(
    {
        "src/molt/_intrinsic_symbols.py",
        "src/molt/_wasm_abi_generated.py",
        "wasm/wasm_abi_generated.json",
    }
)
PUBLIC_SYMBOL_MAP_ENTRY_RE = re.compile(
    r'\s*"(?P<key>[A-Za-z_][A-Za-z0-9_]*)"\s*:\s*'
    r'"(?P<value>[A-Za-z_][A-Za-z0-9_]*)",?\s*'
)
ALLOW_PATH_PREFIXES = ("vendor/rustpython-parser/",)


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


def _run(cmd: list[str]) -> subprocess.CompletedProcess[str]:
    # Git counts LF-delimited lines. Text-mode universal newline conversion
    # would turn a CR within an added line into an unmarked protocol line.
    proc = subprocess.run(cmd, check=False, capture_output=True)
    return subprocess.CompletedProcess(
        proc.args,
        proc.returncode,
        stdout=proc.stdout.decode("utf-8"),
        stderr=proc.stderr.decode("utf-8"),
    )


def _is_placeholder_value(value: str) -> bool:
    normalized = value.strip().casefold()
    if not normalized:
        return True
    if normalized.startswith("<") and normalized.endswith(">"):
        normalized = normalized[1:-1]
    if all(ch in {"x", "*", "-", "_", "."} for ch in normalized):
        return True
    return normalized.replace("_", "-") in PLACEHOLDER_VALUES


def _is_sensitive_name(name: str) -> bool:
    # The assignment grammar admits ASCII names. Padding retains whole
    # underscore/hyphen components without per-component objects or generators.
    components = "_" + name.lower().replace("-", "_") + "_"
    return (
        "_apikey_" in components
        or "_token_" in components
        or "_secret_" in components
        or "_password_" in components
        or "_api_key_" in components
    )


def _allows_bare_data(path: str) -> bool:
    return Path(path).suffix.casefold() not in SOURCE_SUFFIXES


def _quoted_value(line: str, start: int) -> str | None:
    match = QUOTED_VALUE_RE.match(line, start)
    if match is None:
        return None
    quote = match.group("quote")
    quote_start = match.end() - 1
    delimiter = quote * (3 if line.startswith(quote * 3, quote_start) else 1)
    value_start = quote_start + len(delimiter)
    index = value_start
    while index < len(line):
        if line[index] == "\\":
            index += 2
        elif line.startswith(delimiter, index):
            return line[value_start:index]
        else:
            index += 1
    return None


def _assignment_values(path: str, line: str) -> Iterator[str]:
    for match in ASSIGNMENT_HEAD_RE.finditer(line):
        if not _is_sensitive_name(match.group("name")):
            continue
        value = _quoted_value(line, match.end())
        if value is not None:
            if path in PUBLIC_SYMBOL_MAP_PATHS:
                entry = PUBLIC_SYMBOL_MAP_ENTRY_RE.fullmatch(line)
                if entry is not None:
                    key = entry.group("key")
                    if value == key or value == "molt_" + key or key == "molt_" + value:
                        continue
            yield value
        elif _allows_bare_data(path):
            bare = BARE_VALUE_RE.match(line, match.end())
            if bare is not None and ENV_REFERENCE_RE.fullmatch(bare[0]) is None:
                yield bare[0]


def _looks_like_sensitive_assignment(path: str, line: str) -> bool:
    for value in _assignment_values(path, line):
        if len(value) < 20:
            continue
        if _is_placeholder_value(value):
            continue
        return True
    return False


def _scan_line(path: str, line_no: int, line: str) -> list[Finding]:
    if path.startswith(ALLOW_PATH_PREFIXES):
        return []
    if ALLOW_MARKER in line:
        return []
    findings: list[Finding] = []
    if PRIVATE_KEY_RE.search(line):
        findings.append(
            Finding(path=path, line_no=line_no, reason="Private key material")
        )
    for reason, pattern in HIGH_CONFIDENCE_PATTERNS:
        if pattern.search(line):
            findings.append(Finding(path=path, line_no=line_no, reason=reason))
    for match in BEARER_TOKEN_RE.finditer(line):
        if len(match.group(1)) >= 24 and not _is_placeholder_value(match.group(1)):
            findings.append(Finding(path=path, line_no=line_no, reason="Bearer token"))
            break
    if _looks_like_sensitive_assignment(path, line):
        findings.append(
            Finding(
                path=path,
                line_no=line_no,
                reason="Sensitive assignment value",
            )
        )
    return findings


def _new_file_path(header: str) -> str | None:
    if header == "/dev/null":
        return None
    if header.startswith('"'):
        # Git quotes path bytes with C escapes, including three-digit octal
        # escapes for non-ASCII bytes. JSON string decoding is not this format.
        if not header.endswith('"'):
            raise ValueError("unterminated quoted new-file path")
        encoded = bytearray()
        escapes = {
            "a": 7,
            "b": 8,
            "t": 9,
            "n": 10,
            "v": 11,
            "f": 12,
            "r": 13,
            "\\": 92,
            '"': 34,
        }
        index = 1
        while index < len(header) - 1:
            char = header[index]
            if char != "\\":
                encoded.extend(char.encode("utf-8"))
                index += 1
                continue
            index += 1
            if index == len(header) - 1:
                raise ValueError("incomplete quoted new-file path escape")
            char = header[index]
            if char in escapes:
                encoded.append(escapes[char])
                index += 1
            elif re.fullmatch(r"[0-3][0-7]{2}", header[index : index + 3]):
                encoded.append(int(header[index : index + 3], 8))
                index += 3
            else:
                raise ValueError("unsupported quoted new-file path escape")
        header = os.fsdecode(bytes(encoded))
    if not header.startswith("b/"):
        raise ValueError("new-file path must use the standard b/ prefix")
    return header[2:]


def iter_added_lines(diff_text: str) -> list[AddedLine]:
    lines: list[AddedLine] = []
    current_path: str | None = None
    current_new_line = 0
    old_remaining = new_remaining = 0
    hunk_seen = False
    hunk_re = re.compile(r"^@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")
    # The diff protocol uses LF. Unicode separators and CR are payload bytes.
    for raw in diff_text.removesuffix("\n").split("\n"):
        # Hunk contents own their leading marker. An added line whose content
        # starts with "++ " is not a new-file header.
        if old_remaining or new_remaining:
            if raw.startswith("+") and new_remaining:
                if current_path is None:
                    raise ValueError("added line has no new-file path")
                lines.append(AddedLine(current_path, current_new_line, raw[1:]))
                current_new_line += 1
                new_remaining -= 1
            elif raw.startswith("-") and old_remaining:
                old_remaining -= 1
            elif raw.startswith(" ") and old_remaining and new_remaining:
                current_new_line += 1
                old_remaining -= 1
                new_remaining -= 1
            elif raw != "\\ No newline at end of file":
                raise ValueError("malformed unified diff hunk")
            continue
        if raw.startswith("diff --git "):
            current_path = None
            hunk_seen = False
            continue
        if raw.startswith("@@"):
            match = hunk_re.match(raw)
            if match is None:
                raise ValueError("malformed unified diff hunk header")
            old_remaining = int(match.group(1) or 1)
            current_new_line = int(match.group(2))
            new_remaining = int(match.group(3) or 1)
            hunk_seen = True
            continue
        if hunk_seen and raw.startswith(("+", "-", " ")):
            raise ValueError("payload exceeds declared unified diff hunk")
        if raw.startswith("+++ "):
            current_path = _new_file_path(raw[4:])
            continue
        if raw.startswith("--- "):
            continue
        if raw.startswith(("+", "-", " ")):
            raise ValueError("payload outside unified diff hunk")
    if old_remaining or new_remaining:
        raise ValueError("incomplete unified diff hunk")
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
    proc = _run(
        [
            "git",
            "diff",
            "--cached",
            "--no-color",
            "--unified=0",
            "--no-ext-diff",
            "--no-textconv",
            "--no-relative",
            "--src-prefix=a/",
            "--dst-prefix=b/",
        ]
    )
    if proc.returncode != 0:
        raise RuntimeError(proc.stderr.strip() or "failed to read staged git diff")
    return proc.stdout


def _security_events_file() -> Path:
    configured = str(os.environ.get("MOLT_SECURITY_EVENTS_FILE") or "").strip()
    if configured:
        path = Path(configured).expanduser()
    else:
        path = Path("logs") / "security" / "events.jsonl"
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
        with open(args.diff_file, "r", encoding="utf-8", newline="") as handle:
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
        location = json.dumps(finding.path, ensure_ascii=True)
        print(
            f"  - {location}:{finding.line_no} [{finding.reason}]",
            file=sys.stderr,
        )
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
