def repeat_text(text: str, count: int) -> str:
    parts: list[str] = []
    i = 0
    while i < count:
        parts.append(text)
        i += 1
    return "".join(parts)


def main() -> None:
    haystack: str = repeat_text("alpha,beta,gamma,", 50_000)
    print(len(haystack.split(",")))


if __name__ == "__main__":
    main()
