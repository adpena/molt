"""Purpose: differential coverage for subprocess.check_output errors."""

import subprocess
import sys


def main():
    try:
        subprocess.check_output(
            [sys.executable, "-c", "import sys; sys.exit(3)"],
            text=True,
        )
        print("check_output", "missed")
    except subprocess.CalledProcessError as exc:
        print("check_output", type(exc).__name__, exc.returncode)


if __name__ == "__main__":
    main()
