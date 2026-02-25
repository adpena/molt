"""Purpose: differential coverage for subprocess.Popen context-manager semantics."""

import subprocess
import sys


def main():
    with subprocess.Popen(  # noqa: S603
        [sys.executable, "-c", "print('cm-ok')"],
        stdout=subprocess.PIPE,
        text=True,
    ) as proc:
        inside = proc.stdout.read().strip()
        print("inside_returncode", proc.returncode)
    print("cm_output", inside)
    print("cm_returncode", proc.returncode)
    print("cm_stdout_closed", proc.stdout.closed)

    proc2 = None
    try:
        with subprocess.Popen(  # noqa: S603
            [sys.executable, "-c", "import time; time.sleep(0.05)"],
            stdout=subprocess.PIPE,
            text=True,
        ) as proc2:
            raise RuntimeError("boom")
    except RuntimeError as exc:
        print("body_exc", type(exc).__name__, str(exc))
    if proc2 is not None:
        print("cm2_returncode", proc2.returncode)
        print("cm2_stdout_closed", proc2.stdout.closed)


if __name__ == "__main__":
    main()
