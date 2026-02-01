def main() -> None:
    size = 1_000_000
    nums = [1 for _ in range(size)]
    acc = 1
    for x in nums:
        acc = acc * x
    print(acc)


if __name__ == "__main__":
    main()
