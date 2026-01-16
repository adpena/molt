import itertools

print(list(itertools.islice(itertools.count(10, 2), 5)))
print(list(itertools.islice(itertools.cycle([1, 2, 3]), 7)))
print(list(itertools.repeat("x", 3)))
print(list(itertools.islice(itertools.repeat(1), 3)))

print(list(itertools.accumulate([1, 2, 3, 4])))
print(list(itertools.accumulate([1, 2, 3, 4], initial=10)))

print(list(itertools.pairwise([1, 2, 3])))
print(list(itertools.product("ab", repeat=2)))
print(list(itertools.permutations([1, 2, 3], 2)))
print(list(itertools.combinations([1, 2, 3], 2)))
print(list(itertools.product([1, 2], repeat=0)))
print(list(itertools.permutations([1, 2, 3], 0)))
print(list(itertools.combinations([1, 2, 3], 0)))
print(list(itertools.permutations([1, 2], 3)))
print(list(itertools.combinations([1, 2, 3], 5)))

groups = [(key, list(group)) for key, group in itertools.groupby("AAAABBBCCDAABBB")]
print(groups)

a, b = itertools.tee([1, 2, 3], 2)
print(list(a))
print(list(b))
