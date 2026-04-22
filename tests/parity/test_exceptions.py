# Parity test: exceptions
# All output via print() for diff comparison

print("=== Basic raise/except ===")
try:
    raise ValueError("test error")
except ValueError as e:
    print(f"caught: {e}")

print("=== Exception types ===")
exceptions = [
    (ValueError, "bad value"),
    (TypeError, "bad type"),
    (KeyError, "bad key"),
    (IndexError, "bad index"),
    (AttributeError, "no attr"),
    (RuntimeError, "runtime"),
    (ZeroDivisionError, "div zero"),
    (NameError, "no name"),
    (FileNotFoundError, "no file"),
    (NotImplementedError, "not impl"),
    (OverflowError, "overflow"),
    (StopIteration, "stop"),
]
for exc_type, msg in exceptions:
    try:
        raise exc_type(msg)
    except Exception as e:
        print(f"{type(e).__name__}: {e}")

print("=== Multiple except clauses ===")
for val in [0, "abc", None, [1, 2]]:
    try:
        result = 10 / val
    except ZeroDivisionError:
        print(f"{val!r} -> ZeroDivisionError")
    except TypeError:
        print(f"{val!r} -> TypeError")

print("=== Except tuple ===")
try:
    raise KeyError("k")
except (ValueError, KeyError, TypeError) as e:
    print(f"caught tuple: {type(e).__name__}: {e}")

print("=== Bare except ===")
try:
    raise RuntimeError("oops")
except:
    print("bare except caught it")

print("=== Exception hierarchy ===")
try:
    raise FileNotFoundError("gone")
except OSError as e:
    print(f"OSError caught FileNotFoundError: {e}")

try:
    raise KeyError("k")
except LookupError as e:
    print(f"LookupError caught KeyError: {e}")

print("=== try/except/else/finally ===")
for val in [2, 0]:
    result = []
    try:
        result.append("try")
        x = 10 / val
    except ZeroDivisionError:
        result.append("except")
    else:
        result.append("else")
    finally:
        result.append("finally")
    print(f"val={val}: {result}")

print("=== Nested exceptions ===")
try:
    try:
        raise ValueError("inner")
    except ValueError:
        raise TypeError("converted")
except TypeError as e:
    print(f"outer caught: {e}")

print("=== Exception chaining (raise from) ===")
try:
    try:
        raise ValueError("original")
    except ValueError as e:
        raise RuntimeError("wrapper") from e
except RuntimeError as e:
    print(f"caught: {e}")
    print(f"cause: {e.__cause__}")
    print(f"cause type: {type(e.__cause__).__name__}")

print("=== Suppress context (from None) ===")
try:
    try:
        raise ValueError("original")
    except ValueError:
        raise RuntimeError("clean") from None
except RuntimeError as e:
    print(f"caught: {e}")
    print(f"cause: {e.__cause__}")
    print(f"suppress: {e.__suppress_context__}")

print("=== Exception args ===")
e = ValueError("a", "b", "c")
print(e.args)
print(len(e.args))
e = ValueError()
print(e.args)

print("=== Custom exception ===")


class MyError(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code = code


try:
    raise MyError(404, "not found")
except MyError as e:
    print(f"MyError: code={e.code}, msg={e}")

print("=== Custom exception hierarchy ===")


class AppError(Exception):
    pass


class DatabaseError(AppError):
    pass


class ConnectionError_(DatabaseError):
    pass


try:
    raise ConnectionError_("db down")
except AppError as e:
    print(f"caught as AppError: {type(e).__name__}: {e}")

print(issubclass(ConnectionError_, DatabaseError))
print(issubclass(ConnectionError_, AppError))
print(issubclass(ConnectionError_, Exception))

print("=== Finally always runs ===")


def test_finally(do_raise):
    result = []
    try:
        result.append("try")
        if do_raise:
            raise ValueError("boom")
        result.append("no-raise")
    except ValueError:
        result.append("except")
    finally:
        result.append("finally")
    return result


print(test_finally(True))
print(test_finally(False))

print("=== Finally with return ===")


def return_in_finally():
    try:
        return "try"
    finally:
        return "finally"


print(return_in_finally())

print("=== Re-raise ===")
try:
    try:
        raise ValueError("original")
    except ValueError:
        raise
except ValueError as e:
    print(f"re-raised: {e}")

print("=== Exception in except block ===")
try:
    try:
        raise ValueError("first")
    except ValueError:
        raise TypeError("second")
except TypeError as e:
    print(f"caught: {e}")
    print(f"context: {e.__context__}")

print("=== StopIteration in generator ===")


def gen():
    yield 1
    yield 2


g = gen()
print(next(g))
print(next(g))
try:
    next(g)
except StopIteration:
    print("generator exhausted")

print("=== Exception attributes ===")
try:
    {}["missing"]
except KeyError as e:
    print(f"KeyError args: {e.args}")

try:
    [1, 2, 3][10]
except IndexError as e:
    print(f"IndexError: {e}")

try:
    int("abc")
except ValueError as e:
    print(f"ValueError: {e}")

print("=== Exception notes (PEP 678) ===")
try:
    e = ValueError("original")
    e.add_note("note 1")
    e.add_note("note 2")
    raise e
except ValueError as e:
    print(f"caught: {e}")
    print(f"notes: {e.__notes__}")

print("=== Exception in with block ===")


class SuppressingCM:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is ValueError:
            print(f"suppressed: {exc_val}")
            return True
        return False


with SuppressingCM():
    raise ValueError("suppressed error")
print("continued after suppressed error")

with SuppressingCM():
    pass
print("no error case")

try:
    with SuppressingCM():
        raise TypeError("not suppressed")
except TypeError as e:
    print(f"propagated: {e}")

print("=== Assertion error ===")
try:
    assert False, "assertion message"
except AssertionError as e:
    print(f"AssertionError: {e}")

try:
    assert False
except AssertionError as e:
    print(f"AssertionError args: {e.args}")

print("=== Exception str/repr ===")
e = ValueError("test")
print(str(e))
print(repr(e))
e2 = ValueError(1, 2, 3)
print(str(e2))
print(repr(e2))
