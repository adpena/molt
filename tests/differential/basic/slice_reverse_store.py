"""A slice store of a reversed slice of the same list, in every spelling and
near miss, against CPython: the compiler reverses in place only where the
bounds are proven to name one range, and the statement runs as written
everywhere else."""


def prefix(seq, k):
    seq[: k + 1] = seq[k::-1]
    return seq


def prefix_via_name(seq, k):
    n = k + 1
    seq[:n] = seq[n - 1 :: -1]
    return seq


def middle(seq, i, j):
    seq[i:j] = seq[j - 1 : i - 1 : -1]
    return seq


def suffix(seq, i):
    seq[i:] = seq[: i - 1 : -1]
    return seq


def whole(seq):
    seq[:] = seq[::-1]
    return seq


def through_alias(seq, k):
    other = seq
    seq[: k + 1] = other[k::-1]
    return seq


def short_target(seq, k):
    seq[:k] = seq[k::-1]
    return seq


def wide_target(seq, k):
    seq[: k + 2] = seq[k::-1]
    return seq


def other_list(seq, other, k):
    seq[: k + 1] = other[k::-1]
    return seq


def step_two(seq, k):
    seq[: k + 1] = seq[k::-2]
    return seq


def stops_short(seq, k):
    seq[: k + 1] = seq[k:0:-1]
    return seq


def used_after(seq, k):
    seq[: k + 1] = flipped = seq[k::-1]
    return seq, flipped


def flips(perm):
    perm = perm[:]
    count = 0
    k = perm[0]
    while k:
        perm[: k + 1] = perm[k::-1]
        count += 1
        k = perm[0]
    return count, perm


def run(label, function, *args):
    try:
        print(label, function(*args))
    except Exception as exc:
        print(label, type(exc).__name__, exc)


for k in [0, 1, 3, 5, 9, -1, -3, -7, True, False, 2**70, -(2**70), None, 2.0]:
    run(("prefix", k), prefix, list(range(6)), k)
    run(("prefix_via_name", k), prefix_via_name, list(range(6)), k)
    run(("through_alias", k), through_alias, list(range(6)), k)
    run(("short_target", k), short_target, list(range(6)), k)
    run(("wide_target", k), wide_target, list(range(6)), k)
    run(("other_list", k), other_list, list(range(6)), list("abcdef"), k)
    run(("step_two", k), step_two, list(range(6)), k)
    run(("stops_short", k), stops_short, list(range(6)), k)
    run(("used_after", k), used_after, list(range(6)), k)

for i in [0, 1, 2, 5, 6, 9, -1, -2]:
    run(("suffix", i), suffix, list(range(6)), i)
    for j in [0, 1, 3, 6, 8, -1]:
        run(("middle", i, j), middle, list(range(6)), i, j)

run("whole", whole, list(range(6)))
run("whole empty", whole, [])
run("whole one", whole, [7])
run("whole refs", whole, ["a", object.__name__, "c"])
run("prefix empty", prefix, [], 3)
run("prefix one", prefix, [4], 0)
run("prefix refs", prefix, ["a", "b", "c", "d"], 2)
run("prefix tuple", prefix, (1, 2, 3), 1)
run("prefix str", prefix, list(range(4)), "2")
run("flips", flips, [3, 1, 4, 0, 2])


class Recorded(list):
    def __getitem__(self, key):
        print("getitem", key)
        return list.__getitem__(self, key)

    def __setitem__(self, key, value):
        print("setitem", key, value)
        list.__setitem__(self, key, value)


run("subclass prefix", prefix, Recorded(range(5)), 2)
run("subclass whole", whole, Recorded(range(4)))
run("subclass middle", middle, Recorded(range(6)), 1, 4)


class Index:
    def __init__(self, value):
        self.value = value

    def __index__(self):
        print("index", self.value)
        return self.value

    def __add__(self, other):
        print("add", self.value, other)
        return Index(self.value + other)

    def __sub__(self, other):
        print("sub", self.value, other)
        return Index(self.value - other)


run("index prefix", prefix, list(range(6)), Index(3))
run("index via name", prefix_via_name, list(range(6)), Index(3))
run("index middle", middle, list(range(6)), Index(1), Index(4))

# Mutation during iteration sees the reversed list.
seq = list(range(6))
seen = []
for item in seq:
    seen.append(item)
    if item == 1:
        prefix(seq, 4)
print("mutate during iteration", seen, seq)
