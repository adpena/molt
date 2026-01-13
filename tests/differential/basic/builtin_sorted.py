print(sorted([3, 1, 2]))
print(sorted((3, 1, 2), reverse=True))
print(sorted(["b", "a", "c"]))
print(sorted([("b", 2), ("a", 3), ("b", 1)], key=lambda x: x[0]))
print(sorted([3, 1, 2], key=lambda x: -x))
print(sorted([1, 2, 3], reverse="yes"))


def expect_error(fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001 - intentional for parity checks
        print(type(exc).__name__)


expect_error(lambda: sorted())
expect_error(lambda: sorted(1))
expect_error(lambda: sorted([1], key=1))
expect_error(lambda: sorted([1, "a"]))
