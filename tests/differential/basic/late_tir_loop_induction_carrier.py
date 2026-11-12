"""Purpose: a loop whose induction variable is a proven inline int keeps
CPython's results when the late-TIR stage expands the other integer ops of
its body (HF-821). The accumulators cross the inline-int window and the i64
range; the induction variable feeds sums, comparisons, breaks and the code
after the loop."""

LIMIT = 2**46


def sum_to(n):
    total = 0
    i = 0
    while i < n:
        total = total + i
        i = i + 1
    return total, i


def scaled(n, scale):
    total = 0
    i = 0
    while i < n:
        total = total + i * scale
        i = i + 1
    return total


def literal_bound():
    total = LIMIT - 40
    i = 0
    while i < 100:
        total = total + i
        i = i + 1
    return total, i


def nested(rows, cols):
    total = 0
    r = 0
    while r < rows:
        c = 0
        while c < cols:
            total = total + r * cols + c
            c = c + 1
        r = r + 1
    return total, r


def breaks_at(n, stop):
    total = 0
    i = 0
    while i < n:
        if total > stop:
            break
        total = total + i
        i = i + 1
    return total, i


def counts_down(n):
    total = 0
    i = n
    while i > 0:
        total = total - i
        i = i - 1
    return total, i


def for_range(n):
    total = LIMIT - 10
    last = -1
    for i in range(n):
        total += i
        last = i
    return total, last


def mixed_types(n):
    total = 0
    i = 0
    while i < n:
        total = total + i
        if i == 3:
            total = total + 0.5
        i = i + 1
    return total, i


print(sum_to(0), sum_to(1), sum_to(1000))
print(scaled(200, 2**40), scaled(200, 2**62), scaled(50, -(2**62)))
print(literal_bound())
print(nested(7, 9), nested(0, 5), nested(3, 0))
print(breaks_at(1000, 300), breaks_at(10, 10**9))
print(counts_down(500), counts_down(0))
print(for_range(0), for_range(1), for_range(64))
print(mixed_types(8))
