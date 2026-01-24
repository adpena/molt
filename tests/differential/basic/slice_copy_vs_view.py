"""Purpose: differential coverage for slice copy vs view behavior."""


def main():
    data = [[1], [2], [3]]
    sliced = data[:]
    sliced[0].append(9)
    print("mutated", data[0])
    sliced[0] = [42]
    print("rebind", data[0])


if __name__ == "__main__":
    main()
