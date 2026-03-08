#!/usr/bin/env python3
"""Static quality gate for transpiled Luau output.

Transpiles Python sources to Luau with Molt, then optionally runs ``luau-analyze``
when available. Analyzer warning classes are parsed on a best-effort basis and
reported in aggregate.

Usage:
    python tools/check_luau_static.py source.py
    python tools/check_luau_static.py --batch tests/differential/basic --pattern "*.py"
    python tools/check_luau_static.py --batch tests --json-out luau_static_report.json
    python tools/check_luau_static.py source.py --require-analyzer

Exit codes:
    0 -- success (all transpiles succeeded; analyzer optional)
    1 -- one or more transpile/analyzer execution failures
    2 -- usage/configuration error
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from collections import Counter
from dataclasses import asdict, dataclass, field
from pathlib import Path

_WARNING_CLASS_PATTERNS = (
    re.compile(r"\bWarning\[(?P<class>[A-Za-z_][A-Za-z0-9_]*)\]"),
    re.compile(
        r"\bwarning(?:\s*\[[^\]]+\])?\s*[:\-]\s*(?P<class>[A-Za-z_][A-Za-z0-9_]*)",
        flags=re.IGNORECASE,
    ),
    re.compile(r"\[(?P<class>[A-Za-z_][A-Za-z0-9_]*)\]\s*$"),
)

_KNOWN_WARNING_CLASSES = {
    "DeprecatedApi",
    "LocalShadow",
    "TypeMismatch",
    "UninitializedLocal",
    "UnknownGlobal",
    "UnknownType",
    "UnreachableCode",
}

_WARNING_WORD_RE = re.compile(r"\bwarning\b", flags=re.IGNORECASE)
_WARNING_SUMMARY_RE = re.compile(
    r"\b\d+\s+warnings?\b.*\b(found|generated|emitted|total)\b",
    flags=re.IGNORECASE,
)


@dataclass
class FileResult:
    source: str
    luau_output: str
    build_ok: bool = False
    build_returncode: int | None = None
    build_stdout: str = ""
    build_stderr: str = ""
    analyzer_status: str = "skipped"
    analyzer_returncode: int | None = None
    analyzer_stdout: str = ""
    analyzer_stderr: str = ""
    warning_count: int = 0
    warning_classes: dict[str, int] = field(default_factory=dict)


@dataclass
class AggregateReport:
    sources_total: int
    transpile_pass: int
    transpile_fail: int
    analyzer_available: bool
    analyzer_required: bool
    analyzer_executed: int
    analyzer_skipped: int
    analyzer_failures: int
    warnings_total: int
    warning_classes: dict[str, int]
    files: list[FileResult]

    def to_dict(self) -> dict[str, object]:
        payload = asdict(self)
        payload["files"] = [asdict(item) for item in self.files]
        return payload


def _repo_root() -> Path:
    return Path(__file__).resolve().parents[1]


def _with_repo_pythonpath(env: dict[str, str], repo_root: Path) -> dict[str, str]:
    updated = env.copy()
    src_dir = str(repo_root / "src")
    current = updated.get("PYTHONPATH", "")
    if current:
        parts = current.split(os.pathsep)
        if src_dir not in parts:
            updated["PYTHONPATH"] = src_dir + os.pathsep + current
    else:
        updated["PYTHONPATH"] = src_dir
    return updated


def make_build_env() -> dict[str, str]:
    """Build child-process env with external-volume defaults."""
    env = os.environ.copy()
    ext_root = Path(env.get("MOLT_EXT_ROOT", "/Volumes/APDataStore/Molt")).expanduser().resolve()
    if not ext_root.is_dir():
        raise RuntimeError(
            "External volume is required for Luau static checks. "
            f"Set MOLT_EXT_ROOT to a mounted external path (current: {ext_root})."
        )
    env.setdefault("MOLT_EXT_ROOT", str(ext_root))
    env.setdefault("CARGO_TARGET_DIR", str(ext_root / "cargo-target"))
    env.setdefault("MOLT_DIFF_CARGO_TARGET_DIR", env["CARGO_TARGET_DIR"])
    env.setdefault("MOLT_CACHE", str(ext_root / "molt_cache"))
    env.setdefault("MOLT_DIFF_ROOT", str(ext_root / "diff"))
    env.setdefault("MOLT_DIFF_TMPDIR", str(ext_root / "tmp"))
    env.setdefault("UV_CACHE_DIR", str(ext_root / "uv-cache"))
    env.setdefault("TMPDIR", env["MOLT_DIFF_TMPDIR"])
    env.setdefault("PYTHONHASHSEED", "0")
    return _with_repo_pythonpath(env, _repo_root())


def _pick_temp_parent(env: dict[str, str]) -> str | None:
    explicit_keys = {key for key in ("MOLT_DIFF_TMPDIR", "TMPDIR") if os.environ.get(key)}
    for key in ("MOLT_DIFF_TMPDIR", "TMPDIR"):
        value = env.get(key)
        if not value:
            continue
        candidate = Path(value)
        if candidate.is_dir():
            return str(candidate)
        if key not in explicit_keys:
            continue
        try:
            candidate.mkdir(parents=True, exist_ok=True)
        except OSError:
            continue
        if candidate.is_dir():
            return str(candidate)
    return None


def collect_sources(source: str | None, batch: str | None, pattern: str) -> list[Path]:
    if source and batch:
        raise ValueError("Provide either a source file or --batch DIR, not both.")

    if batch:
        batch_root = Path(batch)
        if not batch_root.is_dir():
            raise ValueError(f"Batch directory not found: {batch}")
        matches = sorted(
            p.resolve()
            for p in batch_root.rglob(pattern)
            if p.is_file() and p.suffix == ".py"
        )
        if not matches:
            raise ValueError(f"No Python files matched pattern '{pattern}' under: {batch}")
        return matches

    if source:
        src = Path(source)
        if not src.is_file():
            raise ValueError(f"Source file not found: {source}")
        if src.suffix != ".py":
            raise ValueError(f"Source must be a .py file: {source}")
        return [src.resolve()]

    raise ValueError("Provide a source file or --batch DIR.")


def _luau_output_path(source: Path, index: int, out_root: Path) -> Path:
    digest = hashlib.sha1(str(source).encode("utf-8")).hexdigest()[:8]
    return out_root / f"{index:04d}_{source.stem}_{digest}.luau"


def _trim_output(text: str, limit: int = 2000) -> str:
    if len(text) <= limit:
        return text
    return text[: limit - 3] + "..."


def transpile_to_luau(
    source: Path,
    output_path: Path,
    env: dict[str, str],
    timeout_s: float,
) -> tuple[bool, int | None, str, str]:
    cmd = [
        sys.executable,
        "-m",
        "molt.cli",
        "build",
        str(source),
        "--target",
        "luau",
        "--profile",
        "dev",
        "--output",
        str(output_path),
    ]
    try:
        proc = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            cwd=str(_repo_root()),
            env=env,
            timeout=timeout_s,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return False, None, "", f"build timed out after {timeout_s}s"

    ok = proc.returncode == 0 and output_path.exists()
    if proc.returncode == 0 and not output_path.exists():
        ok = False
    stderr = proc.stderr
    if proc.returncode == 0 and not output_path.exists():
        stderr = stderr + "\nMissing Luau output file after successful build exit."
    return ok, proc.returncode, proc.stdout, stderr


def _is_warning_line(line: str) -> bool:
    lower = line.lower()
    if _WARNING_SUMMARY_RE.search(line):
        return False
    if "0 warnings" in lower:
        return False
    if _WARNING_WORD_RE.search(line):
        return True
    return any(name in line for name in _KNOWN_WARNING_CLASSES)


def _extract_warning_class(line: str) -> str:
    for pattern in _WARNING_CLASS_PATTERNS:
        match = pattern.search(line)
        if match:
            return match.group("class")

    for token in re.findall(r"\b[A-Za-z_][A-Za-z0-9_]*\b", line):
        if token in _KNOWN_WARNING_CLASSES:
            return token

    warning_match = _WARNING_WORD_RE.search(line)
    if warning_match:
        tail = line[warning_match.end() :]
        candidate = re.search(r"[A-Za-z_][A-Za-z0-9_]*", tail)
        if candidate:
            return candidate.group(0)

    camel_case = re.search(r"\b[A-Z][A-Za-z0-9_]+\b", line)
    if camel_case:
        return camel_case.group(0)

    return "UnknownWarning"


def parse_analyzer_warnings(output: str) -> tuple[int, dict[str, int]]:
    classes: Counter[str] = Counter()
    for raw_line in output.splitlines():
        line = raw_line.strip()
        if not line:
            continue
        if not _is_warning_line(line):
            continue
        warning_class = _extract_warning_class(line)
        classes[warning_class] += 1
    return sum(classes.values()), dict(sorted(classes.items()))


def run_analyzer(
    analyzer: str,
    luau_file: Path,
    timeout_s: float,
) -> tuple[str, int | None, str, str, int, dict[str, int]]:
    try:
        proc = subprocess.run(
            [analyzer, str(luau_file)],
            capture_output=True,
            text=True,
            cwd=str(_repo_root()),
            timeout=timeout_s,
            check=False,
        )
    except subprocess.TimeoutExpired:
        return "timeout", None, "", f"analyzer timed out after {timeout_s}s", 0, {}
    except OSError as exc:
        return "error", None, "", str(exc), 0, {}

    combined = "\n".join(part for part in (proc.stdout, proc.stderr) if part)
    warning_count, warning_classes = parse_analyzer_warnings(combined)
    status = "ok" if proc.returncode == 0 else "nonzero_exit"
    return (
        status,
        proc.returncode,
        proc.stdout,
        proc.stderr,
        warning_count,
        warning_classes,
    )


def _print_file_line(index: int, total: int, result: FileResult) -> None:
    source_name = Path(result.source).name
    build_state = "PASS" if result.build_ok else "FAIL"
    analyzer_state = result.analyzer_status
    print(
        f"[{index}/{total}] {source_name}: build={build_state}, "
        f"analyzer={analyzer_state}, warnings={result.warning_count}"
    )


def _aggregate(
    results: list[FileResult],
    analyzer_available: bool,
    analyzer_required: bool,
) -> AggregateReport:
    warning_classes: Counter[str] = Counter()
    for item in results:
        warning_classes.update(item.warning_classes)

    transpile_pass = sum(1 for item in results if item.build_ok)
    transpile_fail = sum(1 for item in results if not item.build_ok)
    analyzer_executed = sum(
        1 for item in results if item.analyzer_status in {"ok", "nonzero_exit", "timeout", "error"}
    )
    analyzer_skipped = len(results) - analyzer_executed
    analyzer_failures = sum(
        1
        for item in results
        if item.analyzer_status in {"nonzero_exit", "timeout", "error"}
    )
    warnings_total = sum(item.warning_count for item in results)

    return AggregateReport(
        sources_total=len(results),
        transpile_pass=transpile_pass,
        transpile_fail=transpile_fail,
        analyzer_available=analyzer_available,
        analyzer_required=analyzer_required,
        analyzer_executed=analyzer_executed,
        analyzer_skipped=analyzer_skipped,
        analyzer_failures=analyzer_failures,
        warnings_total=warnings_total,
        warning_classes=dict(sorted(warning_classes.items())),
        files=results,
    )


def _write_json(path: Path, payload: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(payload, indent=2, sort_keys=True), encoding="utf-8")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        description=__doc__,
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "source",
        nargs="?",
        help="Single Python file to transpile",
    )
    parser.add_argument(
        "--batch",
        metavar="DIR",
        help="Recursively transpile .py files in DIR",
    )
    parser.add_argument(
        "--pattern",
        default="*.py",
        help="Glob pattern used with --batch (default: *.py)",
    )
    parser.add_argument(
        "--json-out",
        metavar="FILE",
        help="Write a machine-readable JSON report",
    )
    parser.add_argument(
        "--require-analyzer",
        action="store_true",
        help="Fail when luau-analyze is not available",
    )
    parser.add_argument(
        "--build-timeout",
        type=float,
        default=240.0,
        help="Timeout (seconds) for each transpile command (default: 240)",
    )
    parser.add_argument(
        "--analyzer-timeout",
        type=float,
        default=60.0,
        help="Timeout (seconds) for each analyzer invocation (default: 60)",
    )

    args = parser.parse_args(argv)

    try:
        sources = collect_sources(args.source, args.batch, args.pattern)
    except ValueError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2

    analyzer = shutil.which("luau-analyze")
    if args.require_analyzer and analyzer is None:
        print(
            "ERROR: --require-analyzer was set but 'luau-analyze' was not found in PATH.",
            file=sys.stderr,
        )
        return 2

    analyzer_available = analyzer is not None
    if analyzer_available:
        print(f"Analyzer: enabled ({analyzer})")
    else:
        print("Analyzer: skipped (luau-analyze not found in PATH)")

    try:
        env = make_build_env()
    except RuntimeError as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    temp_parent = _pick_temp_parent(env)

    print(f"Luau static check: {len(sources)} source file(s)")
    if temp_parent:
        print(f"Temp root: {temp_parent}")
    else:
        print("Temp root: system default")

    results: list[FileResult] = []
    with tempfile.TemporaryDirectory(prefix="molt-luau-static-", dir=temp_parent) as tmpdir:
        tmp_root = Path(tmpdir)
        for index, source in enumerate(sources, start=1):
            luau_path = _luau_output_path(source, index, tmp_root)
            result = FileResult(source=str(source), luau_output=str(luau_path))

            (
                result.build_ok,
                result.build_returncode,
                build_stdout,
                build_stderr,
            ) = transpile_to_luau(
                source=source,
                output_path=luau_path,
                env=env,
                timeout_s=args.build_timeout,
            )
            result.build_stdout = _trim_output(build_stdout)
            result.build_stderr = _trim_output(build_stderr)

            if result.build_ok and analyzer_available and analyzer is not None:
                (
                    result.analyzer_status,
                    result.analyzer_returncode,
                    analyzer_stdout,
                    analyzer_stderr,
                    result.warning_count,
                    result.warning_classes,
                ) = run_analyzer(analyzer, luau_path, args.analyzer_timeout)
                result.analyzer_stdout = _trim_output(analyzer_stdout)
                result.analyzer_stderr = _trim_output(analyzer_stderr)
            elif result.build_ok:
                result.analyzer_status = "skipped_unavailable"
            else:
                result.analyzer_status = "skipped_build_failed"

            results.append(result)
            _print_file_line(index, len(sources), result)

    aggregate = _aggregate(results, analyzer_available, args.require_analyzer)

    print("\nSummary:")
    print(
        f"  Transpile: {aggregate.transpile_pass} pass, {aggregate.transpile_fail} fail"
    )
    print(
        "  Analyzer: "
        f"executed={aggregate.analyzer_executed}, "
        f"skipped={aggregate.analyzer_skipped}, "
        f"failures={aggregate.analyzer_failures}"
    )
    print(f"  Warnings: {aggregate.warnings_total}")
    if aggregate.warning_classes:
        formatted = ", ".join(
            f"{name}={count}" for name, count in aggregate.warning_classes.items()
        )
        print(f"  Warning classes: {formatted}")
    else:
        print("  Warning classes: (none)")

    if args.json_out:
        report_path = Path(args.json_out)
        _write_json(report_path, aggregate.to_dict())
        print(f"  JSON report: {report_path}")

    if aggregate.transpile_fail > 0:
        return 1
    if aggregate.analyzer_failures > 0:
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
