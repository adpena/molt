"""A counter in nested loops with a variable range step crosses 2**63.

A variable step makes the frontend emit one copy of the inner loop per step
sign, and every copy stores the same source variable. The copy that reads the
counter through that shared variable cannot keep it raw, so the checked add
in the versioned loop must take its exact boxed lane: no raw sum from boxed
operands, no wrapped count.
"""


def count_from_zero():
    step = 8
    count = 0
    for y in range(0, 64, step):
        for x in range(-50, 50, step):
            if x * y > 7:
                count += 1
    return count


def count_across_2_63():
    step = 5
    count = (1 << 63) - 20
    for y in range(0, 32, step):
        for x in range(-24, 24, step):
            for z in range(-24, 24, step):
                if x * y * z > 7:
                    count += 1
    return count


def count_from_minimum():
    step = 8
    count = -(1 << 63) + 3
    for y in range(0, 64, step):
        for x in range(-50, 50, step):
            if x * y > 7:
                count += 1
    return count


for result in (count_from_zero(), count_across_2_63(), count_from_minimum()):
    print(result, type(result).__name__)
