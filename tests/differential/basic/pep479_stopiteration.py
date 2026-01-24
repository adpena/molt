"""Purpose: differential coverage for PEP 479 StopIteration handling."""


def gen_direct():
    raise StopIteration("boom")
    yield 1


def gen_yield_from():
    def inner():
        raise StopIteration("inner")
        yield 1

    yield from inner()


for factory in (gen_direct, gen_yield_from):
    try:
        next(factory())
    except Exception as exc:
        print(type(exc).__name__, exc)
