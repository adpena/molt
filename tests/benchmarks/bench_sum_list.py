def main() -> None:
    size = 1_000_000
    nums = [i for i in range(size)]
    total = 0
    for x in nums:
        total += x
    print(total)


if __name__ == "__main__":
    main()
