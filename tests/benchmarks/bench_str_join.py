def main() -> None:
    size = 1_000_000
    items = ["a" for _ in range(size)]
    print(len("-".join(items)))


if __name__ == "__main__":
    main()
