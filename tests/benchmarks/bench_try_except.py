def main() -> None:
    total = 0
    i = 0
    while i < 200_000:
        try:
            total += i
        except ValueError:
            total -= 1
        i += 1

    print(total)


if __name__ == "__main__":
    main()
