def repeat_text(text: str, count: int) -> str:
    parts: list[str] = []
    i = 0
    while i < count:
        parts.append(text)
        i += 1
    return "".join(parts)


def main() -> None:
    repeat: str = repeat_text("na\u00efve", 200_000)
    haystack: str = repeat + "\u2603"
    print(haystack.find("\u2603"))


if __name__ == "__main__":
    main()
