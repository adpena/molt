"""Purpose: differential coverage for unpack assignment error propagation."""


class BoomIter:
    def __init__(self, fail_after):
        self.count = 0
        self.fail_after = fail_after

    def __iter__(self):
        return self

    def __next__(self):
        if self.count == self.fail_after:
            raise RuntimeError("boom")
        self.count += 1
        return self.count


def plain_unpack():
    a = "unset"
    b = "unset"
    try:
        a, b = BoomIter(1)
    except Exception as exc:
        print("plain_err", type(exc).__name__)
    print("plain_vals", a, b)


def starred_unpack():
    a = "unset"
    b = "unset"
    try:
        a, *rest, b = BoomIter(2)
    except Exception as exc:
        print("star_err", type(exc).__name__)
    print("star_vals", a, b)
    print("star_rest_set", "rest" in locals())


plain_unpack()
starred_unpack()
