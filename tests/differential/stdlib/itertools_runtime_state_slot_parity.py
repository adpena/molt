"""Purpose: exercise every itertools class/next-fn/sentinel slot so the
RuntimeState-scoped object slots (in-tree builtins/itertools.rs and the
molt-runtime-itertools satellite, reconciled in Move R.1b) produce byte-identical
output. This same source runs under the default (full) tier — which compiles the
SATELLITE — and under MOLT_DIFF_STDLIB_PROFILE=micro — which compiles the IN-TREE
copy. Identical output across both tiers proves the two physical copies share one
behavior for the lazily-cached itertools type objects.

The slots are populated lazily on first use of each constructor; constructing and
re-constructing every itertools type exercises the get-or-init path for all 21
class slots, the 21 next-fn slots, the shared iter-self function slot, and the
keyword-marker sentinel slot.
"""

import itertools


def drive():
    out = []

    # count / repeat / cycle (infinite — bounded via islice)
    out.append(list(itertools.islice(itertools.count(10, 2), 4)))
    out.append(list(itertools.islice(itertools.cycle("AB"), 5)))
    out.append(list(itertools.repeat("x", 3)))

    # chain / chain.from_iterable
    out.append(list(itertools.chain("ab", "cd")))
    out.append(list(itertools.chain.from_iterable([[1, 2], [3], []])))

    # accumulate (default + binary func)
    out.append(list(itertools.accumulate([1, 2, 3, 4])))
    out.append(list(itertools.accumulate([1, 2, 3, 4], lambda a, b: a * b)))

    # batched
    out.append([list(b) for b in itertools.batched(range(7), 3)])

    # combinations / combinations_with_replacement / permutations / product
    out.append(list(itertools.combinations("ABC", 2)))
    out.append(list(itertools.combinations_with_replacement("AB", 2)))
    out.append(list(itertools.permutations("ABC", 2)))
    out.append(list(itertools.product("AB", "xy")))

    # compress / dropwhile / takewhile / filterfalse
    out.append(list(itertools.compress("ABCDEF", [1, 0, 1, 0, 1, 1])))
    out.append(list(itertools.dropwhile(lambda n: n < 3, [1, 2, 3, 4, 1])))
    out.append(list(itertools.takewhile(lambda n: n < 3, [1, 2, 3, 4, 1])))
    out.append(list(itertools.filterfalse(lambda n: n % 2, range(8))))

    # pairwise
    out.append(list(itertools.pairwise("ABCD")))

    # starmap
    out.append(list(itertools.starmap(lambda a, b: a + b, [(1, 2), (3, 4)])))

    # groupby (key default + keyfunc)
    out.append([(k, list(g)) for k, g in itertools.groupby("aaabbbcca")])
    out.append(
        [(k, list(g)) for k, g in itertools.groupby(range(8), key=lambda n: n // 3)]
    )

    # tee independence
    a, b = itertools.tee([1, 2, 3], 2)
    out.append((list(a), list(b)))

    # zip_longest with fillvalue (exercises the keyword-marker sentinel slot)
    out.append(list(itertools.zip_longest("AB", "wxyz", fillvalue="-")))

    return out


# Run the full battery twice: the second pass hits the already-initialized slots
# (the cached class/next-fn objects), so any divergence between the lazy-init and
# cached-read paths would show up here too.
for _ in range(2):
    for row in drive():
        print(row)
