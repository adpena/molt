"""Purpose: differential coverage for boolean or/and value semantics.

Tests that `or` and `and` return the correct operand (not just True/False),
covering bool-bool and int operands.
"""

if __name__ == "__main__":
    # Bool-bool cases
    print(False or True)
    print(True or False)
    print(False or False)
    print(True or True)
    print(True and False)
    print(True and True)
    print(False and True)
    print(False and False)

    # Integer operands (return the deciding value, not a bool)
    print(0 or 42)
    print(1 and 2)
    print(0 and 2)

    # Multi-value chains
    print(False or 0 or 42)
    print(True and 1 and 0)

    # Mixed and/or (operator precedence: `and` binds tighter)
    print(False or True and True)
    print(True or False and False)
    print(0 or 1 and 2)
    print(1 and 0 or 3)
