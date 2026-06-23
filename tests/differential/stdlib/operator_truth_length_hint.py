"""Purpose: differential coverage for operator.truth and operator.length_hint."""

import operator


class Seq:
    def __len__(self):
        return 3


class LenAndHint:
    def __len__(self):
        return 3

    def __length_hint__(self):
        return 7


class HintOnly:
    def __length_hint__(self):
        return 4


class Empty:
    pass


class LenTypeErrorAndHint:
    def __len__(self):
        raise TypeError("bad len")

    def __length_hint__(self):
        return 5


class LenValueErrorAndHint:
    def __len__(self):
        raise ValueError("bad len")

    def __length_hint__(self):
        return 6


class NegativeLenAndHint:
    def __len__(self):
        return -1

    def __length_hint__(self):
        return 8


if __name__ == "__main__":
    print("truth", operator.truth(Seq()))
    print("length", operator.length_hint([1, 2, 3]))
    print("default_zero", operator.length_hint(Empty()))
    print("hint_only", operator.length_hint(HintOnly()))
    print("len_wins", operator.length_hint(LenAndHint()))
    print("len_type_error_falls_back", operator.length_hint(LenTypeErrorAndHint()))
    for label, value in (
        ("len_value_error", LenValueErrorAndHint()),
        ("negative_len", NegativeLenAndHint()),
    ):
        try:
            print(label, operator.length_hint(value))
        except Exception as exc:
            print(label + "_raises", type(exc).__name__, str(exc))
