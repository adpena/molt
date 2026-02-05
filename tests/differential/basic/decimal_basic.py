"""Purpose: differential coverage for decimal basic."""

import decimal


ctx = decimal.getcontext().copy()
ctx.prec = 6
val = ctx.divide(decimal.Decimal("1"), decimal.Decimal("3"))
print(val)

q = decimal.Decimal("1.2345").quantize(decimal.Decimal("0.01"))
print(q)
