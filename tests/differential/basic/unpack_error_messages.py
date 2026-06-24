"""Purpose: differential coverage for exact unpacking error messages."""


class PlainObject:
    pass


class IterTooMany:
    def __iter__(self):
        return iter([1, 2, 3])


def report(label, fn):
    try:
        fn()
    except Exception as exc:
        print(label, type(exc).__name__, repr(str(exc)))
    else:
        print(label, "NO_ERROR")


def list_too_many():
    a, b = [1, 2, 3]


def tuple_too_many():
    a, b = (1, 2, 3)


def iter_too_many():
    a, b = IterTooMany()


def list_too_few():
    a, b, c = [1]


def starred_too_few():
    a, *b, c = [1]


def int_noniterable():
    a, b = 5


def float_noniterable():
    a, b = 1.5


def none_noniterable():
    a, b = None


def object_noniterable():
    a, b = object()


def custom_noniterable():
    a, b = PlainObject()


if __name__ == "__main__":
    report("list_too_many", list_too_many)
    report("tuple_too_many", tuple_too_many)
    report("iter_too_many", iter_too_many)
    report("list_too_few", list_too_few)
    report("starred_too_few", starred_too_few)
    report("int_noniterable", int_noniterable)
    report("float_noniterable", float_noniterable)
    report("none_noniterable", none_noniterable)
    report("object_noniterable", object_noniterable)
    report("custom_noniterable", custom_noniterable)
