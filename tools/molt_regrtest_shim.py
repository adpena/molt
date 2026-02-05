#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import time
from xml.sax.saxutils import escape
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[1]


def parse_args(argv: list[str]) -> tuple[argparse.Namespace, list[str]]:
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument(
        "--molt-cmd",
        default=os.environ.get(
            "MOLT_REGRTEST_MOLT_CMD", f"{sys.executable} -m molt.cli run"
        ),
        help="Command used to run a test file (shell-like string).",
    )
    parser.add_argument(
        "--cpython-dir",
        type=Path,
        default=os.environ.get("MOLT_REGRTEST_CPYTHON_DIR", ""),
        help="Path to CPython checkout.",
    )
    return parser.parse_known_args(argv)


def split_python_flags(args: list[str]) -> tuple[list[str], list[str]]:
    if "-m" in args:
        idx = args.index("-m")
        return args[:idx], args[idx:]
    return [], args


def find_python_index(cmd: list[str]) -> int | None:
    for idx, arg in enumerate(cmd):
        name = Path(arg).name
        if name.startswith("python"):
            return idx
    return None


def coverage_args_from_env() -> list[str]:
    if os.environ.get("MOLT_REGRTEST_COVERAGE") != "1":
        return []
    coverage_dir = os.environ.get("MOLT_REGRTEST_COVERAGE_DIR")
    if not coverage_dir:
        return []
    data_file = Path(coverage_dir) / ".coverage"
    data_file.parent.mkdir(parents=True, exist_ok=True)
    args = ["--parallel-mode", "--data-file", str(data_file)]
    source = os.environ.get("MOLT_REGRTEST_COVERAGE_SOURCE")
    if source:
        args.extend(["--source", source])
    return args


def wrap_python_cmd(
    cmd: list[str], python_flags: list[str], coverage_args: list[str]
) -> tuple[list[str], bool]:
    idx = find_python_index(cmd)
    if idx is None:
        return cmd, False
    prefix = cmd[: idx + 1]
    suffix = cmd[idx + 1 :]
    wrapped = prefix + python_flags
    if coverage_args:
        wrapped.extend(["-m", "coverage", "run"])
        wrapped.extend(coverage_args)
    wrapped.extend(suffix)
    return wrapped, True


def resolve_cpython_dir(parsed_dir: Path) -> Path | None:
    if parsed_dir and Path(parsed_dir).exists():
        return Path(parsed_dir)
    env_path = os.environ.get("PYTHONPATH", "")
    for entry in env_path.split(os.pathsep):
        if not entry:
            continue
        path = Path(entry)
        if path.name == "Lib" and (path / "test").exists():
            return path.parent
    repo_root = Path(__file__).resolve().parents[1]
    fallback = repo_root / "third_party" / "cpython"
    if fallback.exists():
        return fallback
    return None


def build_env(cpython_dir: Path) -> dict[str, str]:
    env = os.environ.copy()
    env["PYTHONHASHSEED"] = "0"
    env["MOLT_PROJECT_ROOT"] = str(REPO_ROOT)
    env["MOLT_REGRTEST_CPYTHON_DIR"] = str(cpython_dir)
    repo_src = str(REPO_ROOT / "src")
    cpython_lib = Path(cpython_dir / "Lib").resolve()
    extra_roots = env.get("MOLT_MODULE_ROOTS", "")
    if extra_roots:
        env["MOLT_MODULE_ROOTS"] = os.pathsep.join([str(cpython_lib), extra_roots])
    else:
        env["MOLT_MODULE_ROOTS"] = str(cpython_lib)
    existing = env.get("PYTHONPATH", "")
    entries = []
    for entry in existing.split(os.pathsep):
        if not entry:
            continue
        try:
            resolved = Path(entry).resolve()
        except OSError:
            entries.append(entry)
            continue
        if resolved == cpython_lib or str(resolved).startswith(
            str(cpython_lib) + os.sep
        ):
            continue
        entries.append(entry)
    if entries:
        env["PYTHONPATH"] = os.pathsep.join([repo_src, *entries])
    else:
        env["PYTHONPATH"] = repo_src
    return env


def resolve_test_path(cpython_dir: Path, test_name: str) -> Path | None:
    name = test_name
    if name.startswith("test."):
        name = name[len("test.") :]
    module_path = cpython_dir / "Lib" / "test" / Path(*name.split("."))
    candidate = module_path.with_suffix(".py")
    if candidate.exists():
        return candidate
    if module_path.is_dir():
        nested = module_path / f"{module_path.name}.py"
        if nested.exists():
            return nested
        init_py = module_path / "__init__.py"
        if init_py.exists():
            return init_py
    return None


def module_name_from_test_path(cpython_dir: Path, test_path: Path) -> str | None:
    lib_root = (cpython_dir / "Lib").resolve()
    try:
        rel = test_path.resolve().relative_to(lib_root)
    except ValueError:
        return None
    if rel.name == "__init__.py":
        rel = rel.parent
    else:
        rel = rel.with_suffix("")
    if not rel.parts:
        return None
    return ".".join(rel.parts)


def build_junit_xml(
    test_name: str,
    duration: float | None,
    state: str,
    stdout: str,
    stderr: str,
    skip_reason: str | None = None,
) -> str:
    duration_text = f"{duration:.6f}" if duration is not None else "0.000000"
    escaped_name = escape(test_name)
    errors = 1 if state not in {"PASSED", "SKIPPED"} else 0
    skipped = 1 if state == "SKIPPED" else 0
    testcase = f'<testcase name="{escaped_name}" time="{duration_text}">'
    if state == "SKIPPED":
        reason = skip_reason or "skipped"
        escaped_reason = escape(reason)
        testcase += f'<skipped message="{escaped_reason}">{escaped_reason}</skipped>'
    elif state != "PASSED":
        detail = escape(f"molt failed\nstdout:\n{stdout}\nstderr:\n{stderr}")
        testcase += f'<error message="molt failed">{detail}</error>'
    testcase += "</testcase>"
    return (
        f'<testsuite tests="1" errors="{errors}" failures="0" '
        f'skipped="{skipped}">'
        f"{testcase}</testsuite>"
    )


def compat_skip_reason(stdout: str, stderr: str) -> str | None:
    text = "\n".join([stdout, stderr])
    if "MOLT_COMPAT_ERROR:" not in text:
        return None
    feature = None
    location = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("feature:"):
            feature = stripped[len("feature:") :].strip()
        elif stripped.startswith("location:"):
            location = stripped[len("location:") :].strip()
    if feature and location:
        return f"MOLT_COMPAT_ERROR: {feature} @ {location}"
    if feature:
        return f"MOLT_COMPAT_ERROR: {feature}"
    if location:
        return f"MOLT_COMPAT_ERROR: {location}"
    return "MOLT_COMPAT_ERROR"


def run_molt_test(
    molt_cmd: str,
    cpython_dir: Path,
    test_name: str,
    python_flags: list[str],
) -> dict:
    test_path = resolve_test_path(cpython_dir, test_name)
    if test_path is None:
        fallback = cpython_dir / "Lib" / "test" / f"{test_name}.py"
        print(f"molt regrtest shim: test not found: {fallback}", file=sys.stderr)
        xml_data = build_junit_xml(test_name, None, "WORKER_FAILED", "", "")
        result = {
            "__test_result__": "TestResult",
            "test_name": test_name,
            "state": "WORKER_FAILED",
            "duration": None,
            "xml_data": [xml_data],
            "stats": None,
            "errors": [(test_name, f"test not found: {fallback}")],
            "failures": None,
        }
        if sys.version_info >= (3, 13):
            result["covered_lines"] = None
        return result
    cmd = shlex.split(molt_cmd)
    coverage_args = coverage_args_from_env()
    cmd, wrapped = wrap_python_cmd(cmd, python_flags, coverage_args)
    if (python_flags or coverage_args) and not wrapped:
        print(
            "molt regrtest shim: unable to apply python flags/coverage; "
            "non-python molt-cmd",
            file=sys.stderr,
        )
    module_name = module_name_from_test_path(cpython_dir, test_path)
    if module_name and "--module" not in cmd:
        cmd.extend(["--module", module_name])
    else:
        cmd.append(str(test_path))
    env = build_env(cpython_dir)
    start = time.perf_counter()
    result = subprocess.run(
        cmd,
        env=env,
        cwd=REPO_ROOT,
        text=True,
        capture_output=True,
    )
    duration = time.perf_counter() - start
    if result.returncode == 0:
        state = "PASSED"
        errors = None
        skip_reason = None
    else:
        skip_reason = compat_skip_reason(result.stdout, result.stderr)
        if skip_reason:
            state = "SKIPPED"
            errors = None
        else:
            state = "FAILED"
            errors = [
                (
                    test_name,
                    f"molt rc={result.returncode}\nstdout:\n{result.stdout}\n"
                    f"stderr:\n{result.stderr}",
                )
            ]
    xml_data = build_junit_xml(
        test_name,
        duration,
        state,
        result.stdout,
        result.stderr,
        skip_reason=skip_reason,
    )
    result = {
        "__test_result__": "TestResult",
        "test_name": test_name,
        "state": state,
        "duration": duration,
        "xml_data": [xml_data],
        "stats": None,
        "errors": errors,
        "failures": None,
    }
    if sys.version_info >= (3, 13):
        result["covered_lines"] = None
    return result


def run_worker(
    worker_json: str,
    molt_cmd: str,
    cpython_dir: Path,
    python_flags: list[str],
) -> int:
    try:
        payload = json.loads(worker_json)
    except json.JSONDecodeError as exc:
        print(f"molt regrtest shim: invalid worker json: {exc}", file=sys.stderr)
        return 1
    tests = payload.get("tests") or []
    if not tests:
        print("molt regrtest shim: no tests in worker json", file=sys.stderr)
        return 1
    test_name = tests[0]
    result = run_molt_test(molt_cmd, cpython_dir, test_name, python_flags)
    sys.stdout.write("\n")
    sys.stdout.write(json.dumps(result))
    sys.stdout.flush()
    return 0


def passthrough(args: list[str]) -> int:
    cmd = [sys.executable, *args]
    result = subprocess.run(cmd, text=True)
    return result.returncode


def main() -> int:
    ns, rest = parse_args(sys.argv[1:])
    rest_raw = rest
    python_flags, rest = split_python_flags(rest)
    if "-m" in rest:
        idx = rest.index("-m")
        if idx + 1 >= len(rest):
            print("molt regrtest shim: missing module after -m", file=sys.stderr)
            return 1
        module = rest[idx + 1]
        if module == "test.libregrtest.worker":
            if idx + 2 >= len(rest):
                print("molt regrtest shim: missing worker json", file=sys.stderr)
                return 1
            cpython_dir = resolve_cpython_dir(ns.cpython_dir)
            if cpython_dir is None:
                print("molt regrtest shim: cpython dir not found", file=sys.stderr)
                return 1
            return run_worker(rest[idx + 2], ns.molt_cmd, cpython_dir, python_flags)
    return passthrough(rest_raw)


if __name__ == "__main__":
    raise SystemExit(main())
