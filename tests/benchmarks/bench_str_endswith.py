def repeat_text(text: str, count: int) -> str:
    parts: list[str] = []
    i = 0
    while i < count:
        parts.append(text)
        i += 1
    return "".join(parts)


def main() -> None:
    haystack: str = repeat_text("a", 500_000) + "b"
    print(haystack.endswith("b"))


if __name__ == "__main__":
    main()
