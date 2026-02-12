"""Purpose: ensure doctest finds and runs simple examples."""

import doctest


def add(a: int, b: int) -> int:
    """Add two numbers.

    >>> add(1, 2)
    3
    """

    return a + b


result = doctest.testmod()
print(result.failed, result.attempted)
