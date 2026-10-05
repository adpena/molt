"""Purpose: differential coverage for raise from context."""


def raise_from() -> None:
    try:
        raise KeyError("x")
    except Exception as exc:
        raise ValueError("boom") from exc


def raise_context() -> None:
    try:
        raise KeyError("x")
    except Exception:
        try:
            raise RuntimeError("inner")
        except Exception:
            raise ValueError("outer")


try:
    raise_from()
except Exception as exc:
    print(exc.__cause__ is None)
    print(exc.__context__ is None)
    print(exc.__cause__ is exc.__context__)
    print(exc.__suppress_context__ is True)


def class_in_handler():
    try:
        raise KeyError("context")
    except KeyError:
        raise ValueError


def class_from_none():
    raise ValueError from None


def instance_from_class():
    raise ValueError("instance") from KeyError


events = []


class ConstructedError(Exception):
    def __init__(self):
        events.append("construct")


def cause_expression():
    events.append("cause-expression")
    return KeyError


def evaluated_before_construction():
    raise ConstructedError from cause_expression()


def invalid_operand():
    raise "invalid"


def invalid_cause():
    raise ValueError from 1


class InvalidConstruction(Exception):
    def __new__(cls):
        return 42


def invalid_constructor():
    raise InvalidConstruction


def generator_class():
    yield "suspend"
    raise ValueError from KeyError


async def coroutine_class():
    raise ValueError from None


def resume_generator():
    iterator = generator_class()
    next(iterator)
    next(iterator)


def resume_coroutine():
    coroutine_class().send(None)


for label, action in (
    ("class-handler", class_in_handler),
    ("class-none", class_from_none),
    ("instance-class", instance_from_class),
    ("evaluation-order", evaluated_before_construction),
    ("invalid-operand", invalid_operand),
    ("invalid-cause", invalid_cause),
    ("invalid-constructor", invalid_constructor),
    ("generator-class", resume_generator),
    ("coroutine-class", resume_coroutine),
):
    try:
        action()
    except BaseException as exc:
        print(label, type(exc).__name__, str(exc))
        print(type(exc.__cause__).__name__, type(exc.__context__).__name__, exc.__suppress_context__)
print(events)

try:
    raise_context()
except Exception as exc:
    print(exc.__cause__ is None)
    print(exc.__context__ is None)
    print(exc.__cause__ is exc.__context__)
    print(exc.__suppress_context__ is True)
