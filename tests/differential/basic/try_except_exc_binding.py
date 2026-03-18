"""Purpose: exception handler name binding/cleanup must not leak or crash."""


def trigger() -> None:
    raise TypeError("boom")


try:
    trigger()
except Exception as exc:
    print(type(exc).__name__)

print("after-exc")

try:
    print(exc)  # noqa: F821
except NameError as exc:
    print(type(exc).__name__)
