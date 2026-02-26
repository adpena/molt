"""Purpose: differential coverage for subprocess text-mode stdin type checking."""

import subprocess
import sys


def _child_cmd():
    return [sys.executable, "-c", "import sys; sys.stdout.write(sys.stdin.read())"]


def _cleanup(proc):
    try:
        proc.kill()
    except Exception:
        pass
    try:
        proc.wait(timeout=1.0)
    except Exception:
        pass


def main():
    proc = subprocess.Popen(
        _child_cmd(),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        proc.stdin.write(b"bad-bytes")
        print("stdin_write", "missed")
    except Exception as exc:
        print("stdin_write", type(exc).__name__, str(exc))
    finally:
        _cleanup(proc)

    proc = subprocess.Popen(
        _child_cmd(),
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    try:
        proc.communicate(input=b"bad-bytes")
        print("communicate_input", "missed")
    except Exception as exc:
        print("communicate_input", type(exc).__name__, str(exc))
    finally:
        _cleanup(proc)


if __name__ == "__main__":
    main()
