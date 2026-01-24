"""Purpose: differential coverage for negative-step slicing."""


def main():
    data = [0, 1, 2, 3, 4]
    print("rev", data[::-1])
    print("step", data[4:1:-2])
    print("empty", data[1:4:-1])


if __name__ == "__main__":
    main()
