"""Purpose: differential coverage for slice deletion behavior."""


def main():
    data = [0, 1, 2, 3, 4, 5, 6]
    del data[1:5:2]
    print("deleted", data)

    data2 = ["a", "b", "c", "d"]
    del data2[:]
    print("cleared", data2)


if __name__ == "__main__":
    main()
