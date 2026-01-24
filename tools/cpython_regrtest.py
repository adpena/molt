#!/usr/bin/env python3
from __future__ import annotations

import argparse
import csv
import datetime as dt
import json
import os
import shlex
import shutil
import subprocess
import sys
from dataclasses import dataclass, replace
from pathlib import Path
from typing import Iterable
from xml.etree import ElementTree

REPO_ROOT = Path(__file__).resolve().parents[1]


@dataclass
class RegrtestConfig:
    repo_root: Path
    cpython_dir: Path
    cpython_branch: str
    host_python: str
    use_uv: bool
    uv_project: Path | None
    uv_python: list[str]
    uv_prepare: bool
    uv_add: list[str]
    molt_cmd: list[str]
    molt_capabilities: str | None
    molt_shim_path: Path
    output_root: Path
    output_dir: Path
    skip_file: Path | None
    workers: int
    rerun_failures: bool
    match: list[str]
    match_file: Path | None
    ignore: list[str]
    ignore_file: Path | None
    resources: list[str]
    timeout: int | None
    junit_xml: Path
    tests: list[str]
    regrtest_args: list[str]
    enable_coverage: bool
    coverage_source: list[str]
    coverage_dir: Path
    stdlib_version: str
    stdlib_source: str
    matrix_path: Path
    matrix_format: str
    type_matrix_path: Path
    semantics_matrix_path: Path
    diff_enabled: bool
    diff_paths: list[Path]
    diff_python_version: str | None
    core_only: bool
    core_file: Path
    property_tests: Path | None
    rust_coverage: bool
    rust_coverage_dir: Path
    dry_run: bool
    allow_clone: bool


@dataclass
class RegrtestSummary:
    tests: int
    failures: int
    errors: int
    skipped: int
    failed_modules: list[str]
    returncode: int


@dataclass
class CoverageSummary:
    total_percent: float
    files: dict[str, float]


@dataclass
class RustCoverageSummary:
    output_dir: Path
    returncode: int
    command: list[str]
    available: bool
    message: str | None


@dataclass
class DiffSummary:
    total: int
    passed: int
    failed: int
    failed_files: list[str]
    returncode: int
    json_path: Path | None
    md_path: Path | None


@dataclass
class MatrixReport:
    json_path: Path
    md_path: Path
    summary: dict[str, dict[str, int]]


def parse_args(argv: list[str]) -> RegrtestConfig:
    parser = argparse.ArgumentParser(
        description="Run CPython's regrtest against Molt with reporting.",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="Repository root (default: repo root).",
    )
    parser.add_argument(
        "--cpython-dir",
        type=Path,
        default=REPO_ROOT / "third_party" / "cpython",
        help="Path to a CPython checkout (default: third_party/cpython).",
    )
    parser.add_argument(
        "--cpython-branch",
        default="v3.12.x",
        help="CPython branch/tag to clone when --clone is used.",
    )
    parser.add_argument(
        "--clone",
        action="store_true",
        help="Clone CPython into --cpython-dir if missing (network).",
    )
    parser.add_argument(
        "--host-python",
        default=sys.executable,
        help="Host CPython executable used to run regrtest.",
    )
    parser.add_argument(
        "--uv",
        action="store_true",
        help="Use uv run for host Python and dependency management.",
    )
    parser.add_argument(
        "--uv-project",
        type=Path,
        default=None,
        help="uv project root (defaults to --repo-root).",
    )
    parser.add_argument(
        "--uv-python",
        action="append",
        default=[],
        help="Python version for uv run (repeatable).",
    )
    parser.add_argument(
        "--uv-prepare",
        action="store_true",
        help="Install uv Python versions and add dependencies with uv add.",
    )
    parser.add_argument(
        "--uv-add",
        action="append",
        default=[],
        help="Extra dependencies to add with uv add (repeatable).",
    )
    parser.add_argument(
        "--molt-cmd",
        nargs="+",
        default=None,
        help="Command used by the regrtest shim to run a test file.",
    )
    parser.add_argument(
        "--molt-capabilities",
        default="fs.read,env.read",
        help="Comma-separated MOLT_CAPABILITIES for Molt test runs.",
    )
    parser.add_argument(
        "--molt-shim",
        type=Path,
        default=REPO_ROOT / "tools" / "molt_regrtest_shim.py",
        help="Path to the regrtest python shim.",
    )
    parser.add_argument(
        "--output-dir",
        type=Path,
        default=None,
        help="Directory for logs and reports (default: logs/cpython_regrtest/<ts>).",
    )
    parser.add_argument(
        "--skip-file",
        type=Path,
        default=REPO_ROOT / "tools" / "cpython_regrtest_skip.txt",
        help="File listing test modules to skip (one per line).",
    )
    parser.add_argument(
        "--workers",
        type=int,
        default=max(1, (os.cpu_count() or 1)),
        help="Number of parallel workers (-j).",
    )
    parser.add_argument(
        "--rerun-failures",
        action="store_true",
        help="Re-run failed tests (-w).",
    )
    parser.add_argument(
        "--match",
        action="append",
        default=[],
        help="Match test cases/methods (regrtest -m).",
    )
    parser.add_argument(
        "--match-file",
        type=Path,
        default=None,
        help="File with match patterns (regrtest --matchfile).",
    )
    parser.add_argument(
        "--ignore",
        action="append",
        default=[],
        help="Ignore test cases/methods (regrtest -i).",
    )
    parser.add_argument(
        "--ignore-file",
        type=Path,
        default=None,
        help="File with ignore patterns (regrtest --ignorefile).",
    )
    parser.add_argument(
        "--resource",
        action="append",
        default=[],
        help="Enable resource-intensive tests (regrtest -u).",
    )
    parser.add_argument(
        "--timeout",
        type=int,
        default=None,
        help="Per-test timeout in seconds (regrtest --timeout).",
    )
    parser.add_argument(
        "--regrtest-arg",
        action="append",
        default=[],
        help="Extra argument to pass to regrtest (repeatable).",
    )
    parser.add_argument(
        "--coverage",
        action="store_true",
        help="Enable coverage run for the host regrtest driver.",
    )
    parser.add_argument(
        "--coverage-source",
        action="append",
        default=[str(REPO_ROOT / "src" / "molt")],
        help="Coverage --source entries (repeatable).",
    )
    parser.add_argument(
        "--stdlib-version",
        default="3.12",
        help="Stdlib version for module list (default: 3.12).",
    )
    parser.add_argument(
        "--stdlib-source",
        default="auto",
        choices=["auto", "sys", "stdlib-list"],
        help="Stdlib module source.",
    )
    parser.add_argument(
        "--matrix-path",
        type=Path,
        default=REPO_ROOT
        / "docs"
        / "spec"
        / "areas"
        / "compat"
        / "0015_STDLIB_COMPATIBILITY_MATRIX.md",
        help="Path to Molt stdlib compatibility matrix.",
    )
    parser.add_argument(
        "--matrix-format",
        choices=["json", "csv", "both"],
        default="both",
        help="Output format for stdlib matrix.",
    )
    parser.add_argument(
        "--type-matrix-path",
        type=Path,
        default=REPO_ROOT
        / "docs"
        / "spec"
        / "areas"
        / "compat"
        / "0014_TYPE_COVERAGE_MATRIX.md",
        help="Path to the Molt type coverage matrix.",
    )
    parser.add_argument(
        "--semantics-matrix-path",
        type=Path,
        default=REPO_ROOT
        / "docs"
        / "spec"
        / "areas"
        / "compat"
        / "0023_SEMANTIC_BEHAVIOR_MATRIX.md",
        help="Path to the Molt semantic behavior matrix.",
    )
    parser.add_argument(
        "--diff",
        action=argparse.BooleanOptionalAction,
        default=True,
        help="Run the Molt differential suite alongside regrtest.",
    )
    parser.add_argument(
        "--diff-path",
        action="append",
        type=Path,
        default=[],
        help="Path to differential tests (repeatable).",
    )
    parser.add_argument(
        "--diff-python-version",
        default=None,
        help="Python version for differential runs (defaults to regrtest version).",
    )
    parser.add_argument(
        "--core-only",
        action="store_true",
        help="Run a curated core-only test list via --fromfile.",
    )
    parser.add_argument(
        "--core-file",
        type=Path,
        default=REPO_ROOT / "tools" / "cpython_regrtest_core.txt",
        help="Path to a core-only test list for regrtest.",
    )
    parser.add_argument(
        "--property-tests",
        type=Path,
        default=None,
        help="Optional path to property-based tests (pytest).",
    )
    parser.add_argument(
        "--rust-coverage",
        action="store_true",
        help="Run cargo llvm-cov to collect Rust coverage.",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print commands without executing.",
    )
    parser.add_argument(
        "tests",
        nargs="*",
        help="Optional test modules to run (e.g., test_math).",
    )
    args = parser.parse_args(argv)

    repo_root = args.repo_root.resolve()
    cpython_dir = args.cpython_dir.resolve()
    output_root = args.output_dir
    if output_root is None:
        ts = dt.datetime.now(dt.UTC).strftime("%Y%m%d_%H%M%S")
        output_root = repo_root / "logs" / "cpython_regrtest" / ts
    output_root = output_root.resolve()
    molt_shim = args.molt_shim.resolve()
    uv_project = args.uv_project.resolve() if args.uv_project is not None else None
    skip_file = args.skip_file.resolve() if args.skip_file is not None else None
    core_file = args.core_file.resolve()
    matrix_path = args.matrix_path.resolve()
    type_matrix_path = args.type_matrix_path.resolve()
    semantics_matrix_path = args.semantics_matrix_path.resolve()

    uv_python = args.uv_python
    if args.uv and not uv_python:
        uv_python = ["3.12"]

    diff_paths = args.diff_path
    if args.diff and not diff_paths:
        diff_paths = [args.repo_root / "tests" / "differential" / "basic"]

    molt_cmd = args.molt_cmd
    if not molt_cmd:
        molt_cmd = [args.host_python, "-m", "molt.cli", "run", "--compiled"]
    if len(molt_cmd) == 1:
        split_cmd = shlex.split(molt_cmd[0])
        if split_cmd:
            molt_cmd = split_cmd

    return RegrtestConfig(
        repo_root=repo_root,
        cpython_dir=cpython_dir,
        cpython_branch=args.cpython_branch,
        host_python=args.host_python,
        use_uv=args.uv,
        uv_project=uv_project,
        uv_python=uv_python,
        uv_prepare=args.uv_prepare,
        uv_add=args.uv_add,
        molt_cmd=molt_cmd,
        molt_capabilities=args.molt_capabilities,
        molt_shim_path=molt_shim,
        output_root=output_root,
        output_dir=output_root,
        skip_file=skip_file if skip_file and skip_file.exists() else None,
        workers=max(1, args.workers),
        rerun_failures=args.rerun_failures,
        match=args.match,
        match_file=args.match_file,
        ignore=args.ignore,
        ignore_file=args.ignore_file,
        resources=args.resource,
        timeout=args.timeout,
        junit_xml=output_root / "junit.xml",
        tests=args.tests,
        regrtest_args=args.regrtest_arg,
        enable_coverage=args.coverage,
        coverage_source=args.coverage_source,
        coverage_dir=output_root / "coverage",
        stdlib_version=args.stdlib_version,
        stdlib_source=args.stdlib_source,
        matrix_path=matrix_path,
        matrix_format=args.matrix_format,
        type_matrix_path=type_matrix_path,
        semantics_matrix_path=semantics_matrix_path,
        diff_enabled=args.diff,
        diff_paths=diff_paths,
        diff_python_version=args.diff_python_version,
        core_only=args.core_only,
        core_file=core_file,
        property_tests=args.property_tests,
        rust_coverage=args.rust_coverage,
        rust_coverage_dir=output_root / "rust_coverage",
        dry_run=args.dry_run,
        allow_clone=args.clone,
    )


def log_line(handle, message: str) -> None:
    timestamp = dt.datetime.now(dt.UTC).strftime("%Y-%m-%dT%H:%M:%SZ")
    handle.write(f"[{timestamp}] {message}\n")
    handle.flush()


def run_command(
    cmd: list[str],
    *,
    cwd: Path | None,
    env: dict[str, str] | None,
    log_handle,
    dry_run: bool,
) -> int:
    log_line(log_handle, f"cmd: {shlex.join(cmd)}")
    if dry_run:
        return 0
    result = subprocess.run(
        cmd,
        cwd=cwd,
        env=env,
        stdout=log_handle,
        stderr=log_handle,
        text=True,
        check=False,
    )
    return result.returncode


def resolve_uv_project(config: RegrtestConfig) -> Path:
    return config.uv_project or config.repo_root


def resolve_uv_deps(config: RegrtestConfig) -> list[str]:
    deps: list[str] = []
    if config.enable_coverage:
        deps.append("coverage")
    if config.stdlib_source in ("auto", "stdlib-list"):
        deps.append("stdlib-list")
    if config.property_tests is not None:
        deps.append("pytest")
        deps.append("hypothesis")
    deps.extend(config.uv_add)
    return sorted(set(deps))


def prepare_uv(
    config: RegrtestConfig,
    python_versions: Iterable[str],
    *,
    log_handle,
) -> None:
    if not config.use_uv or not config.uv_prepare:
        return
    for version in python_versions:
        cmd = ["uv", "python", "install", version]
        _ = run_command(
            cmd,
            cwd=config.repo_root,
            env=None,
            log_handle=log_handle,
            dry_run=config.dry_run,
        )
    deps = resolve_uv_deps(config)
    if deps:
        cmd = ["uv", "add", "--dev"]
        project = resolve_uv_project(config)
        if project:
            cmd.extend(["--project", str(project)])
        cmd.extend(deps)
        _ = run_command(
            cmd,
            cwd=config.repo_root,
            env=None,
            log_handle=log_handle,
            dry_run=config.dry_run,
        )


def host_python_cmd(config: RegrtestConfig, python_version: str | None) -> list[str]:
    if config.use_uv:
        cmd = ["uv", "run"]
        project = resolve_uv_project(config)
        if project:
            cmd.extend(["--project", str(project)])
        if python_version:
            cmd.extend(["--python", python_version])
        cmd.extend(["--", "python"])
        return cmd
    return [config.host_python]


def ensure_cpython_checkout(
    cpython_dir: Path,
    branch: str,
    *,
    allow_clone: bool,
    log_handle,
    dry_run: bool,
) -> None:
    if cpython_dir.exists():
        return
    if not allow_clone:
        raise FileNotFoundError(
            f"CPython dir missing: {cpython_dir} (use --clone to fetch)"
        )

    def fallback_branch(name: str) -> str | None:
        if name.startswith("v") and name.endswith(".x"):
            return name[1:-2]
        if name.startswith("v"):
            return name[1:]
        return None

    cpython_dir.parent.mkdir(parents=True, exist_ok=True)

    def run_clone(target: str) -> int:
        cmd = [
            "git",
            "clone",
            "--depth",
            "1",
            "--branch",
            target,
            "https://github.com/python/cpython.git",
            str(cpython_dir),
        ]
        return run_command(
            cmd, cwd=None, env=None, log_handle=log_handle, dry_run=dry_run
        )

    rc = run_clone(branch)
    if rc != 0:
        alt = fallback_branch(branch)
        if alt and alt != branch:
            log_line(log_handle, f"clone failed for {branch}; retrying {alt}")
            if cpython_dir.exists() and not dry_run:
                shutil.rmtree(cpython_dir)
            rc = run_clone(alt)
    if rc != 0:
        raise RuntimeError(f"CPython clone failed for {branch}")


def load_skip_list(path: Path | None) -> list[str]:
    if path is None:
        return []
    modules: list[str] = []
    for line in path.read_text().splitlines():
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            continue
        name = stripped
        if name.endswith(".py"):
            name = Path(name).stem
        name = name.replace("Lib/test/", "").replace("Lib/test\\", "")
        if name.startswith("test_"):
            modules.append(name)
    return sorted(set(modules))


def build_regrtest_cmd(
    config: RegrtestConfig,
    regrtest_path: Path,
    skip_modules: list[str],
    host_cmd: list[str],
) -> list[str]:
    shim_python = "python" if config.use_uv else config.host_python
    python_cmd = shlex.join(
        [
            shim_python,
            str(config.molt_shim_path),
            "--molt-cmd",
            shlex.join(config.molt_cmd),
            "--cpython-dir",
            str(config.cpython_dir),
        ]
    )
    cmd = host_cmd + [str(regrtest_path), "--python", python_cmd]
    cmd.extend(["--junit-xml", str(config.junit_xml)])
    cmd.extend(["-j", str(config.workers)])
    if config.rerun_failures:
        cmd.append("-w")
    for pattern in config.match:
        cmd.extend(["-m", pattern])
    if config.match_file is not None:
        cmd.extend(["--matchfile", str(config.match_file)])
    for pattern in config.ignore:
        cmd.extend(["-i", pattern])
    if config.ignore_file is not None:
        cmd.extend(["--ignorefile", str(config.ignore_file)])
    for resource in config.resources:
        cmd.extend(["-u", resource])
    if config.timeout is not None:
        cmd.extend(["--timeout", str(config.timeout)])
    if config.core_only:
        cmd.extend(["--fromfile", str(config.core_file)])
    for mod in skip_modules:
        cmd.extend(["-x", mod])
    cmd.extend(config.regrtest_args)
    if config.tests:
        cmd.extend(config.tests)
    return cmd


def parse_junit(path: Path) -> RegrtestSummary:
    tree = ElementTree.parse(path)
    root = tree.getroot()
    suites = [root] if root.tag == "testsuite" else root.findall("testsuite")
    tests = failures = errors = skipped = 0
    failed_modules: set[str] = set()
    for suite in suites:
        tests += int(suite.attrib.get("tests", 0))
        failures += int(suite.attrib.get("failures", 0))
        errors += int(suite.attrib.get("errors", 0))
        skipped += int(suite.attrib.get("skipped", 0))
        for case in suite.iter("testcase"):
            if case.find("failure") is None and case.find("error") is None:
                continue
            classname = case.attrib.get("classname", "")
            name = case.attrib.get("name", "")
            module = classname.split(".")[0] if classname else name.split(".")[0]
            if module:
                failed_modules.add(module)
    return RegrtestSummary(
        tests=tests,
        failures=failures,
        errors=errors,
        skipped=skipped,
        failed_modules=sorted(failed_modules),
        returncode=1 if failures or errors else 0,
    )


def parse_markdown_tables(path: Path) -> list[tuple[list[str], list[dict[str, str]]]]:
    if not path.exists():
        return []
    lines = path.read_text().splitlines()
    tables: list[tuple[list[str], list[dict[str, str]]]] = []
    idx = 0
    while idx < len(lines) - 1:
        line = lines[idx].strip()
        if not (line.startswith("|") and "|" in line):
            idx += 1
            continue
        sep = lines[idx + 1].strip()
        if not (sep.startswith("|") and "-" in sep):
            idx += 1
            continue
        header = [cell.strip() for cell in line.strip("|").split("|")]
        rows: list[dict[str, str]] = []
        idx += 2
        while idx < len(lines):
            row_line = lines[idx].strip()
            if not row_line.startswith("|"):
                break
            cells = [cell.strip() for cell in row_line.strip("|").split("|")]
            if len(cells) == len(header):
                rows.append(dict(zip(header, cells)))
            idx += 1
        tables.append((header, rows))
        idx += 1
    return tables


def status_bucket(status: str) -> str:
    lowered = status.strip().lower()
    if "supported" in lowered or "implemented" in lowered:
        return "supported"
    if "partial" in lowered:
        return "partial"
    if "planned" in lowered:
        return "planned"
    if "missing" in lowered:
        return "missing"
    if "divergent" in lowered:
        return "divergent"
    return "unknown"


def parse_type_coverage(path: Path) -> dict[str, list[dict[str, str]]]:
    tables = parse_markdown_tables(path)
    types: list[dict[str, str]] = []
    builtins: list[dict[str, str]] = []
    for headers, rows in tables:
        if "Type" in headers and "Status" in headers:
            for row in rows:
                name = row.get("Type", "")
                if not name:
                    continue
                types.append(
                    {
                        "name": name,
                        "status": row.get("Status", ""),
                        "priority": row.get("Priority", ""),
                        "milestone": row.get("Milestone", ""),
                        "owner": row.get("Owner", ""),
                        "notes": row.get("Required Semantics (short)", ""),
                    }
                )
        if "Builtin" in headers and "Status" in headers:
            for row in rows:
                name = row.get("Builtin", "")
                if not name:
                    continue
                builtins.append(
                    {
                        "name": name,
                        "status": row.get("Status", ""),
                        "priority": row.get("Priority", ""),
                        "milestone": row.get("Milestone", ""),
                        "owner": row.get("Owner", ""),
                        "notes": row.get("Required Semantics (short)", ""),
                    }
                )
    return {"types": types, "builtins": builtins}


def parse_semantics_matrix(path: Path) -> list[dict[str, str]]:
    tables = parse_markdown_tables(path)
    entries: list[dict[str, str]] = []
    for headers, rows in tables:
        if "Feature" not in headers or "Status" not in headers:
            continue
        for row in rows:
            feature = row.get("Feature", "")
            if not feature:
                continue
            entries.append(
                {
                    "feature": feature,
                    "status": row.get("Status", ""),
                    "semantics": row.get("Semantics", ""),
                    "behavior": row.get("Molt Behavior", ""),
                    "notes": row.get("Notes", ""),
                }
            )
    return entries


def summarize_statuses(items: Iterable[dict[str, str]], key: str) -> dict[str, int]:
    counts: dict[str, int] = {}
    for item in items:
        bucket = status_bucket(item.get(key, ""))
        counts[bucket] = counts.get(bucket, 0) + 1
    return counts


def write_type_semantics_report(config: RegrtestConfig) -> MatrixReport:
    type_data = parse_type_coverage(config.type_matrix_path)
    semantics_data = parse_semantics_matrix(config.semantics_matrix_path)
    summary = {
        "types": summarize_statuses(type_data["types"], "status"),
        "builtins": summarize_statuses(type_data["builtins"], "status"),
        "semantics": summarize_statuses(semantics_data, "status"),
    }
    payload = {
        "type_matrix_path": str(config.type_matrix_path),
        "semantics_matrix_path": str(config.semantics_matrix_path),
        "summary": summary,
        "types": type_data["types"],
        "builtins": type_data["builtins"],
        "semantics": semantics_data,
    }
    json_path = config.output_dir / "type_semantics_matrix.json"
    md_path = config.output_dir / "type_semantics_matrix.md"
    json_path.write_text(json.dumps(payload, indent=2, sort_keys=True))
    md_lines = [
        "# Type + Semantics Matrix Summary",
        "",
        "## Type coverage",
        "",
        "| Status | Count |",
        "| --- | --- |",
    ]
    for status, count in sorted(summary["types"].items()):
        md_lines.append(f"| {status} | {count} |")
    md_lines.extend(
        [
            "",
            "## Builtins coverage",
            "",
            "| Status | Count |",
            "| --- | --- |",
        ]
    )
    for status, count in sorted(summary["builtins"].items()):
        md_lines.append(f"| {status} | {count} |")
    md_lines.extend(
        [
            "",
            "## Semantics coverage",
            "",
            "| Status | Count |",
            "| --- | --- |",
        ]
    )
    for status, count in sorted(summary["semantics"].items()):
        md_lines.append(f"| {status} | {count} |")
    md_lines.append("")
    md_path.write_text("\n".join(md_lines))
    return MatrixReport(json_path=json_path, md_path=md_path, summary=summary)


def run_coverage(
    config: RegrtestConfig,
    regrtest_cmd: list[str],
    host_cmd: list[str],
    *,
    cwd: Path,
    env: dict[str, str],
    log_handle,
) -> int:
    coverage_dir = config.coverage_dir
    coverage_dir.mkdir(parents=True, exist_ok=True)
    data_file = coverage_dir / ".coverage"
    cmd = host_cmd + [
        "-m",
        "coverage",
        "run",
        "--parallel-mode",
        "--pylib",
        "--data-file",
        str(data_file),
    ]
    if config.coverage_source:
        cmd.extend(["--source", ",".join(config.coverage_source)])
    cmd.extend(regrtest_cmd[len(host_cmd) :])
    return run_command(
        cmd, cwd=cwd, env=env, log_handle=log_handle, dry_run=config.dry_run
    )


def finalize_coverage(
    config: RegrtestConfig,
    host_cmd: list[str],
    log_handle,
) -> CoverageSummary | None:
    if config.dry_run:
        return None
    coverage_dir = config.coverage_dir
    data_file = coverage_dir / ".coverage"
    coverage_files = list(coverage_dir.glob(".coverage*"))
    if not coverage_files:
        return None
    cmd_combine = host_cmd + [
        "-m",
        "coverage",
        "combine",
        "--data-file",
        str(data_file),
        str(coverage_dir),
    ]
    _ = run_command(
        cmd_combine, cwd=None, env=None, log_handle=log_handle, dry_run=False
    )
    json_path = coverage_dir / "coverage.json"
    html_dir = coverage_dir / "html"
    cmd_json = host_cmd + [
        "-m",
        "coverage",
        "json",
        "-o",
        str(json_path),
        "--data-file",
        str(data_file),
    ]
    cmd_html = host_cmd + [
        "-m",
        "coverage",
        "html",
        "-d",
        str(html_dir),
        "--data-file",
        str(data_file),
    ]
    _ = run_command(cmd_json, cwd=None, env=None, log_handle=log_handle, dry_run=False)
    _ = run_command(cmd_html, cwd=None, env=None, log_handle=log_handle, dry_run=False)
    data = json.loads(json_path.read_text())
    files = data.get("files", {})
    coverage_files: dict[str, float] = {}
    total_percent = 0.0
    total_covered = 0
    total_statements = 0
    for filename, entry in files.items():
        summary = entry.get("summary", {})
        covered = int(summary.get("covered_lines", 0))
        statements = int(summary.get("num_statements", 0))
        percent = float(summary.get("percent_covered", 0.0))
        coverage_files[filename] = percent
        total_covered += covered
        total_statements += statements
    if total_statements:
        total_percent = (total_covered / total_statements) * 100.0
    return CoverageSummary(total_percent=total_percent, files=coverage_files)


def run_rust_coverage(
    config: RegrtestConfig,
    *,
    log_handle,
) -> RustCoverageSummary | None:
    if not config.rust_coverage:
        return None
    output_dir = config.rust_coverage_dir
    output_dir.mkdir(parents=True, exist_ok=True)
    cmd = [
        "cargo",
        "llvm-cov",
        "--workspace",
        "--html",
        "--output-dir",
        str(output_dir),
    ]
    if config.dry_run:
        log_line(log_handle, f"cmd: {shlex.join(cmd)}")
        return RustCoverageSummary(
            output_dir=output_dir,
            returncode=0,
            command=cmd,
            available=True,
            message="dry-run",
        )
    if shutil.which("cargo") is None:
        message = "cargo not found; skipping rust coverage"
        log_line(log_handle, message)
        return RustCoverageSummary(
            output_dir=output_dir,
            returncode=1,
            command=cmd,
            available=False,
            message=message,
        )
    check = subprocess.run(
        ["cargo", "llvm-cov", "--version"],
        capture_output=True,
        text=True,
        check=False,
    )
    if check.returncode != 0:
        message = (
            "cargo-llvm-cov not available (install via cargo install cargo-llvm-cov)"
        )
        log_line(log_handle, message)
        if check.stderr:
            log_line(log_handle, f"cargo-llvm-cov stderr: {check.stderr.strip()}")
        return RustCoverageSummary(
            output_dir=output_dir,
            returncode=check.returncode,
            command=cmd,
            available=False,
            message=message,
        )
    rc = run_command(
        cmd,
        cwd=config.repo_root,
        env=None,
        log_handle=log_handle,
        dry_run=False,
    )
    return RustCoverageSummary(
        output_dir=output_dir,
        returncode=rc,
        command=cmd,
        available=True,
        message=None if rc == 0 else "cargo llvm-cov failed",
    )


def load_stdlib_modules(version: str, source: str) -> list[str]:
    if source in ("auto", "stdlib-list"):
        try:
            from stdlib_list import stdlib_list

            return sorted(stdlib_list(version))
        except Exception:
            if source == "stdlib-list":
                raise
    if not hasattr(sys, "stdlib_module_names"):
        return []
    return sorted(sys.stdlib_module_names)


def parse_stdlib_matrix(path: Path) -> dict[str, dict[str, str]]:
    if not path.exists():
        return {}
    lines = path.read_text().splitlines()
    header_idx = None
    for idx, line in enumerate(lines):
        if line.strip().startswith("| Module | Tier | Status |"):
            header_idx = idx
            break
    if header_idx is None or header_idx + 2 >= len(lines):
        return {}
    headers = [cell.strip() for cell in lines[header_idx].strip().strip("|").split("|")]
    rows = {}
    for line in lines[header_idx + 2 :]:
        if not line.strip().startswith("|"):
            break
        cells = [cell.strip() for cell in line.strip().strip("|").split("|")]
        if len(cells) != len(headers):
            continue
        data = dict(zip(headers, cells))
        module_cell = data.get("Module", "")
        module_names = [name.strip() for name in module_cell.split("/") if name.strip()]
        for name in module_names:
            rows[name] = data
    return rows


def normalize_status(status: str) -> str:
    lowered = status.strip().lower()
    if lowered in {"supported", "implemented"}:
        return "supported"
    if lowered in {"partial"}:
        return "partial"
    return "unsupported"


def write_stdlib_matrix(
    config: RegrtestConfig,
    modules: Iterable[str],
    matrix: dict[str, dict[str, str]],
) -> tuple[Path | None, Path | None]:
    rows = []
    for name in sorted(set(modules)):
        meta = matrix.get(name, {})
        status = normalize_status(meta.get("Status", "Unsupported"))
        rows.append(
            {
                "module": name,
                "status": status,
                "tier": meta.get("Tier", ""),
                "priority": meta.get("Priority", ""),
                "milestone": meta.get("Milestone", ""),
                "owner": meta.get("Owner", ""),
                "notes": meta.get("Notes", ""),
            }
        )
    json_path = None
    csv_path = None
    if config.matrix_format in {"json", "both"}:
        json_path = config.output_dir / "stdlib_matrix.json"
        json_path.write_text(json.dumps(rows, indent=2, sort_keys=True))
    if config.matrix_format in {"csv", "both"}:
        csv_path = config.output_dir / "stdlib_matrix.csv"
        with csv_path.open("w", newline="") as handle:
            writer = csv.DictWriter(
                handle,
                fieldnames=[
                    "module",
                    "status",
                    "tier",
                    "priority",
                    "milestone",
                    "owner",
                    "notes",
                ],
            )
            writer.writeheader()
            writer.writerows(rows)
    return json_path, csv_path


def run_diff_suite(
    config: RegrtestConfig,
    host_cmd: list[str],
    python_version: str | None,
    *,
    env: dict[str, str],
    log_handle,
) -> DiffSummary | None:
    if not config.diff_enabled:
        return None
    diff_paths = config.diff_paths or []
    if not diff_paths:
        return None
    diff_version = config.diff_python_version
    total = passed = failed = 0
    failed_files: list[str] = []
    for idx, path in enumerate(diff_paths):
        json_path = config.output_dir / f"diff_{idx}.json"
        cmd = host_cmd + [
            str(config.repo_root / "tests" / "molt_diff.py"),
            str(path),
            "--json-output",
            str(json_path),
        ]
        if diff_version:
            cmd.extend(["--python-version", diff_version])
        rc = run_command(
            cmd,
            cwd=config.repo_root,
            env=env,
            log_handle=log_handle,
            dry_run=config.dry_run,
        )
        if json_path.exists():
            try:
                data = json.loads(json_path.read_text())
                total += int(data.get("total", 0))
                passed += int(data.get("passed", 0))
                failed += int(data.get("failed", 0))
                failed_files.extend(data.get("failed_files", []))
            except json.JSONDecodeError:
                failed += 1
        elif rc != 0:
            failed += 1
    diff_summary = {
        "total": total,
        "passed": passed,
        "failed": failed,
        "failed_files": sorted(set(failed_files)),
        "python_version": diff_version or python_version,
    }
    json_out = config.output_dir / "diff_summary.json"
    md_out = config.output_dir / "diff_summary.md"
    json_out.write_text(json.dumps(diff_summary, indent=2, sort_keys=True))
    md_lines = [
        "# Molt differential summary",
        "",
        "| Metric | Value |",
        "| --- | --- |",
        f"| Total | {total} |",
        f"| Passed | {passed} |",
        f"| Failed | {failed} |",
    ]
    if failed_files:
        md_lines.extend(["", "## Failed files", ""])
        md_lines.extend(f"- {name}" for name in sorted(set(failed_files)))
    md_lines.append("")
    md_out.write_text("\n".join(md_lines))
    return DiffSummary(
        total=total,
        passed=passed,
        failed=failed,
        failed_files=sorted(set(failed_files)),
        returncode=0 if failed == 0 else 1,
        json_path=json_out,
        md_path=md_out,
    )


def run_property_tests(
    config: RegrtestConfig,
    host_cmd: list[str],
    *,
    env: dict[str, str],
    log_handle,
) -> int:
    if config.property_tests is None:
        return 0
    if not config.property_tests.exists():
        log_line(log_handle, f"property tests missing: {config.property_tests}")
        return 1
    cmd = host_cmd + [
        "-m",
        "pytest",
        str(config.property_tests),
    ]
    return run_command(
        cmd,
        cwd=config.repo_root,
        env=env,
        log_handle=log_handle,
        dry_run=config.dry_run,
    )


def write_summary(
    config: RegrtestConfig,
    summary: RegrtestSummary | None,
    coverage: CoverageSummary | None,
    stdlib_paths: tuple[Path | None, Path | None],
    python_version: str | None,
    returncode: int,
    diff_summary: DiffSummary | None,
    matrix_report: MatrixReport,
    rust_coverage: RustCoverageSummary | None,
) -> None:
    payload = {
        "python_version": python_version,
        "tests": summary.tests if summary else 0,
        "failures": summary.failures if summary else 0,
        "errors": summary.errors if summary else 0,
        "skipped": summary.skipped if summary else 0,
        "failed_modules": summary.failed_modules if summary else [],
        "coverage_total": coverage.total_percent if coverage else None,
        "stdlib_matrix": {
            "json": str(stdlib_paths[0]) if stdlib_paths[0] else None,
            "csv": str(stdlib_paths[1]) if stdlib_paths[1] else None,
        },
        "type_semantics_matrix": {
            "json": str(matrix_report.json_path),
            "md": str(matrix_report.md_path),
            "summary": matrix_report.summary,
        },
        "returncode": returncode,
    }
    if diff_summary is not None:
        payload["diff_summary"] = {
            "total": diff_summary.total,
            "passed": diff_summary.passed,
            "failed": diff_summary.failed,
            "failed_files": diff_summary.failed_files,
            "json": str(diff_summary.json_path) if diff_summary.json_path else None,
            "md": str(diff_summary.md_path) if diff_summary.md_path else None,
        }
    if rust_coverage is not None:
        payload["rust_coverage"] = {
            "output_dir": str(rust_coverage.output_dir),
            "returncode": rust_coverage.returncode,
            "available": rust_coverage.available,
            "message": rust_coverage.message,
            "command": rust_coverage.command,
        }
    summary_path = config.output_dir / "summary.json"
    summary_path.write_text(json.dumps(payload, indent=2, sort_keys=True))
    md_path = config.output_dir / "summary.md"
    lines = [
        "# CPython regrtest summary",
        "",
        "| Metric | Value |",
        "| --- | --- |",
        f"| Python | {python_version or 'unknown'} |",
        f"| Tests | {payload['tests']} |",
        f"| Failures | {payload['failures']} |",
        f"| Errors | {payload['errors']} |",
        f"| Skipped | {payload['skipped']} |",
    ]
    if coverage:
        lines.append(f"| Coverage total | {coverage.total_percent:.2f}% |")
    if summary and summary.failed_modules:
        lines.extend(
            [
                "",
                "## Failed modules",
                "",
            ]
        )
        lines.extend(f"- {name}" for name in summary.failed_modules)
    if diff_summary is not None:
        lines.extend(
            [
                "",
                "## Differential summary",
                "",
                f"- Total: {diff_summary.total}",
                f"- Passed: {diff_summary.passed}",
                f"- Failed: {diff_summary.failed}",
            ]
        )
    if rust_coverage is not None:
        rust_state = "ok" if rust_coverage.returncode == 0 else "failed"
        if not rust_coverage.available:
            rust_state = "missing"
        lines.extend(
            [
                "",
                "## Rust coverage",
                "",
                f"- Status: {rust_state}",
                f"- Output: {rust_coverage.output_dir}",
            ]
        )
    lines.append("")
    md_path.write_text("\n".join(lines))


def write_root_summary(output_root: Path, runs: list[dict]) -> None:
    payload = {
        "runs": runs,
    }
    summary_path = output_root / "summary.json"
    summary_path.write_text(json.dumps(payload, indent=2, sort_keys=True))
    md_path = output_root / "summary.md"
    lines = [
        "# CPython regrtest summary (all runs)",
        "",
        "| Python | Tests | Failures | Errors | Skipped | Coverage | Diff Failed | Rust Cov | Return |",
        "| --- | --- | --- | --- | --- | --- | --- | --- | --- |",
    ]
    for run in runs:
        coverage = run.get("coverage_total")
        coverage_text = (
            f"{coverage:.2f}%" if isinstance(coverage, (float, int)) else "-"
        )
        diff_failed = "-"
        diff_summary = run.get("diff_summary") or {}
        if isinstance(diff_summary, dict) and "failed" in diff_summary:
            diff_failed = str(diff_summary.get("failed", "-"))
        rust_cov = "-"
        rust_summary = run.get("rust_coverage") or {}
        if isinstance(rust_summary, dict) and "returncode" in rust_summary:
            rust_cov = "ok" if rust_summary.get("returncode", 1) == 0 else "failed"
            if rust_summary.get("available") is False:
                rust_cov = "missing"
        lines.append(
            "| {python} | {tests} | {failures} | {errors} | {skipped} | {coverage} | {diff_failed} | {rust_cov} | {rc} |".format(
                python=run.get("python_version") or "host",
                tests=run.get("tests", 0),
                failures=run.get("failures", 0),
                errors=run.get("errors", 0),
                skipped=run.get("skipped", 0),
                coverage=coverage_text,
                diff_failed=diff_failed,
                rust_cov=rust_cov,
                rc=run.get("returncode", 0),
            )
        )
    lines.append("")
    md_path.write_text("\n".join(lines))


def build_env(config: RegrtestConfig) -> dict[str, str]:
    env = os.environ.copy()
    env.setdefault("PYTHONHASHSEED", "0")
    if config.molt_capabilities is not None:
        env["MOLT_CAPABILITIES"] = config.molt_capabilities
    return env


def validate_molt_cmd(config: RegrtestConfig, *, log_handle) -> None:
    if not config.molt_cmd:
        raise ValueError("molt-cmd is empty; provide --molt-cmd")
    exe = config.molt_cmd[0]
    if Path(exe).is_file():
        return
    if shutil.which(exe) is not None:
        return
    message = (
        f"molt-cmd not found: {exe}. "
        "Provide --molt-cmd (e.g. 'python -m molt.cli run --compiled') "
        "or ensure the command is on PATH."
    )
    log_line(log_handle, message)
    raise FileNotFoundError(message)


def run_regrtest(
    config: RegrtestConfig,
    python_version: str | None,
) -> tuple[int, Path]:
    run_label = f"py{python_version}" if python_version else "host"
    output_dir = config.output_root / run_label
    output_dir.mkdir(parents=True, exist_ok=True)
    cpython_dir = config.cpython_dir
    cpython_branch = config.cpython_branch
    if python_version:
        if python_version not in cpython_dir.name:
            cpython_dir = cpython_dir.with_name(f"{cpython_dir.name}-{python_version}")
            cpython_branch = f"v{python_version}.x"
    run_config = replace(
        config,
        output_dir=output_dir,
        junit_xml=output_dir / "junit.xml",
        coverage_dir=output_dir / "coverage",
        rust_coverage_dir=output_dir / "rust_coverage",
        cpython_dir=cpython_dir,
        cpython_branch=cpython_branch,
    )
    log_path = output_dir / "regrtest.log"
    with log_path.open("w", encoding="utf-8") as log_handle:
        log_line(log_handle, f"output_dir={output_dir}")
        if run_config.core_only and not run_config.core_file.exists():
            raise FileNotFoundError(f"core file missing: {run_config.core_file}")
        validate_molt_cmd(run_config, log_handle=log_handle)
        ensure_cpython_checkout(
            run_config.cpython_dir,
            run_config.cpython_branch,
            allow_clone=run_config.allow_clone,
            log_handle=log_handle,
            dry_run=run_config.dry_run,
        )
        if not run_config.molt_shim_path.exists():
            raise FileNotFoundError(
                f"molt regrtest shim missing: {run_config.molt_shim_path}"
            )
        regrtest_path = run_config.cpython_dir / "Lib" / "test" / "regrtest.py"
        if not regrtest_path.exists():
            raise FileNotFoundError(f"regrtest.py not found: {regrtest_path}")
        skip_modules = load_skip_list(run_config.skip_file)
        host_cmd = host_python_cmd(run_config, python_version)
        regrtest_cmd = build_regrtest_cmd(
            run_config,
            regrtest_path,
            skip_modules,
            host_cmd,
        )
        env = build_env(run_config)
        cpython_lib = run_config.cpython_dir / "Lib"
        existing_path = env.get("PYTHONPATH", "")
        if existing_path:
            env["PYTHONPATH"] = os.pathsep.join([str(cpython_lib), existing_path])
        else:
            env["PYTHONPATH"] = str(cpython_lib)
        if run_config.enable_coverage:
            run_config.coverage_dir.mkdir(parents=True, exist_ok=True)
            env["MOLT_REGRTEST_COVERAGE"] = "1"
            env["MOLT_REGRTEST_COVERAGE_DIR"] = str(run_config.coverage_dir)
            if run_config.coverage_source:
                env["MOLT_REGRTEST_COVERAGE_SOURCE"] = ",".join(
                    run_config.coverage_source
                )
            regrtest_rc = run_coverage(
                run_config,
                regrtest_cmd,
                host_cmd,
                cwd=run_config.cpython_dir,
                env=env,
                log_handle=log_handle,
            )
        else:
            regrtest_rc = run_command(
                regrtest_cmd,
                cwd=run_config.cpython_dir,
                env=env,
                log_handle=log_handle,
                dry_run=run_config.dry_run,
            )
        summary = None
        if run_config.junit_xml.exists():
            summary = parse_junit(run_config.junit_xml)
            summary.returncode = regrtest_rc
        coverage = None
        if run_config.enable_coverage:
            coverage = finalize_coverage(run_config, host_cmd, log_handle)
        diff_summary = run_diff_suite(
            run_config,
            host_cmd,
            python_version,
            env=env,
            log_handle=log_handle,
        )
        modules = load_stdlib_modules(
            run_config.stdlib_version, run_config.stdlib_source
        )
        matrix = parse_stdlib_matrix(run_config.matrix_path)
        stdlib_paths = write_stdlib_matrix(run_config, modules, matrix)
        matrix_report = write_type_semantics_report(run_config)
        prop_rc = run_property_tests(
            run_config, host_cmd, env=env, log_handle=log_handle
        )
        rust_summary = run_rust_coverage(run_config, log_handle=log_handle)
        diff_rc = diff_summary.returncode if diff_summary else 0
        rust_rc = rust_summary.returncode if rust_summary else 0
        final_rc = max(regrtest_rc, prop_rc, diff_rc, rust_rc)
        if summary:
            summary.returncode = final_rc
        write_summary(
            run_config,
            summary,
            coverage,
            stdlib_paths,
            python_version,
            final_rc,
            diff_summary,
            matrix_report,
            rust_summary,
        )
        summary_path = run_config.output_dir / "summary.json"
        return final_rc, summary_path


def main(argv: list[str] | None = None) -> int:
    config = parse_args(argv or sys.argv[1:])
    if config.uv_prepare and not config.use_uv:
        raise ValueError("--uv-prepare requires --uv")
    config.output_root.mkdir(parents=True, exist_ok=True)
    python_versions = list(dict.fromkeys(config.uv_python)) if config.use_uv else [None]
    prepare_log = config.output_root / "prepare.log"
    with prepare_log.open("w", encoding="utf-8") as log_handle:
        log_line(log_handle, "phase=prepare")
        prepare_uv(config, python_versions, log_handle=log_handle)
    returncodes = []
    runs: list[dict] = []
    for version in python_versions:
        rc, summary_path = run_regrtest(config, version)
        returncodes.append(rc)
        if summary_path.exists():
            try:
                data = json.loads(summary_path.read_text())
                runs.append(data)
            except json.JSONDecodeError:
                runs.append(
                    {
                        "python_version": version,
                        "returncode": rc,
                        "summary_path": str(summary_path),
                    }
                )
        else:
            runs.append(
                {
                    "python_version": version,
                    "returncode": rc,
                    "summary_path": str(summary_path),
                }
            )
    write_root_summary(config.output_root, runs)
    return max(returncodes) if returncodes else 1


if __name__ == "__main__":
    raise SystemExit(main())
