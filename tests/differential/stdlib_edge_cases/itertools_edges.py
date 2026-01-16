import itertools


def section(name):
    print(f"--- {name} ---")


section("Chain empty")
print(list(itertools.chain([], [], [1], [])))
print(list(itertools.chain()))

section("Cycle empty")
# cycle([]) loops infinitely doing nothing in CPython? No, it doesn't yield.
# Wait, actually it caches the iterable. If empty, it finishes immediately?
# "If the iterable is empty, the cycle is empty."
print(list(itertools.cycle([])))

section("Islice edges")
r = range(10)
print(list(itertools.islice(r, 5)))
# Iterator consumed? Yes.
print(list(itertools.islice(r, 5)))  # Next 5 (5-9)

r = range(10)
print(list(itertools.islice(r, 2, 8, 2)))  # 2, 4, 6

try:
    list(itertools.islice(r, -1))
except ValueError:
    print("ValueError caught (negative stop)")

section("Accumulate empty")
print(list(itertools.accumulate([])))
print(list(itertools.accumulate([1, 2, 3], initial=9)))  # Py 3.8+

section("Product/permutations/combinations empty")
print(list(itertools.product([], repeat=2)))
print(list(itertools.permutations([], 0)))
print(list(itertools.permutations([], 1)))
print(list(itertools.combinations([], 0)))
print(list(itertools.combinations([], 1)))

section("Groupby interleaving")
outer = itertools.groupby("AAB")
key1, grp1 = next(outer)
print(key1, next(grp1))
key2, grp2 = next(outer)
print(key2, list(grp2))
try:
    print(next(grp1))
except StopIteration:
    print("StopIteration caught (group exhausted)")

section("Tee interleaving")
a, b = itertools.tee([1, 2, 3], 2)
print(next(a))
print(next(b))
print(list(a))
print(list(b))

section("Tee counts")
print(itertools.tee([1, 2], 0))
(single,) = itertools.tee([1, 2], 1)
print(list(single))
