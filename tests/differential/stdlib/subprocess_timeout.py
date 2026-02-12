"""Purpose: differential coverage for subprocess timeout handling."""

import subprocess
import sys


def main():
    try:
        subprocess.run(
            [sys.executable, "-c", "import time; time.sleep(0.2)"],
            timeout=0.01,
            capture_output=True,
            text=True,
        )
        print("timeout", "missed")
    except Exception as exc:
        print("timeout", type(exc).__name__)


if __name__ == "__main__":
    main()
