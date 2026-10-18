# Parity test: decorators
# All output via print() for diff comparison

print("=== Basic function decorator ===")


def shout(func):
    def wrapper(*args, **kwargs):
        result = func(*args, **kwargs)
        return result.upper()

    return wrapper


@shout
def greet(name):
    return f"hello, {name}"


print(greet("world"))
print(greet("alice"))

print("=== Decorator preserving metadata ===")
import functools


def logged(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        print(f"calling {func.__name__}")
        return func(*args, **kwargs)

    return wrapper


@logged
def add(a, b):
    """Add two numbers."""
    return a + b


print(add(3, 4))
print(add.__name__)
print(add.__doc__)

print("=== Stacked decorators ===")


def bold(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        return f"**{func(*args, **kwargs)}**"

    return wrapper


def italic(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        return f"_{func(*args, **kwargs)}_"

    return wrapper


@bold
@italic
def say(text):
    return text


print(say("hello"))  # bold wraps italic wraps say


@italic
@bold
def say2(text):
    return text


print(say2("hello"))  # italic wraps bold wraps say2

print("=== Decorator factory (with arguments) ===")


def repeat(n):
    def decorator(func):
        @functools.wraps(func)
        def wrapper(*args, **kwargs):
            results = []
            for _ in range(n):
                results.append(func(*args, **kwargs))
            return results

        return wrapper

    return decorator


@repeat(3)
def ping():
    return "pong"


print(ping())


@repeat(1)
def single():
    return "once"


print(single())

print("=== Decorator with state ===")


def count_calls(func):
    @functools.wraps(func)
    def wrapper(*args, **kwargs):
        wrapper.calls += 1
        return func(*args, **kwargs)

    wrapper.calls = 0
    return wrapper


@count_calls
def process(x):
    return x * 2


process(1)
process(2)
process(3)
print(process.calls)

print("=== classmethod decorator ===")


class MyClass:
    instances = 0

    def __init__(self, name):
        self.name = name
        MyClass.instances += 1

    @classmethod
    def count(cls):
        return cls.instances

    @classmethod
    def create(cls, name):
        return cls(name)


a = MyClass.create("a")
b = MyClass.create("b")
print(MyClass.count())
print(a.name, b.name)

print("=== staticmethod decorator ===")


class MathHelper:
    @staticmethod
    def add(a, b):
        return a + b

    @staticmethod
    def is_even(n):
        return n % 2 == 0


print(MathHelper.add(3, 4))
print(MathHelper.is_even(4))
print(MathHelper.is_even(5))
m = MathHelper()
print(m.add(1, 2))

print("=== property decorator ===")


class Rectangle:
    def __init__(self, width, height):
        self._width = width
        self._height = height

    @property
    def width(self):
        return self._width

    @width.setter
    def width(self, value):
        if value < 0:
            raise ValueError("negative width")
        self._width = value

    @property
    def area(self):
        return self._width * self._height


r = Rectangle(5, 3)
print(r.width)
print(r.area)
r.width = 10
print(r.area)

try:
    r.width = -1
except ValueError as e:
    print(f"caught: {e}")

print("=== Class decorator ===")


def add_repr(cls):
    def __repr__(self):
        attrs = ", ".join(f"{k}={v!r}" for k, v in sorted(self.__dict__.items()))
        return f"{cls.__name__}({attrs})"

    cls.__repr__ = __repr__
    return cls


@add_repr
class User:
    def __init__(self, name, age):
        self.name = name
        self.age = age


u = User("Alice", 30)
print(u)

print("=== Class decorator factory ===")


def with_defaults(**defaults):
    def decorator(cls):
        original_init = cls.__init__

        @functools.wraps(original_init)
        def new_init(self, *args, **kwargs):
            for k, v in defaults.items():
                setattr(self, k, v)
            original_init(self, *args, **kwargs)

        cls.__init__ = new_init
        return cls

    return decorator


@with_defaults(created=True, version=1)
class Document:
    def __init__(self, title):
        self.title = title


doc = Document("test")
print(doc.title)
print(doc.created)
print(doc.version)

print("=== Method decorator ===")


def validate_positive(method):
    @functools.wraps(method)
    def wrapper(self, value):
        if value < 0:
            raise ValueError(f"expected positive, got {value}")
        return method(self, value)

    return wrapper


class Account:
    def __init__(self):
        self.balance = 0

    @validate_positive
    def deposit(self, amount):
        self.balance += amount
        return self.balance


acc = Account()
print(acc.deposit(100))
print(acc.deposit(50))
try:
    acc.deposit(-10)
except ValueError as e:
    print(f"caught: {e}")

print("=== Decorator on lambda-like (manual) ===")


def memoize(func):
    cache = {}

    @functools.wraps(func)
    def wrapper(*args):
        if args not in cache:
            cache[args] = func(*args)
        return cache[args]

    return wrapper


@memoize
def fib(n):
    if n < 2:
        return n
    return fib(n - 1) + fib(n - 2)


print(fib(10))
print(fib(20))
print(fib(0))
print(fib(1))
