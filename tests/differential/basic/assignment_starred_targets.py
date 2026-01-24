"""Purpose: differential coverage for starred assignment targets."""


def main():
    a, *b, c = [1, 2, 3, 4]
    print("middle", a, b, c)

    *a, b = [1]
    print("short", a, b)

    try:
        a, *b, c = [1]
        print("error", "missed")
    except Exception as exc:
        print("error", type(exc).__name__)


if __name__ == "__main__":
    main()
