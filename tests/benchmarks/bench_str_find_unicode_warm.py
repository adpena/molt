def repeat_text(text: str, count: int) -> str:
    parts: list[str] = []
    i = 0
    while i < count:
        parts.append(text)
        i += 1
    return "".join(parts)


def main() -> None:
    haystack: str = repeat_text("caf\u00e9", 200_000)
    needle: str = "\u00e9"

    haystack.find(needle)
    total = 0
    for _ in range(25):
        total += haystack.find(needle)
    print(total)


if __name__ == "__main__":
    main()
