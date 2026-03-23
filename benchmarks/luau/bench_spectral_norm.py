def eval_a(i: int, j: int) -> float:
    return 1.0 / ((i + j) * (i + j + 1) / 2 + i + 1)

def eval_a_times_u(n: int, u: list[float], au: list[float]) -> None:
    i: int = 0
    while i < n:
        s: float = 0.0
        j: int = 0
        while j < n:
            s = s + eval_a(i, j) * u[j]
            j = j + 1
        au[i] = s
        i = i + 1

def eval_at_times_u(n: int, u: list[float], atu: list[float]) -> None:
    i: int = 0
    while i < n:
        s: float = 0.0
        j: int = 0
        while j < n:
            s = s + eval_a(j, i) * u[j]
            j = j + 1
        atu[i] = s
        i = i + 1

def eval_ata_times_u(n: int, u: list[float], atu: list[float]) -> None:
    v: list[float] = []
    i: int = 0
    while i < n:
        v.append(0.0)
        i = i + 1
    eval_a_times_u(n, u, v)
    eval_at_times_u(n, v, atu)

def main() -> None:
    n: int = 100
    u: list[float] = []
    v: list[float] = []
    i: int = 0
    while i < n:
        u.append(1.0)
        v.append(0.0)
        i = i + 1

    i = 0
    while i < 10:
        eval_ata_times_u(n, u, v)
        eval_ata_times_u(n, v, u)
        i = i + 1

    vbv: float = 0.0
    vv: float = 0.0
    i = 0
    while i < n:
        vbv = vbv + u[i] * v[i]
        vv = vv + v[i] * v[i]
        i = i + 1

    result: float = (vbv / vv) ** 0.5
    print(result)

main()
