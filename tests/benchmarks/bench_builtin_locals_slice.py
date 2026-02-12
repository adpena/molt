def main() -> None:
    call_locals = locals
    i = 0
    total = 0
    while i < 200_000:
        mapping = call_locals()
        if isinstance(mapping, dict):
            total += 1
        i += 1
    print(total)


if __name__ == "__main__":
    main()
