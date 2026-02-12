"""Purpose: differential coverage for importlib.resources open_text errors."""

from importlib import resources


def main():
    try:
        resources.open_text("importlib", "missing.txt")
        print("missing", "opened")
    except Exception as exc:
        print("missing", type(exc).__name__)


if __name__ == "__main__":
    main()
