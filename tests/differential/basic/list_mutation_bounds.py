# A list mutated between a bounds proof and an index must still raise IndexError.
def after_pop():
    xs = [1, 2, 3]
    xs.pop()
    try:
        return xs[2]
    except IndexError:
        return "IndexError"


def checked_then_cleared():
    xs = [10, 20, 30]
    i = 0
    seen = []
    while i < len(xs):
        xs.clear()
        try:
            seen.append(xs[i])
        except IndexError:
            seen.append("IndexError")
        i += 1
    return seen


def after_clear():
    xs = [5] * 3
    xs.clear()
    try:
        return xs[0]
    except IndexError:
        return "IndexError"


def pop_in_loop():
    xs = [1, 2, 3, 4]
    out = []
    for i in range(len(xs)):
        if i == 1:
            del xs[2:]
        try:
            out.append(xs[i])
        except IndexError:
            out.append("IndexError")
    return out


print(after_pop())
print(checked_then_cleared())
print(after_clear())
print(pop_in_loop())
