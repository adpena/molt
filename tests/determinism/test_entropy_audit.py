"""Static analysis tests for nondeterminism sources in the Molt compiler.

Scans the packages that produce IR for patterns that could make it
nondeterministic:
- Entropy sources: random.*, os.urandom, uuid.*
- Timestamp leakage: time.time(), datetime.now()
- Unsafe iteration: bare dict/set iteration used for output ordering
- id()-based ordering decisions (anywhere in src/molt)

Custody code outside these packages may use entropy for staging names and
upload IDs; those never reach compiler output, which
``tests/determinism/test_ir_determinism.py`` checks by compiling twice.
"""

from __future__ import annotations

import re
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
SRC_DIR = ROOT / "src" / "molt"
# The packages whose code produces IR.
CODEGEN_PACKAGES = (SRC_DIR / "frontend", SRC_DIR / "compiler_analysis")

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------


def _codegen_source_files() -> list[Path]:
    """Every Python source file in the packages that produce IR."""
    missing = [package for package in CODEGEN_PACKAGES if not package.is_dir()]
    assert not missing, f"codegen packages moved: {missing}"
    return sorted(
        path for package in CODEGEN_PACKAGES for path in package.rglob("*.py")
    )


def _findings(
    files: list[Path],
    pattern: re.Pattern[str],
    *,
    exclude_patterns: list[re.Pattern[str]] | None = None,
) -> list[str]:
    return [
        f"  {path.relative_to(ROOT).as_posix()}:{lineno}: {text}"
        for path in files
        for lineno, text in _find_pattern_in_file(
            path, pattern, exclude_patterns=exclude_patterns
        )
    ]


def _output_method_lines(
    path: Path, pattern: re.Pattern[str], *, lookback: int
) -> list[str]:
    """Lines matching ``pattern`` without sorted() inside an output method."""
    lines = path.read_text(encoding="utf-8").splitlines()
    sorted_wrapper = re.compile(r"\bsorted\(")
    unsafe: list[str] = []
    for i, line in enumerate(lines, 1):
        stripped = line.strip()
        if stripped.startswith("#"):
            continue
        if pattern.search(stripped) and not sorted_wrapper.search(stripped):
            for j in range(i - 1, max(0, i - lookback), -1):
                prev = lines[j - 1].strip()
                if prev.startswith("def "):
                    if any(kw in prev for kw in _OUTPUT_METHOD_KEYWORDS):
                        unsafe.append(
                            f"  {path.relative_to(ROOT).as_posix()}:{i}: {stripped}"
                        )
                    break
    return unsafe


def _read_lines(path: Path) -> list[tuple[int, str]]:
    """Return (1-based line number, line text) pairs, skipping comments."""
    lines = []
    for i, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
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

# Only methods that build the final output structure; Python 3.7+ dicts keep
# insertion order, so general emit helpers are deterministic.
_OUTPUT_METHOD_KEYWORDS = ("to_json", "serialize", "dump")

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
    """Verify no entropy sources exist where IR is produced."""

    def test_no_entropy_in_codegen_sources(self) -> None:
        findings = _findings(_codegen_source_files(), _ENTROPY_PATTERN)
        assert not findings, "Entropy sources found in codegen code:\n" + "\n".join(
            findings
        )


class TestTimestampLeakage:
    """Verify no timestamps leak into compiler output."""

    def test_no_output_timestamps_in_codegen_sources(self) -> None:
        """time.monotonic() and timing-context lines stay allowed."""
        findings = _findings(
            _codegen_source_files(),
            _TIMESTAMP_PATTERN,
            exclude_patterns=_TIMESTAMP_EXCLUDES,
        )
        assert not findings, (
            "Timestamp usage found in codegen code (may leak into output):\n"
            + "\n".join(findings)
        )


class TestDictSetIterationSafety:
    """Verify that dict/set iteration doesn't leak ordering into output."""

    def test_no_unsafe_dict_iteration_for_output(self) -> None:
        """Dict iteration in to_json/serialize/dump methods uses sorted()."""
        dict_iter_pattern = re.compile(
            r"for\s+\w+(?:\s*,\s*\w+)?\s+in\s+self\.\w+(?:\.items\(\)|\.keys\(\)|\.values\(\)|\b)"
        )
        unsafe = [
            line
            for path in _codegen_source_files()
            for line in _output_method_lines(path, dict_iter_pattern, lookback=50)
        ]
        assert not unsafe, (
            "Unsorted dict iteration in output-producing methods:\n" + "\n".join(unsafe)
        )

    def test_no_set_iteration_for_output(self) -> None:
        """Set iteration in to_json/serialize/dump methods uses sorted()."""
        set_iter_pattern = re.compile(r"for\s+\w+\s+in\s+(?:self\.\w+_set|set\()")
        unsafe = [
            line
            for path in _codegen_source_files()
            for line in _output_method_lines(path, set_iter_pattern, lookback=80)
        ]
        assert not unsafe, (
            "Unsorted set iteration in output-producing methods:\n" + "\n".join(unsafe)
        )


class TestIdOrdering:
    """Verify that id() is not used for ordering decisions."""

    def test_no_id_ordering_in_compiler_sources(self) -> None:
        findings = _findings(sorted(SRC_DIR.rglob("*.py")), _ID_ORDERING_PATTERN)
        assert not findings, "id()-based ordering found in compiler code:\n" + (
            "\n".join(findings)
        )
