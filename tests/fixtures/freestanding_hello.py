"""Minimal pure-computation program for freestanding wasm target.

No I/O, no randomness, no time -- just computation and a return value.
"""

def fibonacci(n: int) -> int:
    a, b = 0, 1
    for _ in range(n):
        a, b = b, a + b
    return a

result = fibonacci(10)
# result should be 55
