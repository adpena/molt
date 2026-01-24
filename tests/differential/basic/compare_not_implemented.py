"""Purpose: differential coverage for compare not implemented."""


class Left:
    def __lt__(self, other):
        return NotImplemented


class Right:
    def __gt__(self, other):
        return True


print(Left() < Right())


class Both:
    def __lt__(self, other):
        return NotImplemented

    def __gt__(self, other):
        return NotImplemented


def expect_error(fn):
    try:
        fn()
    except Exception as exc:  # noqa: BLE001 - intentional for parity checks
        print(type(exc).__name__)


expect_error(lambda: Both() < Both())
expect_error(lambda: Both() > Both())


class ReverseFalse:
    def __lt__(self, other):
        return NotImplemented

    def __gt__(self, other):
        return False


print(ReverseFalse() < ReverseFalse())
print(NotImplemented is NotImplemented)
print(type(NotImplemented))
