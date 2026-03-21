"""Measures allocation-heavy workload that stresses GC/refcount."""
def main() -> None:
    results = []
    for i in range(1_000_000):
        results.append({"key": i, "value": [i, i + 1, i + 2]})
    print(len(results))

if __name__ == "__main__":
    main()
