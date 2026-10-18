# Parity test: error handling edge cases
# All output via print() for diff comparison

print("=== Exception hierarchy ===")
print(issubclass(ValueError, Exception))
print(issubclass(TypeError, Exception))
print(issubclass(KeyError, LookupError))
print(issubclass(IndexError, LookupError))
print(issubclass(FileNotFoundError, OSError))
print(issubclass(PermissionError, OSError))
print(issubclass(ZeroDivisionError, ArithmeticError))
print(issubclass(OverflowError, ArithmeticError))
print(issubclass(Exception, BaseException))
print(issubclass(KeyboardInterrupt, BaseException))
print(not issubclass(KeyboardInterrupt, Exception))

print("=== Custom exceptions with args ===")


class AppError(Exception):
    def __init__(self, code, message, details=None):
        super().__init__(message)
        self.code = code
        self.details = details


try:
    raise AppError(404, "not found", {"path": "/missing"})
except AppError as e:
    print(f"code={e.code}")
    print(f"message={e}")
    print(f"details={e.details}")
    print(f"args={e.args}")

print("=== Exception multi-arg ===")
e = Exception("a", "b", "c")
print(e.args)
print(str(e))
e2 = Exception()
print(e2.args)
print(str(e2))

print("=== Exception chaining (from) ===")
try:
    try:
        1 / 0
    except ZeroDivisionError as e:
        raise ValueError("bad math") from e
except ValueError as e:
    print(f"caught: {e}")
    print(f"cause type: {type(e.__cause__).__name__}")
    print(f"cause: {e.__cause__}")

print("=== Exception chaining (from None) ===")
try:
    try:
        {}["missing"]
    except KeyError:
        raise RuntimeError("clean error") from None
except RuntimeError as e:
    print(f"caught: {e}")
    print(f"cause: {e.__cause__}")
    print(f"suppress: {e.__suppress_context__}")

print("=== Implicit chaining (__context__) ===")
try:
    try:
        raise ValueError("first")
    except ValueError:
        raise TypeError("second")
except TypeError as e:
    print(f"caught: {e}")
    print(f"context type: {type(e.__context__).__name__}")
    print(f"context: {e.__context__}")
    print(f"cause: {e.__cause__}")

print("=== finally guarantees ===")
results = []


def test_finally_return():
    try:
        results.append("try")
        return "from-try"
    finally:
        results.append("finally")


ret = test_finally_return()
print(ret)
print(results)

print("=== finally with exception ===")
results2 = []
try:
    try:
        results2.append("try")
        raise ValueError("boom")
    finally:
        results2.append("finally")
except ValueError:
    results2.append("except")
print(results2)

print("=== finally overrides return ===")


def finally_overrides():
    try:
        return "try"
    finally:
        return "finally"


print(finally_overrides())

print("=== Nested try/except ===")


def nested_errors(level):
    try:
        if level == 0:
            raise ValueError("level 0")
        try:
            if level == 1:
                raise TypeError("level 1")
            raise RuntimeError(f"level {level}")
        except TypeError as e:
            return f"inner caught: {e}"
    except ValueError as e:
        return f"outer caught: {e}"
    except RuntimeError as e:
        return f"outer caught runtime: {e}"


print(nested_errors(0))
print(nested_errors(1))
print(nested_errors(2))

print("=== Exception in context manager ===")


class SafeBlock:
    def __init__(self, suppress_types):
        self.suppress_types = suppress_types

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type and issubclass(exc_type, self.suppress_types):
            print(f"suppressed: {exc_type.__name__}: {exc_val}")
            return True
        return False


with SafeBlock((ValueError, TypeError)):
    raise ValueError("v")

with SafeBlock((ValueError, TypeError)):
    raise TypeError("t")

try:
    with SafeBlock((ValueError,)):
        raise RuntimeError("r")
except RuntimeError as e:
    print(f"propagated: {e}")

print("=== Re-raise preserves exception ===")
try:
    try:
        raise ValueError("original")
    except ValueError:
        raise
except ValueError as e:
    print(f"re-raised: {e}")

print("=== Exception group pattern (manual) ===")
errors = []
for i, val in enumerate(["1", "abc", "3", "xyz", "5"]):
    try:
        int(val)
    except ValueError as e:
        errors.append((i, str(e)))

print(f"errors collected: {len(errors)}")
for idx, msg in errors:
    print(f"  index {idx}: {msg}")

print("=== Custom exception with __str__ ===")


class DetailedError(Exception):
    def __init__(self, operation, reason):
        self.operation = operation
        self.reason = reason

    def __str__(self):
        return f"{self.operation} failed: {self.reason}"


try:
    raise DetailedError("connect", "timeout")
except DetailedError as e:
    print(e)
    print(repr(e))

print("=== except* -like pattern (sequential) ===")


def run_tasks():
    results = []
    errors = []
    for i in range(5):
        try:
            if i % 2 == 0:
                results.append(i * 10)
            else:
                raise ValueError(f"task {i} failed")
        except ValueError as e:
            errors.append(str(e))
    return results, errors


r, e = run_tasks()
print(f"results: {r}")
print(f"errors: {e}")

print("=== Assertion error ===")
try:
    assert 1 == 2, "one is not two"
except AssertionError as e:
    print(f"AssertionError: {e}")

try:
    assert False
except AssertionError as e:
    print(f"no message: args={e.args}")

print("=== Exception as condition ===")


def safe_divide(a, b):
    try:
        return ("ok", a / b)
    except ZeroDivisionError:
        return ("error", None)


print(safe_divide(10, 3))
print(safe_divide(10, 0))
print(safe_divide(0, 5))
