import subprocess
import sys
import os
from pathlib import Path


def run_cpython(file_path, python_exe=sys.executable):
    env = os.environ.copy()
    paths = [env.get("PYTHONPATH", ""), ".", "src"]
    env["PYTHONPATH"] = os.pathsep.join(p for p in paths if p)
    result = subprocess.run(
        [python_exe, file_path], capture_output=True, text=True, env=env
    )
    return result.stdout, result.stderr, result.returncode


def run_molt(file_path):
    # Clean up stale binary
    if os.path.exists("./hello_molt"):
        os.remove("./hello_molt")

    # Build
    env = os.environ.copy()
    env["PYTHONPATH"] = "src"
    build_res = subprocess.run(
        [sys.executable, "-m", "molt.cli", "build", file_path],
        env=env,
        capture_output=True,
        text=True,
    )
    if build_res.returncode != 0:
        return None, build_res.stderr, build_res.returncode

    # Run
    run_res = subprocess.run(["./hello_molt"], capture_output=True, text=True)
    return run_res.stdout, run_res.stderr, run_res.returncode


def diff_test(file_path, python_exe=sys.executable):
    print(f"Testing {file_path} against {python_exe}...")
    cp_out, cp_err, cp_ret = run_cpython(file_path, python_exe)
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


if __name__ == "__main__":
    import argparse

    parser = argparse.ArgumentParser(description="Molt Differential Test Harness")
    parser.add_argument("file", nargs="?", help="Python file to test")
    parser.add_argument(
        "--python-version", help="Python version to test against (e.g. 3.13)"
    )

    args = parser.parse_args()

    python_exe = sys.executable
    if args.python_version:
        python_exe = f"python{args.python_version}"

    if args.file:
        target = Path(args.file)
        if target.is_dir():
            ok = True
            for file_path in sorted(target.glob("*.py")):
                ok = diff_test(str(file_path), python_exe) and ok
            sys.exit(0 if ok else 1)
        diff_test(args.file, python_exe)
    else:
        # Default test
        with open("temp_test.py", "w") as f:
            f.write("print(1 + 2)\n")
        success = diff_test("temp_test.py", python_exe)
        os.remove("temp_test.py")
        sys.exit(0 if success else 1)
