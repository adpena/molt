# An inlinable leaf callee that raises, with the call wrapped in the caller's
# own try/except. After the TIR inliner splices the callee body in, the inlined
# raise must still be caught by the CALLER's handler — i.e. the inlined callee's
# exception exit must re-enter the caller's post-call exception observation, not
# return out of the merged function. Byte-identical to CPython 3.12/3.13/3.14.


def divide(a, b):
    return a // b


def run(a, b):
    try:
        return divide(a, b)
    except ZeroDivisionError:
        return -1


print(run(10, 2))
print(run(10, 0))
print(run(7, 3))
print(run(100, 0))
print(run(-9, 2))
