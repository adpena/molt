"""Purpose: differential coverage for nested starred assignment targets."""


def main():
    (a, *b), c = ([1, 2, 3], 4)
    print("nested", a, b, c)

    try:
        (a, *b), c = ([1], 2)
        print("error", "missed")
    except Exception as exc:
        print("error", type(exc).__name__)


if __name__ == "__main__":
    main()
