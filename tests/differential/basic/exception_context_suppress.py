"""Purpose: differential coverage for context suppression with from None."""


def main():
    try:
        try:
            raise ValueError("inner")
        except ValueError:
            raise TypeError("outer") from None
    except Exception as exc:
        print("type", type(exc).__name__)
        print("context", exc.__context__)
        print("cause", exc.__cause__)


if __name__ == "__main__":
    main()
