"""Purpose: differential coverage for importlib.resources read_* errors."""

from importlib import resources


def main():
    try:
        resources.read_text("importlib", "missing.txt")
        print("read_text", "missed")
    except Exception as exc:
        print("read_text", type(exc).__name__)

    try:
        resources.read_binary("importlib", "missing.bin")
        print("read_binary", "missed")
    except Exception as exc:
        print("read_binary", type(exc).__name__)


if __name__ == "__main__":
    main()
