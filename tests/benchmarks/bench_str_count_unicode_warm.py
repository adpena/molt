def repeat_text(text: str, count: int) -> str:
    parts: list[str] = []
    i = 0
    while i < count:
        parts.append(text)
        i += 1
    return "".join(parts)


def main() -> None:
    haystack: str = repeat_text("caf\u00e9", 300_000)
    needle: str = "\u00e9"

    haystack.count(needle)
    total = 0
    for _ in range(25):
        total += haystack.count(needle)
    print(total)


if __name__ == "__main__":
    main()
