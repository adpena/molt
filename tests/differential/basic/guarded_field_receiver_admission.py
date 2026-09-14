"""Guarded fields admit only receivers with the expected class layout."""


class Expected:
    guarded: int
    bit_length: object

    def __init__(self):
        self.guarded = 11
        self.bit_length = None


class Other:
    def __init__(self):
        self.guarded = 21


def scalar_as_expected() -> Expected:
    return 3


def other_as_expected() -> Expected:
    return Other()


def expected_as_expected() -> Expected:
    return Expected()


try:
    scalar_as_expected().guarded
except AttributeError as exc:
    print("scalar-read", type(exc).__name__)

try:
    scalar_as_expected().guarded = 12
except AttributeError as exc:
    print("scalar-write", type(exc).__name__)

scalar_method = scalar_as_expected().bit_length
print("scalar-bit-length", scalar_method())

other = other_as_expected()
print("wrong-class-read", other.guarded)
other.guarded = 22
print("wrong-class-write", other.guarded)

expected = expected_as_expected()
print("expected-read", expected.guarded)
expected.guarded = 12
print("expected-write", expected.guarded)
