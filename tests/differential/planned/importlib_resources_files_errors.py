"""Purpose: differential coverage for importlib.resources.files errors."""

from importlib import resources


def main():
    try:
        resources.files("does.not.exist")
    except Exception as exc:
        print("files", type(exc).__name__)


if __name__ == "__main__":
    main()
