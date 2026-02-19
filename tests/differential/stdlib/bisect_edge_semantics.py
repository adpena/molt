"""Purpose: differential coverage for bisect edge semantics and error paths."""

import bisect


class Seq:
    def __init__(self, values):
        self.values = list(values)

    def __len__(self):
        return len(self.values)

    def __getitem__(self, idx):
        return self.values[idx]

    def insert(self, idx, value):
        self.values.insert(idx, value)


class IndexTwo:
    def __index__(self):
        return 2


class IndexBad:
    def __index__(self):
        return 1.25


seq = Seq([1, 3, 5])
print(bisect.bisect_left(seq, 4))
print(bisect.bisect_right(seq, 4, 0, IndexTwo()))
print(bisect.bisect_left([1], 1, 5))
print(bisect.bisect_right([1, 2, 3], 2, 2, 1))

try:
    bisect.bisect_left([1, 2, 3], 2, 0, IndexBad())
except Exception as exc:
    print(type(exc).__name__, exc)

try:
    bisect.bisect_left([1, 2, 3], 2, 0, 4)
except Exception as exc:
    print(type(exc).__name__, exc)

try:
    bisect.bisect_left([{"v": 1}], 1, key=lambda item: item["missing"])
except Exception as exc:
    print(type(exc).__name__, exc)

seq2 = Seq([{"v": 1}, {"v": 3}])
bisect.insort_left(seq2, {"v": 2}, key=lambda item: item["v"])
print([item["v"] for item in seq2.values])

seq3 = Seq([{"v": 1}, {"v": 2}, {"v": 2}, {"v": 3}])
bisect.insort_right(seq3, {"v": 2}, key=lambda item: item["v"])
print([item["v"] for item in seq3.values])
