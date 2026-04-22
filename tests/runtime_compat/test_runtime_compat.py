#!/usr/bin/env python3
"""Runtime compatibility test harness for molt.

Verifies that libraries not only compile but actually EXECUTE correctly
by comparing molt-compiled output against CPython output.

Usage:
    python tests/runtime_compat/test_runtime_compat.py attrs
    python tests/runtime_compat/test_runtime_compat.py --all
    python tests/runtime_compat/test_runtime_compat.py --all --summary
    python tests/runtime_compat/test_runtime_compat.py attrs click six --verbose
"""

import argparse
import difflib
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

SCRIPTS_DIR = Path(__file__).parent / "scripts"
REPO_ROOT = Path(__file__).resolve().parents[2]

# Timeouts in seconds
CPYTHON_TIMEOUT = 30
MOLT_BUILD_TIMEOUT = 120
MOLT_RUN_TIMEOUT = 30


def discover_libraries() -> list[str]:
    """Find all test scripts and extract library names."""
    libs = []
    for p in sorted(SCRIPTS_DIR.glob("test_*.py")):
        name = p.stem.removeprefix("test_")
        libs.append(name)
    return libs


def find_site_packages() -> str:
    """Locate the venv site-packages for --lib-path."""
    venv_sp = REPO_ROOT / ".venv" / "lib"
    if venv_sp.exists():
        for d in venv_sp.iterdir():
            sp = d / "site-packages"
            if sp.is_dir():
                return str(sp)
    # Fallback: ask the current interpreter
    import site

    for sp in site.getsitepackages():
        if os.path.isdir(sp):
            return sp
    return ""


def run_cpython(script: Path, timeout: float = CPYTHON_TIMEOUT) -> tuple[str, int]:
    """Run a script under CPython, return (output, exit_code)."""
    try:
        result = subprocess.run(
            [sys.executable, str(script)],
            capture_output=True,
            text=True,
            timeout=timeout,
            cwd=str(REPO_ROOT),
        )
        output = result.stdout + result.stderr
        return output, result.returncode
    except subprocess.TimeoutExpired:
        return "<TIMEOUT>\n", -1
    except Exception as e:
        return f"<ERROR: {e}>\n", -1


def _find_luau_binary() -> str | None:
    """Return the name of the Luau runner on PATH, or None."""
    for name in ("luau", "lune"):
        if shutil.which(name):
            return name
    return None


def run_molt(
    script: Path,
    lib_path: str,
    target: str = "native",
    build_timeout: float = MOLT_BUILD_TIMEOUT,
    run_timeout: float = MOLT_RUN_TIMEOUT,
    verbose: bool = False,
) -> tuple[str, int, str]:
    """Compile with molt and run. Returns (output, exit_code, build_log)."""
    with tempfile.TemporaryDirectory(prefix="molt_compat_") as tmpdir:
        binary_path = os.path.join(tmpdir, script.stem)

        # Build
        build_cmd = [
            sys.executable,
            "-m",
            "molt",
            "build",
            str(script),
            "--output",
            binary_path,
        ]
        if lib_path:
            build_cmd.extend(["--lib-path", lib_path])
        if target != "native":
            build_cmd.extend(["--target", target])

        try:
            build_result = subprocess.run(
                build_cmd,
                capture_output=True,
                text=True,
                timeout=build_timeout,
                cwd=str(REPO_ROOT),
            )
        except subprocess.TimeoutExpired:
            return "<BUILD TIMEOUT>\n", -1, ""
        except Exception as e:
            return f"<BUILD ERROR: {e}>\n", -1, ""

        build_log = build_result.stdout + build_result.stderr

        if build_result.returncode != 0:
            return f"<BUILD FAILED (exit {build_result.returncode})>\n", -1, build_log

        # Find the output artifact — molt may add an extension or place it differently
        output_path = Path(binary_path)
        if not output_path.exists():
            # Check for common patterns
            candidates = list(Path(tmpdir).glob(f"{script.stem}*"))
            if candidates:
                output_path = candidates[0]
            else:
                return "<BINARY NOT FOUND>\n", -1, build_log

        # WASM target: skip execution (needs a host runtime)
        if target == "wasm":
            return (
                "<WASM BUILD OK — execution skipped (needs host runtime)>\n",
                0,
                build_log,
            )

        # Luau target: run via luau/lune binary
        if target == "luau":
            luau_bin = _find_luau_binary()
            if luau_bin is None:
                return (
                    "<SKIP: neither 'luau' nor 'lune' found on PATH>\n",
                    -1,
                    build_log,
                )
            run_cmd = [luau_bin, str(output_path)]
        else:
            run_cmd = [str(output_path)]

        # Run
        try:
            run_result = subprocess.run(
                run_cmd,
                capture_output=True,
                text=True,
                timeout=run_timeout,
                cwd=str(REPO_ROOT),
            )
            output = run_result.stdout + run_result.stderr
            return output, run_result.returncode, build_log
        except subprocess.TimeoutExpired:
            return "<RUN TIMEOUT>\n", -1, build_log
        except PermissionError:
            # Try making it executable
            os.chmod(str(output_path), 0o755)
            try:
                run_result = subprocess.run(
                    run_cmd,
                    capture_output=True,
                    text=True,
                    timeout=run_timeout,
                    cwd=str(REPO_ROOT),
                )
                output = run_result.stdout + run_result.stderr
                return output, run_result.returncode, build_log
            except Exception as e:
                return f"<RUN ERROR: {e}>\n", -1, build_log
        except Exception as e:
            return f"<RUN ERROR: {e}>\n", -1, build_log


def diff_output(cpython_out: str, molt_out: str) -> str:
    """Return a unified diff between CPython and molt output."""
    cp_lines = cpython_out.splitlines(keepends=True)
    mo_lines = molt_out.splitlines(keepends=True)
    diff = list(
        difflib.unified_diff(cp_lines, mo_lines, fromfile="cpython", tofile="molt")
    )
    return "".join(diff)


def test_library(
    lib: str,
    lib_path: str,
    target: str = "native",
    verbose: bool = False,
) -> dict:
    """Test a single library. Returns a result dict."""
    script = SCRIPTS_DIR / f"test_{lib}.py"
    if not script.exists():
        return {
            "lib": lib,
            "status": "SKIP",
            "reason": f"No test script: {script}",
        }

    result = {"lib": lib}

    # CPython
    t0 = time.monotonic()
    cp_out, cp_code = run_cpython(script)
    result["cpython_time"] = time.monotonic() - t0
    result["cpython_exit"] = cp_code
    result["cpython_output"] = cp_out

    if cp_code != 0:
        result["status"] = "SKIP"
        result["reason"] = f"CPython failed (exit {cp_code})"
        if verbose:
            result["cpython_detail"] = cp_out
        return result

    # Molt
    t0 = time.monotonic()
    mo_out, mo_code, build_log = run_molt(
        script, lib_path, target=target, verbose=verbose
    )
    result["molt_time"] = time.monotonic() - t0
    result["molt_exit"] = mo_code
    result["molt_output"] = mo_out
    result["build_log"] = build_log

    if mo_code == -1 and mo_out.startswith("<BUILD"):
        result["status"] = "BUILD_FAIL"
        result["reason"] = mo_out.strip()
        return result

    if mo_code == -1:
        result["status"] = "RUN_FAIL"
        result["reason"] = mo_out.strip()
        return result

    # Compare
    if cp_out == mo_out and cp_code == mo_code:
        result["status"] = "PASS"
    else:
        result["status"] = "FAIL"
        result["diff"] = diff_output(cp_out, mo_out)
        if cp_code != mo_code:
            result["reason"] = f"Exit codes differ: cpython={cp_code} molt={mo_code}"

    return result


def print_result(r: dict, verbose: bool = False) -> None:
    """Print a single test result."""
    status = r["status"]
    lib = r["lib"]

    status_markers = {
        "PASS": "\033[32mPASS\033[0m",
        "FAIL": "\033[31mFAIL\033[0m",
        "BUILD_FAIL": "\033[33mBUILD_FAIL\033[0m",
        "RUN_FAIL": "\033[33mRUN_FAIL\033[0m",
        "SKIP": "\033[90mSKIP\033[0m",
    }
    marker = status_markers.get(status, status)

    times = ""
    if "cpython_time" in r and "molt_time" in r:
        times = f"  (cpython {r['cpython_time']:.2f}s, molt {r['molt_time']:.2f}s)"
    elif "cpython_time" in r:
        times = f"  (cpython {r['cpython_time']:.2f}s)"

    print(f"  [{marker}] {lib}{times}")

    if r.get("reason") and (verbose or status not in ("PASS", "SKIP")):
        print(f"         {r['reason']}")

    if verbose and status == "FAIL" and r.get("diff"):
        for line in r["diff"].splitlines():
            print(f"         {line}")

    if verbose and status in ("BUILD_FAIL", "RUN_FAIL") and r.get("build_log"):
        for line in r["build_log"].splitlines()[-10:]:
            print(f"         {line}")


def print_summary(results: list[dict]) -> None:
    """Print a summary table."""
    pass_count = sum(1 for r in results if r["status"] == "PASS")
    fail_count = sum(1 for r in results if r["status"] == "FAIL")
    build_fail = sum(1 for r in results if r["status"] == "BUILD_FAIL")
    run_fail = sum(1 for r in results if r["status"] == "RUN_FAIL")
    skip_count = sum(1 for r in results if r["status"] == "SKIP")
    total = len(results)

    print()
    print("=" * 60)
    print("  Runtime Compatibility Results")
    print(
        f"  PASS: {pass_count}  FAIL: {fail_count}  BUILD_FAIL: {build_fail}  RUN_FAIL: {run_fail}  SKIP: {skip_count}  TOTAL: {total}"
    )
    print("=" * 60)

    if fail_count > 0:
        print("\n  Failed (output differs from CPython):")
        for r in results:
            if r["status"] == "FAIL":
                print(f"    - {r['lib']}")

    if build_fail > 0:
        print("\n  Build failures:")
        for r in results:
            if r["status"] == "BUILD_FAIL":
                print(f"    - {r['lib']}: {r.get('reason', '?')}")

    if run_fail > 0:
        print("\n  Runtime failures:")
        for r in results:
            if r["status"] == "RUN_FAIL":
                print(f"    - {r['lib']}: {r.get('reason', '?')}")


def main():
    parser = argparse.ArgumentParser(
        description="Molt runtime compatibility test harness"
    )
    parser.add_argument("libraries", nargs="*", help="Library names to test")
    parser.add_argument(
        "--all", action="store_true", help="Test all discovered libraries"
    )
    parser.add_argument(
        "--verbose", "-v", action="store_true", help="Show detailed output"
    )
    parser.add_argument(
        "--summary", "-s", action="store_true", help="Show only the summary"
    )
    parser.add_argument("--lib-path", default=None, help="Override site-packages path")
    parser.add_argument(
        "--target",
        default="native",
        choices=["native", "luau", "wasm"],
        help="Compilation target backend (default: native)",
    )
    args = parser.parse_args()

    if not args.libraries and not args.all:
        parser.print_help()
        sys.exit(1)

    if args.all:
        libs = discover_libraries()
    else:
        libs = args.libraries

    if not libs:
        print("No libraries to test.")
        sys.exit(1)

    lib_path = args.lib_path or find_site_packages()
    if not lib_path:
        print(
            "WARNING: Could not find site-packages. Builds may fail.", file=sys.stderr
        )

    target = args.target

    # Pre-flight check for luau target
    if target == "luau" and _find_luau_binary() is None:
        print("ERROR: --target luau requires 'luau' or 'lune' on PATH", file=sys.stderr)
        sys.exit(1)

    print(f"Testing {len(libs)} libraries against CPython {sys.version.split()[0]}")
    print(f"  target:   {target}")
    print(f"  lib-path: {lib_path}")
    print()

    results = []
    for lib in libs:
        r = test_library(lib, lib_path, target=target, verbose=args.verbose)
        results.append(r)
        if not args.summary:
            print_result(r, verbose=args.verbose)

    print_summary(results)

    # Exit with failure if any tests failed
    has_failures = any(r["status"] in ("FAIL", "RUN_FAIL") for r in results)
    sys.exit(1 if has_failures else 0)


if __name__ == "__main__":
    main()
