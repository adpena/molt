# bench/pgo_training.py — PGO training corpus for molt-backend
# Exercises: integer arithmetic, float arithmetic, list operations,
# dict operations, string operations, function calls, classes,
# control flow, generators, comprehensions

# Integer arithmetic
def sieve(n):
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
    return sum(1 for x in is_prime if x)

# Float arithmetic
def mandelbrot(size):
    count = 0
    y = 0
    while y < size:
        x = 0
        while x < size:
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
            x += 1
        y += 1
    return count

# Recursive calls
def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)

# List operations
def list_ops(n):
    a = [i * i for i in range(n)]
    total = 0
    for x in a:
        total += x
    return total

# Dict operations
def dict_ops(n):
    d = {}
    for i in range(n):
        d[i] = i * i
    total = 0
    for k in d:
        total += d[k]
    return total

# String operations
def str_ops(n):
    parts = []
    for i in range(n):
        parts.append(str(i))
    return len(",".join(parts))

# Class operations
class Point:
    def __init__(self, x, y):
        self.x = x
        self.y = y
    def dist(self):
        return self.x * self.x + self.y * self.y

def class_ops(n):
    total = 0
    for i in range(n):
        p = Point(i, i + 1)
        total += p.dist()
    return total

# Run all
print(sieve(10000))
print(mandelbrot(50))
print(fib(25))
print(list_ops(10000))
print(dict_ops(10000))
print(str_ops(10000))
print(class_ops(10000))
