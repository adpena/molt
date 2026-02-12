"""Purpose: differential coverage for subprocess shell=True and args validation."""

import subprocess


def main():
    result = subprocess.run(
        "echo hello",
        shell=True,
        capture_output=True,
        text=True,
    )
    print("shell", result.returncode, result.stdout.strip())

    try:
        subprocess.run(123, shell=True)
    except Exception as exc:
        print("args", type(exc).__name__)


if __name__ == "__main__":
    main()
