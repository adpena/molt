"""Purpose: differential coverage for subprocess.check_output."""

import subprocess
import sys


def main():
    output = subprocess.check_output([sys.executable, "-c", "print('ok')"], text=True)
    print("output", output.strip())


if __name__ == "__main__":
    main()
