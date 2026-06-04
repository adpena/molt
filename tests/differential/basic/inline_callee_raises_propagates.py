# A chain of inlinable leaf callees where the innermost raises. The exception
# must propagate up through every inlined frame to the OUTERmost try/except,
# exactly as the un-inlined call/return/check sequence would. Exercises
# bottom-up inlining (inner inlined into middle, middle into outer) with the
# raise crossing multiple inlined boundaries. Byte-identical to CPython.


def inner(seq, x):
    return seq[x]


def middle(seq, x):
    return inner(seq, x) + 1


def outer(seq, x):
    return middle(seq, x) * 2


data = [10, 20, 30]
for i in [0, 1, 2, 5, -1]:
    try:
        print(outer(data, i))
    except IndexError:
        print("index error at", i)
