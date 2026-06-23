"""Helper module for imported_generator_lowering."""


def ladder(limit):
    current = 0
    while current < limit:
        yield current
        current = current + 1


def nested(limit):
    for value in ladder(limit):
        yield value * 10
    yield from ladder(2)


def consume(iterator):
    total = 0
    for value in iterator:
        total = total + value
    return total
