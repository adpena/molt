"""fannkuch benchmark kernel (derived from pyperformance, MIT)."""

import pyperf


def fannkuch(n: int) -> int:
    count = list(range(1, n + 1))
    max_flips = 0
    m = n - 1
    r = n
    perm1 = list(range(n))
    perm = list(range(n))
    perm1_insert = perm1.insert
    perm1_pop = perm1.pop

    while True:
        while r != 1:
            count[r - 1] = r
            r -= 1

        if perm1[0] != 0 and perm1[m] != m:
            perm = perm1[:]
            flips = 0
            k = perm[0]
            while k:
                perm[: k + 1] = perm[k::-1]
                flips += 1
                k = perm[0]
            if flips > max_flips:
                max_flips = flips

        while r != n:
            perm1_insert(r, perm1_pop(0))
            count[r] -= 1
            if count[r] > 0:
                break
            r += 1
        else:
            return max_flips


def bench_fannkuch(loops: int, n: int = 8) -> float:
    start = pyperf.perf_counter()
    for _ in range(loops):
        fannkuch(n)
    return pyperf.perf_counter() - start
