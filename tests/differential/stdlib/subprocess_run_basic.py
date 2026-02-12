"""Purpose: differential coverage for subprocess.run basics."""

import subprocess
import sys


def main():
    result = subprocess.run(
        [sys.executable, "-c", "print('hello')"],
        capture_output=True,
        text=True,
    )
    print("returncode", result.returncode)
    print("stdout", result.stdout.strip())


if __name__ == "__main__":
    main()
