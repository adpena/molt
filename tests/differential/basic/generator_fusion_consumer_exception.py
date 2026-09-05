"""A consumer exception must transfer before any post-yield producer effect."""


def values():
    index = 1
    while index >= 0:
        yield index
        print("producer resumed", index)
        index -= 1


try:
    # One generator instance admits fusion. A bare arithmetic body makes its
    # synchronous check the latch observation, without a later store/call check.
    for value in values():
        1 / value
except ZeroDivisionError as error:
    print("division", type(error).__name__, str(error))
