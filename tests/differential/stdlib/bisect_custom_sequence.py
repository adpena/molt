"""Purpose: differential coverage for bisect generic sequence semantics."""

import bisect


class Seq:
    def __init__(self, values):
        self._values = list(values)

    def __len__(self):
        return len(self._values)

    def __getitem__(self, idx):
        return self._values[idx]


class Box:
    def __init__(self, values):
        self._values = list(values)

    def __len__(self):
        return len(self._values)

    def __getitem__(self, idx):
        return self._values[idx]

    def insert(self, idx, value):
        self._values.insert(idx, value)


seq = Seq([1, 2, 2, 3])
print(bisect.bisect_left(seq, 2))
print(bisect.bisect_right(seq, 2))

box = Box([1, 3])
bisect.insort_left(box, 2)
print(box._values)

class KeyItem:
    def __init__(self, x):
        self.x = x


box2 = Box([KeyItem(1), KeyItem(3)])
bisect.insort_right(box2, KeyItem(2), key=lambda item: item.x)
print([item.x for item in box2._values])
