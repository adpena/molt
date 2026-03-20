"""Molt Python on Cloudflare Workers — Demo Application.

This Python program runs compiled to WASM on Cloudflare's edge network.
No interpreter. No cold start penalty. Just compiled Python.
"""


def fibonacci(n: int) -> int:
    """Compute the nth Fibonacci number."""
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a


def is_prime(n: int) -> bool:
    """Check if a number is prime."""
    if n < 2:
        return False
    if n < 4:
        return True
    if n % 2 == 0 or n % 3 == 0:
        return False
    i = 5
    while i * i <= n:
        if n % i == 0 or n % (i + 2) == 0:
            return False
        i += 6
    return True


def count_primes(limit: int) -> int:
    """Count primes up to limit."""
    count = 0
    for n in range(2, limit + 1):
        if is_prime(n):
            count += 1
    return count


# Entry point
print("Molt Python on Cloudflare Workers")
print("==================================")
print("")
print("Fibonacci sequence:")
for i in range(15):
    print("  fib(" + str(i) + ") = " + str(fibonacci(i)))
print("")
print("Prime counting:")
print("  primes up to 100: " + str(count_primes(100)))
print("  primes up to 1000: " + str(count_primes(1000)))
print("")
print("Runtime: molt (compiled Python -> WASM)")
print("Platform: Cloudflare Workers")
