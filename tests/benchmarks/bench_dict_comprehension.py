"""Measures dict comprehension and iteration patterns."""


def main() -> None:
    data = {str(i): i * i for i in range(100000)}
    total = sum(v for v in data.values() if v % 2 == 0)
    inverted = {v: k for k, v in data.items()}
    print(total, len(inverted))


if __name__ == "__main__":
    main()
