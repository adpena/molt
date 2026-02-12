"""Purpose: differential coverage for subprocess communicate with pipes."""

import subprocess
import sys


def main():
    proc = subprocess.Popen(
        [sys.executable, "-c", "import sys; data=sys.stdin.read(); print(data.upper())"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )
    out, err = proc.communicate("hello")
    print("returncode", proc.returncode)
    print("stdout", out.strip())
    print("stderr", err.strip())


if __name__ == "__main__":
    main()
