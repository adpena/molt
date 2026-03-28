import itertools
print(list(itertools.chain([1, 2], [3, 4])))
print(list(itertools.islice(range(100), 5)))
print(list(itertools.product("AB", repeat=2)))
print(list(itertools.permutations([1, 2, 3], 2)))
print(list(itertools.combinations([1, 2, 3], 2)))
print(list(itertools.accumulate([1, 2, 3, 4])))
print(list(itertools.repeat("x", 3)))
