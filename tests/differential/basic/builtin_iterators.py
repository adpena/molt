"""Purpose: differential coverage for builtin iterators."""

print(list(map(lambda x: x * 2, [1, 2, 3])))
print(list(map(lambda x: x, [1, 2, 3])))
print(list(map(lambda x, y: (x, y), [1, 2], ["a", "b", "c"])))
print(list(filter(None, [0, 1, "", 2])))
print(list(filter(lambda x: x % 2 == 0, [1, 2, 3, 4])))
print(list(zip([1, 2], [3, 4, 5])))
print(list(zip()))
print(list(reversed([1, 2, 3])))
print(list(reversed((1, 2, 3))))


counter = [0]


def next_val():
    counter[0] = counter[0] + 1
    return counter[0]


print(list(iter(next_val, 3)))


def expect_error(fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001 - intentional for parity checks
        print(type(exc).__name__)


expect_error(lambda: iter(1, 2))
expect_error(lambda: map(lambda x: x))
expect_error(lambda: list(filter(1, [1])))
expect_error(lambda: zip(1))
expect_error(lambda: reversed(1))
