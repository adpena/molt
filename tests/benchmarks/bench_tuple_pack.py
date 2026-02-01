def main() -> None:
    total = 0
    for i in range(200_000):
        t: tuple[int, int, int, int] = (i, i + 1, i + 2, i + 3)
        total += t[0] + t[1] + t[2] + t[3]

    print(total)


if __name__ == "__main__":
    main()
