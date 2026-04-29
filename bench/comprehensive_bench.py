"""
Comprehensive benchmark suite for molt vs CPython.

Tests every major operation category:
  1. Integer arithmetic (tight loops)
  2. Float arithmetic (numerical computation)
  3. List operations (create, index, iterate, comprehension)
  4. Dict operations (build, lookup, iterate)
  5. String operations (build, methods, format)
  6. Function calls (recursive, iterative, closures)
  7. Object/class operations (attribute access, method dispatch)
  8. Control flow (branches, exceptions, generators)
  9. Memory patterns (allocation, deallocation, large structures)

Usage:
    python3 bench/comprehensive_bench.py          # CPython
    molt build bench/comprehensive_bench.py ...   # molt
"""

import time


def bench(name, func, *args):
    """Run a benchmark and print results."""
    # warmup
    func(*args)
    # measure
    start = time.time()
    result = func(*args)
    elapsed = (time.time() - start) * 1000
    print(f"  {name:40s} {elapsed:8.2f}ms")
    return result


# ---------------------------------------------------------------------------
# 1. Integer arithmetic
# ---------------------------------------------------------------------------


def int_sum_loop(n):
    """Sum integers 0..n in a while loop."""
    total = 0
    i = 0
    while i < n:
        total += i
        i += 1
    return total


def int_sieve(n):
    """Sieve of Eratosthenes."""
    is_prime = [True] * (n + 1)
    is_prime[0] = is_prime[1] = False
    p = 2
    while p * p <= n:
        if is_prime[p]:
            i = p * p
            while i <= n:
                is_prime[i] = False
                i += p
        p += 1
    count = 0
    for x in is_prime:
        if x:
            count += 1
    return count


def int_fib_iter(n):
    """Iterative fibonacci."""
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a


# ---------------------------------------------------------------------------
# 2. Float arithmetic
# ---------------------------------------------------------------------------


def float_dot_product(n):
    """Dot product of two float lists."""
    a = [float(i) * 0.1 for i in range(n)]
    b = [float(i) * 0.2 for i in range(n)]
    total = 0.0
    for i in range(n):
        total += a[i] * b[i]
    return total


def float_mandelbrot(size):
    """Mandelbrot set computation."""
    count = 0
    for y in range(size):
        for x in range(size):
            cr = (x - size * 3 // 4) / (size // 2)
            ci = (y - size // 2) / (size // 2)
            zr = zi = 0.0
            i = 0
            while i < 50:
                zr2 = zr * zr
                zi2 = zi * zi
                if zr2 + zi2 > 4.0:
                    break
                zi = 2.0 * zr * zi + ci
                zr = zr2 - zi2 + cr
                i += 1
            if i == 50:
                count += 1
    return count


# ---------------------------------------------------------------------------
# 3. List operations
# ---------------------------------------------------------------------------


def list_comprehension(n):
    """List comprehension."""
    return sum([x * x for x in range(n)])


def list_sort(n):
    """Sort a reversed list."""
    a = list(range(n, 0, -1))
    a.sort()
    return a[0]


def list_nested_loop(n):
    """Nested list access."""
    matrix = [[i * n + j for j in range(n)] for i in range(n)]
    total = 0
    for row in matrix:
        for val in row:
            total += val
    return total


# ---------------------------------------------------------------------------
# 4. Dict operations
# ---------------------------------------------------------------------------


def dict_build_and_lookup(n):
    """Build dict then look up every key."""
    d = {}
    for i in range(n):
        d[i] = i * i
    total = 0
    for i in range(n):
        total += d[i]
    return total


def dict_counter(words):
    """Word frequency counting."""
    counts = {}
    for w in words:
        if w in counts:
            counts[w] += 1
        else:
            counts[w] = 1
    return len(counts)


# ---------------------------------------------------------------------------
# 5. String operations
# ---------------------------------------------------------------------------


def string_join(n):
    """Build string via join."""
    parts = []
    for i in range(n):
        parts.append(str(i))
    return len(",".join(parts))


def string_split_count(text, n):
    """Split and count words."""
    total = 0
    for _ in range(n):
        total += len(text.split())
    return total


# ---------------------------------------------------------------------------
# 6. Function calls
# ---------------------------------------------------------------------------


def func_recursive_fib(n):
    """Recursive fibonacci (tests call overhead)."""
    if n <= 1:
        return n
    return func_recursive_fib(n - 1) + func_recursive_fib(n - 2)


def func_closure(n):
    """Closure creation and calling."""

    def make_adder(x):
        def add(y):
            return x + y

        return add

    total = 0
    for i in range(n):
        f = make_adder(i)
        total += f(i)
    return total


# ---------------------------------------------------------------------------
# 7. Object/class operations
# ---------------------------------------------------------------------------


def class_attr_access(n):
    """Attribute access on objects."""

    class Point:
        def __init__(self, x, y):
            self.x = x
            self.y = y

        def distance(self):
            return self.x * self.x + self.y * self.y

    total = 0
    for i in range(n):
        p = Point(i, i + 1)
        total += p.distance()
    return total


# ---------------------------------------------------------------------------
# 8. Control flow
# ---------------------------------------------------------------------------


def control_exception_handling(n):
    """Try/except in a loop."""
    count = 0
    for i in range(n):
        try:
            if i % 1000 == 0:
                raise ValueError("test")
        except ValueError:
            count += 1
    return count


# ---------------------------------------------------------------------------
# Run all benchmarks
# ---------------------------------------------------------------------------

print("=" * 60)
print("Comprehensive Benchmark Suite")
print("=" * 60)

print("\n[Integer Arithmetic]")
bench("sum_loop(1000000)", int_sum_loop, 1000000)
bench("sieve(100000)", int_sieve, 100000)
bench("fib_iter(100000)", int_fib_iter, 100000)

print("\n[Float Arithmetic]")
bench("dot_product(100000)", float_dot_product, 100000)
bench("mandelbrot(100)", float_mandelbrot, 100)

print("\n[List Operations]")
bench("comprehension(100000)", list_comprehension, 100000)
bench("sort(100000)", list_sort, 100000)
bench("nested_loop(300)", list_nested_loop, 300)

print("\n[Dict Operations]")
bench("build_and_lookup(100000)", dict_build_and_lookup, 100000)
words = "the quick brown fox jumps over the lazy dog".split() * 10000
bench("counter(90000 words)", dict_counter, words)

print("\n[String Operations]")
bench("join(100000)", string_join, 100000)
text = "the quick brown fox " * 100
bench("split_count(1000)", string_split_count, text, 1000)

print("\n[Function Calls]")
bench("recursive_fib(30)", func_recursive_fib, 30)
bench("closure(100000)", func_closure, 100000)

print("\n[Object/Class]")
bench("attr_access(100000)", class_attr_access, 100000)

print("\n[Control Flow]")
bench("exception_handling(100000)", control_exception_handling, 100000)

print("=" * 60)
