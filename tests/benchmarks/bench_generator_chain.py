"""Multi-generator fusion benchmark (doc 26 Benchmark Set).

`for x in chain(range(500), range(500)): total += x` — exercises multi-generator
fusion. Once Tier-B fuses the chain into a single index loop, RSS stays flat
(no per-yield pair allocation). Until the `yield from` / multi-generator and
function-scope extensions land (doc 26 Phase-1 Finding #1), this runs correctly
via Tier D (the heap-frame runtime) and is the regression/perf gate the
extensions must satisfy.
"""


def chain(*iterables):
    for it in iterables:
        for elem in it:
            yield elem


def main() -> None:
    total = 0
    outer = 0
    while outer < 1000:
        for x in chain(range(500), range(500)):
            total += x
        outer += 1
    print(total)


if __name__ == "__main__":
    main()
