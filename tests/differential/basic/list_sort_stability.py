"""Purpose: differential coverage for list sort stability."""


def main():
    items = [("a", 1), ("b", 1), ("c", 1)]
    items.sort(key=lambda item: item[1])
    print("order", [item[0] for item in items])


if __name__ == "__main__":
    main()
