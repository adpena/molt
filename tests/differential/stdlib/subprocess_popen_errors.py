"""Purpose: differential coverage for subprocess.Popen errors."""

import subprocess


def main():
    try:
        subprocess.Popen(["/path/does/not/exist"])  # noqa: S603
    except Exception as exc:
        print("popen", type(exc).__name__)


if __name__ == "__main__":
    main()
