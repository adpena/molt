from collections import Counter


def main() -> None:
    words = ["a", "b", "a", "c", "b", "a"]
    total = 0
    outer = 0
    while outer < 5:
        counts = Counter(words)
        total += counts["a"]
        total += len(counts)
        outer += 1
    print(total)


main()
