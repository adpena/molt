"""Static analysis tests for nondeterminism sources in the Molt compiler.

Scans compiler source code for patterns that could introduce nondeterminism:
- Entropy sources: random.*, os.urandom, uuid.*
- Timestamp leakage: time.time(), datetime.now()
- Unsafe iteration: bare dict/set iteration used for output ordering
- id()-based ordering decisions
"""

from __future__ import annotations

import re
from pathlib import Path

import pytest

ROOT = Path(__file__).resolve().parents[2]
FRONTEND_INIT = ROOT / "src" / "molt" / "frontend" / "__init__.py"
SRC_DIR = ROOT / "src" / "molt"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _compiler_source_files() -> list[Path]:
    """Return all Python source files under src/molt/ that are part of the compiler."""
    if not SRC_DIR.is_dir():
        return []
    return sorted(SRC_DIR.rglob("*.py"))


def _read_lines(path: Path) -> list[tuple[int, str]]:
    """Return (1-based line number, line text) pairs, skipping comments."""
    lines = []
    for i, raw in enumerate(path.read_text().splitlines(), 1):
        stripped = raw.strip()
        # Skip pure comment lines and blank lines
        if stripped.startswith("#") or not stripped:
            continue
        lines.append((i, raw))
    return lines


def _find_pattern_in_file(
    path: Path,
    pattern: re.Pattern[str],
    *,
    exclude_patterns: list[re.Pattern[str]] | None = None,
) -> list[tuple[int, str]]:
    """Find lines matching *pattern*, excluding lines matching any exclude pattern."""
    exclude_patterns = exclude_patterns or []
    findings: list[tuple[int, str]] = []
    for lineno, line in _read_lines(path):
        if pattern.search(line):
            # Check exclusions
            if any(ep.search(line) for ep in exclude_patterns):
                continue
            findings.append((lineno, line.strip()))
    return findings


# ---------------------------------------------------------------------------
# Patterns
# ---------------------------------------------------------------------------

# Entropy sources that should never appear in compiler code paths
_ENTROPY_PATTERN = re.compile(
    r"""
    \brandom\.\w+\(           # random.choice(), random.randint(), etc.
    | \bos\.urandom\(         # os.urandom()
    | \buuid\.\w+\(           # uuid.uuid4(), etc.
    | \bsecrets\.\w+\(        # secrets module
    """,
    re.VERBOSE,
)

# Timestamp patterns that could leak into build output
_TIMESTAMP_PATTERN = re.compile(
    r"""
    \btime\.time\(\)          # time.time()
    | \btime\.monotonic\(\)   # time.monotonic() -- OK for perf but not for output
    | \bdatetime\.now\(       # datetime.now()
    | \bdatetime\.utcnow\(   # datetime.utcnow()
    | \bdate\.today\(        # date.today()
    """,
    re.VERBOSE,
)

# Exclude lines that are clearly in logging/debug/stats contexts (not output-affecting)
_TIMESTAMP_EXCLUDES = [
    re.compile(r"\b(?:log|debug|warn|info|perf|stats|diag|timing|elapsed)\b", re.I),
    re.compile(r"#.*(?:timing|perf|debug|stats)", re.I),
    re.compile(r"_(?:timer|elapsed|perf|stats|duration|start_time|end_time)\b"),
    re.compile(r"\bmonotonic\b"),  # monotonic is fine, used for elapsed time
]

# id() used in ordering (e.g., sorted(things, key=id) or comparisons)
_ID_ORDERING_PATTERN = re.compile(
    r"""
    \bsorted\([^)]*key\s*=\s*id\b   # sorted(..., key=id)
    | \.sort\([^)]*key\s*=\s*id\b   # list.sort(key=id)
    | \bid\(\w+\)\s*[<>]            # id(x) < id(y) comparisons
    | [<>]\s*id\(\w+\)              # ... > id(y)
    """,
    re.VERBOSE,
)


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestEntropySourceAudit:
    """Verify no entropy sources exist in compiler code paths."""

    def test_no_entropy_in_frontend(self) -> None:
        """Frontend compiler must not use random/uuid/urandom."""
        if not FRONTEND_INIT.exists():
            pytest.skip("Frontend __init__.py not found")

        findings = _find_pattern_in_file(FRONTEND_INIT, _ENTROPY_PATTERN)
        assert not findings, (
            f"Entropy sources found in {FRONTEND_INIT.name}:\n"
            + "\n".join(f"  line {n}: {t}" for n, t in findings)
        )

    def test_no_entropy_in_compiler_sources(self) -> None:
        """No compiler source file should use entropy sources in codegen paths.

        We exclude cli.py and other non-codegen modules where uuid/random usage
        is legitimate (e.g., temp file naming, upload IDs).
        """
        # Files where entropy usage is acceptable (not in codegen paths)
        _ENTROPY_ALLOWLIST = {
            "src/molt/cli.py",      # temp files, upload IDs
            "src/molt/net.py",      # network request IDs
            "src/molt/asgi.py",     # request handling
            "src/molt/gpu/generate.py",  # sampling for text generation
        }

        all_findings: list[tuple[str, int, str]] = []
        for src in _compiler_source_files():
            rel = src.relative_to(ROOT)
            rel_str = str(rel)
            if "test" in rel_str.lower():
                continue
            if rel_str in _ENTROPY_ALLOWLIST:
                continue
            findings = _find_pattern_in_file(src, _ENTROPY_PATTERN)
            for lineno, text in findings:
                all_findings.append((rel_str, lineno, text))

        assert not all_findings, (
            "Entropy sources found in compiler code:\n"
            + "\n".join(f"  {f}:{n}: {t}" for f, n, t in all_findings)
        )


class TestTimestampLeakage:
    """Verify no timestamps leak into compiler output."""

    def test_no_output_timestamps_in_frontend(self) -> None:
        """Frontend must not embed timestamps into IR output.

        We allow time.monotonic() for performance measurement since it doesn't
        leak into output.
        """
        if not FRONTEND_INIT.exists():
            pytest.skip("Frontend __init__.py not found")

        findings = _find_pattern_in_file(
            FRONTEND_INIT,
            _TIMESTAMP_PATTERN,
            exclude_patterns=_TIMESTAMP_EXCLUDES,
        )
        assert not findings, (
            f"Timestamp usage found in {FRONTEND_INIT.name} "
            f"(may leak into output):\n"
            + "\n".join(f"  line {n}: {t}" for n, t in findings)
        )


class TestDictSetIterationSafety:
    """Verify that dict/set iteration in the frontend doesn't leak ordering."""

    def test_no_unsafe_dict_iteration_for_output(self) -> None:
        """Check that dict iteration in to_json/serialize/dump methods uses sorted().

        We look for patterns like `for k in self.<dict_attr>` in the frontend
        that are NOT wrapped in sorted(). Only flags methods that directly
        produce the final output structure (to_json, serialize, dump), not
        general emit helpers where dict insertion order is deterministic in
        Python 3.7+.
        """
        if not FRONTEND_INIT.exists():
            pytest.skip("Frontend __init__.py not found")

        content = FRONTEND_INIT.read_text()
        lines = content.splitlines()

        # Pattern: for <var> in self.<something>.items() or self.<something>
        # without sorted() wrapping
        dict_iter_pattern = re.compile(
            r"for\s+\w+(?:\s*,\s*\w+)?\s+in\s+self\.\w+(?:\.items\(\)|\.keys\(\)|\.values\(\)|\b)"
        )
        sorted_wrapper = re.compile(r"\bsorted\(")

        # Only flag truly output-producing methods (not general emit helpers,
        # since Python 3.7+ dicts maintain insertion order deterministically).
        _OUTPUT_METHOD_KEYWORDS = ("to_json", "serialize", "dump")

        unsafe_lines: list[tuple[int, str]] = []
        for i, line in enumerate(lines, 1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if dict_iter_pattern.search(stripped):
                if not sorted_wrapper.search(stripped):
                    for j in range(i - 1, max(0, i - 50), -1):
                        prev = lines[j - 1].strip()
                        if prev.startswith("def "):
                            if any(kw in prev for kw in _OUTPUT_METHOD_KEYWORDS):
                                unsafe_lines.append((i, stripped))
                            break

        assert not unsafe_lines, (
            "Unsorted dict iteration in output-producing methods:\n"
            + "\n".join(f"  line {n}: {t}" for n, t in unsafe_lines)
        )

    def test_no_set_iteration_for_output(self) -> None:
        """Verify that set iteration isn't used for output ordering in frontend.

        We only flag set iteration that directly feeds into output-producing
        methods (to_json, serialize, dump). Set iteration used internally for
        building analysis dicts (where iteration order doesn't affect values,
        e.g., max/min aggregation) is safe.
        """
        if not FRONTEND_INIT.exists():
            pytest.skip("Frontend __init__.py not found")

        content = FRONTEND_INIT.read_text()
        lines = content.splitlines()

        set_iter_pattern = re.compile(r"for\s+\w+\s+in\s+(?:self\.\w+_set|set\()")
        sorted_wrapper = re.compile(r"\bsorted\(")
        _OUTPUT_METHOD_KEYWORDS = ("to_json", "serialize", "dump")

        unsafe_lines: list[tuple[int, str]] = []
        for i, line in enumerate(lines, 1):
            stripped = line.strip()
            if stripped.startswith("#"):
                continue
            if set_iter_pattern.search(stripped) and not sorted_wrapper.search(
                stripped
            ):
                # Only flag if inside an output-producing method
                for j in range(i - 1, max(0, i - 80), -1):
                    prev = lines[j - 1].strip()
                    if prev.startswith("def "):
                        if any(kw in prev for kw in _OUTPUT_METHOD_KEYWORDS):
                            unsafe_lines.append((i, stripped))
                        break

        assert not unsafe_lines, (
            "Unsorted set iteration in output-producing methods:\n"
            + "\n".join(f"  line {n}: {t}" for n, t in unsafe_lines)
        )


class TestIdOrdering:
    """Verify that id() is not used for ordering decisions."""

    def test_no_id_ordering_in_frontend(self) -> None:
        """id() must not be used as a sort key or comparison for ordering."""
        if not FRONTEND_INIT.exists():
            pytest.skip("Frontend __init__.py not found")

        findings = _find_pattern_in_file(FRONTEND_INIT, _ID_ORDERING_PATTERN)
        assert not findings, (
            f"id()-based ordering found in {FRONTEND_INIT.name}:\n"
            + "\n".join(f"  line {n}: {t}" for n, t in findings)
        )

    def test_no_id_ordering_in_compiler_sources(self) -> None:
        """No compiler source should use id() for ordering."""
        all_findings: list[tuple[str, int, str]] = []
        for src in _compiler_source_files():
            rel = src.relative_to(ROOT)
            if "test" in str(rel).lower():
                continue
            findings = _find_pattern_in_file(src, _ID_ORDERING_PATTERN)
            for lineno, text in findings:
                all_findings.append((str(rel), lineno, text))

        assert not all_findings, (
            "id()-based ordering found in compiler code:\n"
            + "\n".join(f"  {f}:{n}: {t}" for f, n, t in all_findings)
        )
