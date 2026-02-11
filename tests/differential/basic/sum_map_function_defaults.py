def make_check(itde, make=lambda x: x, check=lambda x: x):
    i, d = itde
    return i + d


chunk = [(1, 2), (3, 4), (5, 6)]
print(sum(map(make_check, chunk)))
