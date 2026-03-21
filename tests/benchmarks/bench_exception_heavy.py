"""Measures exception handling overhead in tight loops."""
def main() -> None:
    total = 0
    for i in range(2_000_000):
        try:
            if i % 3 == 0:
                raise ValueError(i)
            total += i
        except ValueError as e:
            total += int(str(e))
    print(total)

if __name__ == "__main__":
    main()
