"""Exercise every itertools class and sentinel through real iterator behavior.

Full builds use the satellite transport; micro builds include the shared source
in-tree. Both publish methods through the same runtime class authority. Running
the source under each tier checks that transport does not change behavior.

Constructing every iterator twice exercises lazy class publication followed by
cached class reuse. Each class namespace owns its declared iterator methods;
there are no separate iterator-callable caches. The keyword-marker sentinel
retains its runtime-scoped cache.
"""

import functools
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

    # Presence is independent of float bits; initial None is omitted by CPython.
    out.append(list(itertools.accumulate([1.0, 2.0], initial=0.0)))
    out.append(list(itertools.accumulate([], initial=-0.0)))
    out.append(list(itertools.accumulate([1, 2], initial=None)))
    out.append(list(itertools.accumulate([[1], [2]], initial=[])))
    out.append(functools.reduce(lambda a, b: a + b, [[1], [2]], []))
    out.append(functools.reduce(lambda a, b: a + b, [[1], [2]]))

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

    out.append(
        [(k, list(g)) for k, g in itertools.groupby([None, None, 0.0, -0.0, None])]
    )
    groups = itertools.groupby("AABAA")
    first_key, first_group = next(groups)
    second_key, second_group = next(groups)
    third_key, third_group = next(groups)
    # A stale grouper cannot resume when an equal key appears again.
    out.append((first_key, second_key, third_key, list(first_group), list(third_group)))

    # tee independence
    a, b = itertools.tee([1, 2, 3], 2)
    out.append((list(a), list(b)))

    # zip_longest with fillvalue (exercises the keyword-marker sentinel slot)
    out.append(list(itertools.zip_longest("AB", "wxyz", fillvalue="-")))

    return out


# Run the full battery twice: the second pass hits the already-initialized slots
# (the cached classes and namespace-owned methods), so divergence between lazy-init and
# cached-read paths would show up here too.
for _ in range(2):
    for row in drive():
        print(row)
