"""Purpose: differential coverage for subprocess check errors."""

import subprocess
import sys


def main():
    try:
        subprocess.run(
            [sys.executable, "-c", "import sys; sys.exit(2)"],
            check=True,
            capture_output=True,
            text=True,
        )
        print("check", "missed")
    except subprocess.CalledProcessError as exc:
        print("check", type(exc).__name__, exc.returncode)


if __name__ == "__main__":
    main()
