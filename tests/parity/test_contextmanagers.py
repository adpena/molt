# Parity test: context managers
# All output via print() for diff comparison

print("=== Basic with statement ===")


class SimpleCM:
    def __enter__(self):
        print("enter")
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        print("exit")
        return False


with SimpleCM() as cm:
    print("body")
print(type(cm).__name__)

print("=== __enter__ return value ===")


class ValueCM:
    def __init__(self, val):
        self.val = val

    def __enter__(self):
        return self.val

    def __exit__(self, *args):
        return False


with ValueCM(42) as v:
    print(v)

with ValueCM([1, 2, 3]) as v:
    print(v)

print("=== Exception suppression ===")


class Suppressor:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        if exc_type is ValueError:
            print(f"suppressed ValueError: {exc_val}")
            return True
        return False


with Suppressor():
    raise ValueError("oops")
print("continued after suppression")

try:
    with Suppressor():
        raise TypeError("not suppressed")
except TypeError as e:
    print(f"propagated: {e}")

print("=== Exit receives exception info ===")


class Inspector:
    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc_val, exc_tb):
        print(f"exc_type: {exc_type}")
        print(f"exc_val: {exc_val}")
        print(f"exc_tb is None: {exc_tb is None}")
        return True


with Inspector():
    pass

with Inspector():
    raise RuntimeError("inspected")

print("=== Nested with statements ===")


class Named:
    def __init__(self, name):
        self.name = name

    def __enter__(self):
        print(f"enter {self.name}")
        return self

    def __exit__(self, *args):
        print(f"exit {self.name}")
        return False


with Named("outer"):
    with Named("inner"):
        print("innermost")

print("=== Multiple context managers ===")
with Named("A") as a, Named("B") as b:
    print(f"body: {a.name} {b.name}")

print("=== contextlib.contextmanager ===")
from contextlib import contextmanager


@contextmanager
def managed(name):
    print(f"setup {name}")
    try:
        yield name
    finally:
        print(f"cleanup {name}")


with managed("resource") as r:
    print(f"using {r}")

print("=== contextmanager with exception ===")


@contextmanager
def careful():
    print("acquiring")
    try:
        yield "handle"
    except ValueError as e:
        print(f"handled in cm: {e}")
    finally:
        print("releasing")


with careful() as h:
    print(f"got {h}")
    raise ValueError("cm error")
print("after cm error")

print("=== contextmanager cleanup on success ===")
with careful() as h:
    print(f"got {h}")
print("after success")

print("=== Exit order (LIFO) ===")


class Ordered:
    def __init__(self, n):
        self.n = n

    def __enter__(self):
        print(f"enter-{self.n}")
        return self

    def __exit__(self, *args):
        print(f"exit-{self.n}")
        return False


with Ordered(1):
    with Ordered(2):
        with Ordered(3):
            print("all entered")

print("=== Exception in __enter__ ===")


class FailEnter:
    def __enter__(self):
        raise RuntimeError("enter failed")

    def __exit__(self, *args):
        print("exit called")
        return False


try:
    with FailEnter():
        print("should not reach")
except RuntimeError as e:
    print(f"caught: {e}")

print("=== Exception in __exit__ ===")


class FailExit:
    def __enter__(self):
        return self

    def __exit__(self, *args):
        raise RuntimeError("exit failed")


try:
    with FailExit():
        pass
except RuntimeError as e:
    print(f"caught exit error: {e}")

print("=== Context manager as resource tracker ===")


class ResourceTracker:
    resources = []

    def __init__(self, name):
        self.name = name

    def __enter__(self):
        ResourceTracker.resources.append(self.name)
        return self

    def __exit__(self, *args):
        ResourceTracker.resources.remove(self.name)
        return False


with ResourceTracker("db"):
    with ResourceTracker("file"):
        print(sorted(ResourceTracker.resources))
    print(sorted(ResourceTracker.resources))
print(ResourceTracker.resources)
