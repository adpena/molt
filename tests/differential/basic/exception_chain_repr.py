"""Purpose: differential coverage for exception chaining repr."""


def main():
    try:
        try:
            raise ValueError("inner")
        except ValueError as exc:
            raise RuntimeError("outer") from exc
    except Exception as exc:
        print("repr", repr(exc))
        print("cause", type(exc.__cause__).__name__)


if __name__ == "__main__":
    main()
