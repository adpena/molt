"""Purpose: differential coverage for __len__ + __getitem__ iteration.

Behavior: iteration must stop on IndexError, not on __len__.
Parity: CPython ignores __len__ when iterating via __getitem__.
Pitfalls: unbounded __getitem__ can OOM; use islice to cap output.
"""

import itertools

class Seq:
    def __len__(self):
        return 3

    def __getitem__(self, index):
        return index


if __name__ == "__main__":
    # Avoid unbounded iteration while still verifying __len__ doesn't cap __getitem__ iteration.
    print("values", list(itertools.islice(Seq(), 5)))
