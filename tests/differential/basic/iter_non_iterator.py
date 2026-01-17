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
