"""Module globals bound once to a constant, read by functions.

Purpose: a read of a global whose module binds it exactly once and no code
rebinds is the bound value. The fold must apply only to immediate constants
(int, bool, None, float) with no writer anywhere in the program; every other
global keeps its cell. Each case prints what CPython prints.
"""

SCALE = 3
NEGATIVE = -(1 << 46)
FLAG = True
NOTHING = None
HALF = 0.5
NAN = float("nan")
NEG_ZERO = -0.0
BIG = 1 << 80
TEXT = "text"
REBOUND = 10
DELETED = 7


def scaled_sum(n):
    total = 0
    for i in range(n):
        total += SCALE
    return total


def immediates():
    print(NEGATIVE, NEGATIVE - 1, FLAG, FLAG + 1, NOTHING is None, HALF * 4)
    print(NAN != NAN, NAN is NAN, str(NEG_ZERO), 1 / float("inf") == NEG_ZERO)
    print(SCALE is SCALE, type(SCALE).__name__, type(HALF).__name__, type(FLAG).__name__)


def heap_values():
    # Heap objects keep their identity across reads.
    print(BIG is BIG, BIG + 1, TEXT is TEXT, TEXT.upper())


def read_rebound():
    return REBOUND


def rebind():
    global REBOUND
    REBOUND = REBOUND + 1


def read_deleted():
    try:
        return DELETED
    except NameError as exc:
        return "NameError: " + str(exc)


def delete_it():
    global DELETED
    del DELETED


def main():
    print(scaled_sum(10), scaled_sum(0), scaled_sum(200000))
    immediates()
    heap_values()
    print(read_rebound())
    rebind()
    print(read_rebound())
    print(read_deleted())
    delete_it()
    print(read_deleted())


main()
