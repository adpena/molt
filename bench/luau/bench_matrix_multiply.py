def matrix_multiply(n: int, a: list[float], b: list[float], c: list[float]) -> None:
    i: int = 0
    while i < n:
        j: int = 0
        while j < n:
            s: float = 0.0
            k: int = 0
            while k < n:
                s = s + a[i * n + k] * b[k * n + j]
                k = k + 1
            c[i * n + j] = s
            j = j + 1
        i = i + 1

def main() -> None:
    n: int = 100
    a: list[float] = []
    b: list[float] = []
    c: list[float] = []

    i: int = 0
    while i < n * n:
        a.append((i * 17 + 3) % 100 * 0.01)
        b.append((i * 31 + 7) % 100 * 0.01)
        c.append(0.0)
        i = i + 1

    matrix_multiply(n, a, b, c)

    total: float = 0.0
    i = 0
    while i < n * n:
        total = total + c[i]
        i = i + 1
    print(total)

main()
