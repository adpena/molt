"""Purpose: differential coverage for subprocess communicate repeat-input guard."""

import subprocess
import sys


def main():
    proc = subprocess.Popen(
        [
            sys.executable,
            "-c",
            "import sys; sys.stdout.write(sys.stdin.read())",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    out, err = proc.communicate("first-input")
    print("first_out", repr(out))
    print("first_err", repr(err))
    print("first_rc", proc.returncode)

    try:
        proc.communicate("second-input")
        print("repeat_input", "missed")
    except Exception as exc:
        print("repeat_input", type(exc).__name__, str(exc))


if __name__ == "__main__":
    main()
