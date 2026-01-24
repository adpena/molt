"""Purpose: differential coverage for reraise context clearing."""


def main():
    try:
        try:
            raise ValueError("inner")
        except ValueError:
            raise
    except Exception as exc:
        print("type", type(exc).__name__)
        print("context", exc.__context__)


if __name__ == "__main__":
    main()
