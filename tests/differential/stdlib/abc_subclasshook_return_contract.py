"""Purpose: differential coverage for abc.__subclasshook__ return contracts."""

import abc


EXPECTED_MSG = "__subclasshook__ must return either False, True, or NotImplemented"


class Candidate:
    pass


def expect_invalid_subclasshook_return(label, return_value):
    class Hooked(abc.ABC):
        @classmethod
        def __subclasshook__(cls, subclass):
            return return_value

    try:
        issubclass(Candidate, Hooked)
    except Exception as exc:  # noqa: BLE001
        print(label, type(exc).__name__, str(exc))
        assert type(exc) is AssertionError, (label, type(exc).__name__)
        assert str(exc) == EXPECTED_MSG, (label, str(exc))
    else:
        raise AssertionError(f"{label}: expected AssertionError")


for name, bad in (
    ("invalid-int", 1),
    ("invalid-str", "yes"),
    ("invalid-object", object()),
):
    expect_invalid_subclasshook_return(name, bad)


class HookTrue(abc.ABC):
    @classmethod
    def __subclasshook__(cls, subclass):
        return True


class HookFalse(abc.ABC):
    @classmethod
    def __subclasshook__(cls, subclass):
        return False


class HookNotImplemented(abc.ABC):
    @classmethod
    def __subclasshook__(cls, subclass):
        return NotImplemented


print("hook-true", issubclass(Candidate, HookTrue))
print("hook-false", issubclass(Candidate, HookFalse))
print("hook-notimpl-fallback", issubclass(Candidate, HookNotImplemented))
