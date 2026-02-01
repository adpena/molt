from __future__ import annotations


def main() -> None:
    size: int = 1_000_000
    nums: list[int] = list(range(size))
    total: int = 0
    for x in nums:
        total += x
    print(total)


if __name__ == "__main__":
    main()
