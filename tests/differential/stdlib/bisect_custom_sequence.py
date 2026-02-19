"""Purpose: differential coverage for bisect custom sequences."""

import bisect


class Seq:
    def __init__(self, items):
        self.data = list(items)
        self.inserts = []

    def __len__(self):
        return len(self.data)

    def __getitem__(self, idx):
        return self.data[idx]

    def insert(self, idx, value):
        self.inserts.append((idx, value))
        self.data.insert(idx, value)


seq = Seq([1, 3, 5])
print(bisect.bisect_left(seq, 3))
print(bisect.bisect_right(seq, 3))

bisect.insort_left(seq, 4)
print(seq.data)
print(seq.inserts)
