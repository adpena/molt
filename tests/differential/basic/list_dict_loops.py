"""Purpose: differential coverage for list loops and cached-storage lifetime."""

lines = ["alpha", "beta", "gamma", "beta"]

counts = {"alpha": 0, "beta": 0, "gamma": 0}
for i in range(len(lines)):
    name = lines[i]
    counts[name] = counts[name] + 1

print(counts["alpha"])
print(counts["beta"])
print(counts["gamma"])

matrix = [[1, 2], [3, 4], [5, 6]]

acc = 0
for row in matrix:
    for value in row:
        acc = acc + value
print(acc)


def sibling_branch_digest(take_first):
    values = [3, 5, 7, 11]
    digest = 0
    if take_first:
        for i in range(len(values)):
            digest = digest + values[i] * (i + 1)
    else:
        for i in range(len(values)):
            digest = digest + values[i] * (i + 2)
    return digest


print("cache:sibling", sibling_branch_digest(True), sibling_branch_digest(False))


def alias_growth_digest():
    values = [2, 4, 6, 8]
    before = 0
    for i in range(len(values)):
        before = before + values[i]

    alias = values
    for i in range(24):
        alias.append(100 + i)
    alias.extend([211, 223, 227, 229])

    after = 0
    for i in range(len(values)):
        after = after + values[i] * (i + 1)
    return before, len(values), after


print("cache:alias-growth", alias_growth_digest())


def generic_alias_growth_digest():
    values = [1, None, "three", False]
    before_none = 0
    for i in range(len(values)):
        if values[i] is None:
            before_none = before_none + 1

    alias = values
    alias.extend([None, "tail", 9, None])

    after_none = 0
    for i in range(len(values)):
        if values[i] is None:
            after_none = after_none + 1
    return before_none, len(values), after_none


print("cache:generic-growth", generic_alias_growth_digest())


def closure_mutation_digest():
    values = [1, 3, 5, 7]
    view = values

    def mutate():
        values.extend([9, 11, 13, 15, 17])

    before = 0
    for i in range(len(view)):
        before = before + view[i]
    mutate()
    after = 0
    for i in range(len(view)):
        after = after + view[i] * (i + 1)
    return before, len(view), after


print("cache:closure", closure_mutation_digest())


GLOBAL_CACHE_VALUES = [7, 11, 13]


def mutate_global_cache_values():
    GLOBAL_CACHE_VALUES.extend([17, 19, 23, 29, 31])


def global_mutation_digest():
    view = GLOBAL_CACHE_VALUES
    before = 0
    for i in range(len(view)):
        before = before + view[i]
    mutate_global_cache_values()
    after = 0
    for i in range(len(view)):
        after = after + view[i] * (i + 1)
    return before, len(view), after


print("cache:global", global_mutation_digest())


def deletion_and_clear_digest():
    deleted = [10, 20, 30, 40, 50]
    deleted_before = 0
    for i in range(len(deleted)):
        deleted_before = deleted_before + deleted[i]
    del deleted[0]
    deleted_after = 0
    for i in range(len(deleted)):
        deleted_after = deleted_after + deleted[i] * (i + 1)

    cleared = [2, 3, 5, 7]
    cleared_before = 0
    for i in range(len(cleared)):
        cleared_before = cleared_before + cleared[i]
    clear_alias = cleared
    clear_alias.clear()
    clear_alias.extend([41, 43, 47, 53, 59])
    cleared_after = 0
    for i in range(len(cleared)):
        cleared_after = cleared_after + cleared[i] * (i + 1)
    return deleted_before, deleted_after, cleared_before, cleared_after


print("cache:delete-clear", deletion_and_clear_digest())


def nested_mutation_digest():
    values = [1, 2, 3]
    before = 0
    for i in range(len(values)):
        before = before + values[i]

    alias = values
    for outer in range(3):
        for inner in range(8):
            alias.append(100 * outer + inner)

    after = 0
    for i in range(len(values)):
        after = after + values[i] * (i + 1)
    return before, len(values), after


print("cache:nested", nested_mutation_digest())


class CacheLifetimeFinalizer:
    def __init__(self, values):
        self.values = values

    def __del__(self):
        self.values.extend([37, 41, 43, 47])


def finalizer_mutation_digest():
    values = [2, 3, 5]
    owner = CacheLifetimeFinalizer(values)
    before = 0
    for i in range(len(values)):
        before = before + values[i]
    owner = None
    after = 0
    for i in range(len(values)):
        after = after + values[i] * (i + 1)
    return before, owner, len(values), after


print("cache:finalizer", finalizer_mutation_digest())


def indexed_truth(values, index):
    selected = values[index]
    if selected:
        return 1
    return 0


print(
    "cache:negative-bool",
    indexed_truth([True, False], -1),
    indexed_truth([False, True], -1),
    indexed_truth([True, False], 1),
    indexed_truth([False, True], 1),
)


def bool_snapshot_digest(values):
    selected = values[-1]
    alias = values
    alias.clear()
    alias.extend([0, 2, 4])
    current = values[0]
    if selected:
        return 1, current, len(values)
    return 0, current, len(values)


print(
    "cache:bool-snapshot",
    bool_snapshot_digest([True, False]),
    bool_snapshot_digest([False, True]),
)
