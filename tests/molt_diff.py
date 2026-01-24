import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path


def _resolve_python_exe(python_exe: str) -> str:
    if not python_exe:
        return sys.executable
    if os.sep in python_exe or Path(python_exe).is_absolute():
        candidate = Path(python_exe)
        if candidate.exists():
            return python_exe
        base_exe = getattr(sys, "_base_executable", "")
        if base_exe and Path(base_exe).exists():
            return base_exe
    return python_exe


def _collect_env_overrides(file_path: str) -> dict[str, str]:
    overrides: dict[str, str] = {}
    try:
        text = Path(file_path).read_text()
    except OSError:
        return overrides
    for line in text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("# MOLT_ENV:"):
            continue
        payload = stripped[len("# MOLT_ENV:") :].strip()
        for token in payload.split():
            if "=" not in token:
                continue
            key, value = token.split("=", 1)
            overrides[key] = value
    return overrides


def run_cpython(file_path, python_exe=sys.executable):
    python_exe = _resolve_python_exe(python_exe)
    env = os.environ.copy()
    paths = [env.get("PYTHONPATH", ""), ".", "src"]
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    env["PYTHONHASHSEED"] = "0"
    env.update(_collect_env_overrides(file_path))
    bootstrap = (
        "import runpy, sys; "
        "import molt.shims as shims; "
        "shims.install(); "
        "runpy.run_path(sys.argv[1], run_name='__main__')"
    )
    result = subprocess.run(
        [python_exe, "-c", bootstrap, file_path],
        capture_output=True,
        text=True,
        env=env,
    )
    return result.stdout, result.stderr, result.returncode


def run_molt(file_path):
    output_root = Path(tempfile.mkdtemp(prefix="molt_diff_"))
    output_binary = output_root / f"{Path(file_path).stem}_molt"

    # Build
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    env["PYTHONHASHSEED"] = "0"
    env.update(_collect_env_overrides(file_path))
    try:
        build_cmd = [
            sys.executable,
            "-m",
            "molt.cli",
            "build",
            file_path,
            "--out-dir",
            str(output_root),
        ]
        codec = env.get("MOLT_CODEC")
        if codec:
            build_cmd.extend(["--codec", codec])
        build_res = subprocess.run(
            build_cmd,
            env=env,
            capture_output=True,
            text=True,
        )
        if build_res.returncode != 0:
            return None, build_res.stderr, build_res.returncode

        # Run
        run_res = subprocess.run(
            [str(output_binary)], capture_output=True, text=True, env=env
        )
        return run_res.stdout, run_res.stderr, run_res.returncode
    finally:
        shutil.rmtree(output_root, ignore_errors=True)


def diff_test(file_path, python_exe=sys.executable):
    print(f"Testing {file_path} against {python_exe}...")
    cp_out, cp_err, cp_ret = run_cpython(file_path, python_exe)
    if cp_ret != 0 and (
        "msgpack is required for parse_msgpack fallback" in cp_err
        or "cbor2 is required for parse_cbor fallback" in cp_err
    ):
        print(f"[SKIP] {file_path} (missing msgpack/cbor2 in CPython env)")
        return True
    molt_out, molt_err, molt_ret = run_molt(file_path)

    if molt_out is None:
        print(f"[FAIL] Molt failed to build {file_path}")
        print(molt_err)
        return False

    if cp_out == molt_out and cp_ret == molt_ret:
        print(f"[PASS] {file_path}")
        return True
    else:
        print(f"[FAIL] {file_path} mismatch")
        print(f"  CPython stdout: {cp_out!r}")
        print(f"  Molt    stdout: {molt_out!r}")
        print(f"  CPython return: {cp_ret} stderr: {cp_err!r}")
        print(f"  Molt    return: {molt_ret} stderr: {molt_err!r}")
        return False


def run_diff(target: Path, python_exe: str) -> dict:
    results: list[tuple[str, bool]] = []
    if target.is_dir():
        for file_path in sorted(target.glob("*.py")):
            results.append((str(file_path), diff_test(str(file_path), python_exe)))
    else:
        results.append((str(target), diff_test(str(target), python_exe)))
    total = len(results)
    failed_files = [path for path, ok in results if not ok]
    failed = len(failed_files)
    passed = total - failed
    return {
        "total": total,
        "passed": passed,
        "failed": failed,
        "failed_files": failed_files,
        "python_exe": python_exe,
    }


def _emit_json(payload: dict, output_path: str | None, stdout: bool) -> None:
    text = json.dumps(payload, indent=2, sort_keys=True)
    if output_path:
        Path(output_path).write_text(text)
    if stdout:
        print(text)


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Molt Differential Test Harness")
    parser.add_argument("file", nargs="?", help="Python file to test")
    parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)"
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit JSON summary to stdout.",
    )
    parser.add_argument(
        "--json-output",
        help="Write JSON summary to a file.",
    )

    args = parser.parse_args()

    python_exe = sys.executable
    if args.python_version:
        python_exe = f"python{args.python_version}"

    if args.file:
        target = Path(args.file)
        summary = run_diff(target, python_exe)
        _emit_json(summary, args.json_output, args.json)
        sys.exit(0 if summary["failed"] == 0 else 1)
    # Default test
    with open("temp_test.py", "w") as f:
        f.write("print(1 + 2)\n")
    summary = run_diff(Path("temp_test.py"), python_exe)
    _emit_json(summary, args.json_output, args.json)
    os.remove("temp_test.py")
    sys.exit(0 if summary["failed"] == 0 else 1)
