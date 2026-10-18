# Parity test: functions
# All output via print() for diff comparison

print("=== Basic function ===")


def add(a, b):
    return a + b


print(add(2, 3))
print(add(0, 0))
print(add(-1, 1))

print("=== Default arguments ===")


def greet(name, greeting="Hello"):
    return f"{greeting}, {name}!"


print(greet("Alice"))
print(greet("Bob", "Hi"))

print("=== Mutable default (trap) ===")


def append_to(val, lst=None):
    if lst is None:
        lst = []
    lst.append(val)
    return lst


print(append_to(1))
print(append_to(2))
print(append_to(3, [10, 20]))

print("=== *args ===")


def varargs(*args):
    return args


print(varargs())
print(varargs(1))
print(varargs(1, 2, 3))

print("=== **kwargs ===")


def kwonly(**kwargs):
    return sorted(kwargs.items())


print(kwonly())
print(kwonly(a=1, b=2))

print("=== Mixed args ===")


def mixed(a, b, *args, key="default", **kwargs):
    return (a, b, args, key, sorted(kwargs.items()))


print(mixed(1, 2))
print(mixed(1, 2, 3, 4))
print(mixed(1, 2, 3, key="custom", extra=99))

print("=== Positional-only (PEP 570) ===")


def posonly(a, b, /, c, d):
    return a + b + c + d


print(posonly(1, 2, 3, 4))
print(posonly(1, 2, c=3, d=4))

print("=== Keyword-only ===")


def kwonly_args(a, *, b, c=10):
    return (a, b, c)


print(kwonly_args(1, b=2))
print(kwonly_args(1, b=2, c=3))

print("=== Lambda ===")


def square(x):
    return x * x


print(square(5))
print((lambda x, y: x + y)(3, 4))
print(sorted([3, 1, 2], key=lambda x: -x))

print("=== Closures ===")


def make_counter(start=0):
    count = start

    def increment():
        nonlocal count
        count += 1
        return count

    return increment


c = make_counter()
print(c())
print(c())
print(c())

c2 = make_counter(10)
print(c2())
print(c2())

print("=== Nested closures ===")


def outer(x):
    def middle(y):
        def inner(z):
            return x + y + z

        return inner

    return middle


print(outer(1)(2)(3))

print("=== Closure over loop variable ===")
funcs = []
for i in range(5):
    funcs.append(lambda i=i: i)
print([f() for f in funcs])

print("=== Decorators ===")


def double_result(func):
    def wrapper(*args, **kwargs):
        return func(*args, **kwargs) * 2

    return wrapper


@double_result
def compute(x):
    return x + 1


print(compute(5))

print("=== Decorator with arguments ===")


def multiply_by(factor):
    def decorator(func):
        def wrapper(*args, **kwargs):
            return func(*args, **kwargs) * factor

        return wrapper

    return decorator


@multiply_by(3)
def inc(x):
    return x + 1


print(inc(10))

print("=== Stacked decorators ===")


def add_prefix(func):
    def wrapper(*a, **kw):
        return "prefix_" + func(*a, **kw)

    return wrapper


def add_suffix(func):
    def wrapper(*a, **kw):
        return func(*a, **kw) + "_suffix"

    return wrapper


@add_prefix
@add_suffix
def get_name():
    return "hello"


print(get_name())

print("=== Generators ===")


def count_up(n):
    i = 0
    while i < n:
        yield i
        i += 1


print(list(count_up(5)))


def fib_gen(n):
    a, b = 0, 1
    for _ in range(n):
        yield a
        a, b = b, a + b


print(list(fib_gen(10)))

print("=== Generator expressions ===")
g = (x * x for x in range(5))
print(list(g))
print(sum(x for x in range(10)))

print("=== Generator send ===")


def accumulator():
    total = 0
    while True:
        val = yield total
        if val is None:
            break
        total += val


g = accumulator()
next(g)
print(g.send(10))
print(g.send(20))
print(g.send(5))

print("=== yield from ===")


def chain_gen(*iterables):
    for it in iterables:
        yield from it


print(list(chain_gen([1, 2], [3, 4], [5])))

print("=== Recursion ===")


def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)


print(fib(10))


def factorial(n):
    if n <= 1:
        return 1
    return n * factorial(n - 1)


print(factorial(10))

print("=== Higher-order functions ===")


def apply_twice(f, x):
    return f(f(x))


print(apply_twice(lambda x: x + 1, 5))
print(apply_twice(lambda x: x * 2, 3))

print("=== Function as return value ===")


def make_adder(n):
    def adder(x):
        return x + n

    return adder


add5 = make_adder(5)
print(add5(10))
print(add5(20))

print("=== Unpacking in calls ===")


def f(a, b, c):
    return a + b + c


args = [1, 2, 3]
print(f(*args))
kwargs = {"a": 10, "b": 20, "c": 30}
print(f(**kwargs))

print("=== Nested default evaluation ===")


def make_funcs():
    result = []
    for i in range(3):

        def f(x, i=i):
            return x + i

        result.append(f)
    return result


funcs = make_funcs()
print([f(10) for f in funcs])

print("=== Global / nonlocal ===")
_global_var = "initial"


def modify_global():
    global _global_var
    _global_var = "modified"


modify_global()
print(_global_var)


def outer_nl():
    x = 10

    def inner():
        nonlocal x
        x = 20

    inner()
    return x


print(outer_nl())
