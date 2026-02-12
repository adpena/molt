def main() -> None:
    call_import = __import__
    i = 0
    total = 0
    while i < 100_000:
        mod = call_import("builtins")
        if mod is not None:
            total += 1
        i += 1
    print(total)


if __name__ == "__main__":
    main()
