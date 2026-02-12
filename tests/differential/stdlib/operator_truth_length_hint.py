"""Purpose: differential coverage for operator.truth and operator.length_hint."""

import operator


class Seq:
    def __len__(self):
        return 3


if __name__ == "__main__":
    print("truth", operator.truth(Seq()))
    print("length", operator.length_hint([1, 2, 3]))
