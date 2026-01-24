"""Purpose: differential coverage for iter non iterator."""


def show(label, value):
    print(label, value)


class BadIter:
    def __iter__(self):
        return []


try:
    iter(BadIter())
except TypeError as exc:
    show("iter_bad_list", str(exc))


class IterGood:
    def __iter__(self):
        return self

    def __next__(self):
        return 3


it = iter(IterGood())
show("iter_good_value", next(it))


class NextOnly:
    def __next__(self):
        return 7


next_only = NextOnly()
show("next_only_value", next(next_only))
try:
    iter(next_only)
except TypeError as exc:
    show("next_only_iter_error", str(exc))
