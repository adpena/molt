from collections import Counter


def main() -> None:
    base = (
        "molt builds fast python binaries for data pipelines and services "
        "python workloads stay deterministic and safe with explicit contracts "
    )
    parts: list[str] = []
    i = 0
    while i < 400:
        parts.append(base)
        i += 1
    text: str = "".join(parts).strip()
    words: list[str] = text.split()

    total = 0
    outer = 0
    while outer < 80:
        counts: Counter[str] = Counter(words)
        total += counts["molt"]
        total += counts["python"]
        total += len(counts)
        outer += 1

    print(total)


if __name__ == "__main__":
    main()
