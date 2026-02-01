def main() -> None:
    nums = list(range(1_000_000))
    acc = nums[0]
    for x in nums:
        if x > acc:
            acc = x
    print(acc)


if __name__ == "__main__":
    main()
