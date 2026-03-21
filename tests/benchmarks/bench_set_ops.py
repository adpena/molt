"""Measures set construction, membership, and set algebra."""
def main() -> None:
    a = set(range(0, 100000, 2))
    b = set(range(0, 100000, 3))
    union = a | b
    inter = a & b
    diff = a - b
    sym = a ^ b
    total = len(union) + len(inter) + len(diff) + len(sym)
    print(total)

if __name__ == "__main__":
    main()
