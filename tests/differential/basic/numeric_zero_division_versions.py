"""Version-bound numeric errors, signed division, bool and timedelta satellites.

Output is compared with the matched CPython minor; no expected-message fallback
or version normalization is allowed to hide a semantic mismatch.
"""
import operator
from datetime import timedelta

values = (True, 7, -7, 10**80, 7.0, -7.0, 7+2j)
zeros = (False, 0, 0.0, -0.0, 0j)
for name, operation in (("div", operator.truediv), ("floor", operator.floordiv), ("mod", operator.mod), ("divmod", divmod), ("idiv", operator.itruediv), ("ifloor", operator.ifloordiv), ("imod", operator.imod)):
    for left in values:
        for right in zeros:
            try:
                result = operation(left, right)
                print(name, type(left).__name__, repr(right), "RETURN", repr(result))
            except Exception as error:
                print(name, type(left).__name__, repr(right), type(error).__name__, str(error))
for base in (0, 0.0, -0.0, 0j):
    for exponent in (-1, -1.0, -1+0j, 1j):
        try:
            print("power", repr(base), repr(exponent), "RETURN", repr(pow(base, exponent)))
        except Exception as error:
            print("power", repr(base), repr(exponent), type(error).__name__, str(error))
for exponent in (-1, 0, 1, 10):
    try:
        print("modpow", exponent, "RETURN", pow(7, exponent, 0))
    except Exception as error:
        print("modpow", exponent, type(error).__name__, str(error))
for left in (-2**63, -7, -1, 0, 1, 7, 2**63-1, 10**80):
    for right in (-3, -1, True, 3):
        print("signed", left, right, left//right, left%right, divmod(left, right))
for left in (timedelta(microseconds=7), timedelta(microseconds=-7), timedelta.max, timedelta.min):
    for right in (True, False, -3, -1, 3, timedelta(microseconds=-3), timedelta(0)):
        for name, operation in (("floor", operator.floordiv), ("mod", operator.mod), ("div", operator.truediv)):
            try:
                print("timedelta", repr(left), repr(right), name, "RETURN", repr(operation(left, right)))
            except Exception as error:
                print("timedelta", repr(left), repr(right), name, type(error).__name__, str(error))

print("modpow_minimum_signed", pow(3, -2**63, 7))
for bucket in (-3, 3):
    data = {}
    header = True
    for line in ["header", "7|X|A|X|2", "-7|X|A|X|3"]:
        if header:
            header = False
            continue
        x = line.split("|")
        if x[0] == "END" or x[4] == "ENDP":
            continue
        timestamp = int(x[0])
        symbol = x[2]
        volume = int(x[4])
        series = data.setdefault(symbol, [])
        series.append((timestamp // bucket, volume))
    print("fused_taq", bucket, data)
