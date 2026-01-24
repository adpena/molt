"""Purpose: differential coverage for exception context switching."""


def main():
    try:
        try:
            raise ValueError("inner")
        except ValueError:
            raise TypeError("outer")
    except Exception as exc:
        print("type", type(exc).__name__)
        print("context", type(exc.__context__).__name__)
        print("cause", exc.__cause__)


if __name__ == "__main__":
    main()
