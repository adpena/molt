"""Purpose: differential coverage for subprocess pipes in bytes mode."""

import subprocess
import sys


def main():
    proc = subprocess.Popen(
        [sys.executable, "-c", "import sys; data=sys.stdin.buffer.read(); sys.stdout.buffer.write(data.upper())"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    out, err = proc.communicate(b"hello")
    print("returncode", proc.returncode)
    print("stdout", out)
    print("stderr", err)


if __name__ == "__main__":
    main()
