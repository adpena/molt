"""Purpose: a handler naming builtin exception classes matches by class, tuple or subclass."""


class AppError(ValueError):
    pass


def classify(error):
    try:
        raise error
    except KeyError:
        return "KeyError"
    except (TypeError, IndexError):
        return "TypeError|IndexError"
    except ValueError as caught:
        return "ValueError:" + type(caught).__name__
    except LookupError:
        return "LookupError"
    except ArithmeticError:
        return "ArithmeticError"
    except Exception:
        return "Exception"


for error in [
    KeyError("k"),
    TypeError("t"),
    IndexError(1),
    ValueError(2),
    AppError(3),
    UnicodeDecodeError("utf-8", b"\xff", 0, 1, "bad"),
    LookupError(),
    ZeroDivisionError(),
    OverflowError(),
    RuntimeError("r"),
    StopIteration(5),
]:
    print(classify(error))


def unmatched():
    try:
        try:
            raise OSError(2, "missing")
        except (ValueError, KeyError):
            return "wrong"
    except OSError as outer:
        return "outer:" + str(outer.errno)


print(unmatched())


def base_exit():
    try:
        raise SystemExit(4)
    except BaseException as caught:
        return type(caught).__name__ + ":" + str(caught.code)


print(base_exit())


def shadowed():
    ValueError = KeyError
    try:
        raise KeyError("shadow")
    except ValueError as caught:
        return "matched shadow " + repr(caught)


print(shadowed())


total = 0
for i in range(30):
    try:
        if i % 3 == 0:
            raise ValueError(i)
        if i % 5 == 0:
            raise KeyError(i)
        total += i
    except (KeyError, ValueError) as e:
        total += 1000 if isinstance(e, KeyError) else int(str(e))
print("total", total)

try:
    try:
        raise ExceptionGroup("group", [ValueError(1)])
    except ExceptionGroup as group:
        print("group", len(group.exceptions))
        raise TypeError("inner")
except TypeError as error:
    print("context", type(error.__context__).__name__)
