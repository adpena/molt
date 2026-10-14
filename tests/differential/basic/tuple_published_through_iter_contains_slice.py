"""Purpose: a tuple published through iter, __contains__ or slicing keeps its identity.

Guards escape analysis that treats these uses as non-escaping (HF-214).
"""

saved = []


class Box:
    def __contains__(self, item):
        saved.append(item)
        return True


def via_iter(a, b):
    t = (a, b)
    return iter(t)


def via_contains(a, b):
    t = (a, b)
    return t in Box()


def via_slice(a, b):
    t = (a, b)
    return t[:]


it = via_iter([1], [2])
print(list(it))
print(via_contains(3, 4), saved)
s = via_slice(5, 6)
print(s, len(s))
