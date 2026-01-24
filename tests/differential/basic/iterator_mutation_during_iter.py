"""Purpose: differential coverage for mutation during iteration."""


def main():
    data = [1, 2, 3, 4]
    out = []
    for item in data:
        out.append(item)
        if item == 2:
            data.append(5)
    print("out", out)
    print("data", data)


if __name__ == "__main__":
    main()
