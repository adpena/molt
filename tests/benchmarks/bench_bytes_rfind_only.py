def main() -> None:
    haystack = b"ab" * 5_000_000
    needle = b"abab"
    i = 0
    total = 0
    while i < 200:
        total += haystack.rfind(needle)
        i += 1
    print(total)


if __name__ == "__main__":
    main()
