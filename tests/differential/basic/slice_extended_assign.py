"""Purpose: differential coverage for extended slice assignment."""


def main():
    data = [0, 1, 2, 3, 4, 5]
    data[::2] = [10, 11, 12]
    print("step_assign", data)

    try:
        data[::2] = [1, 2]
        print("mismatch", "missed")
    except Exception as exc:
        print("mismatch", type(exc).__name__)


if __name__ == "__main__":
    main()
