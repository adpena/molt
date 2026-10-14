"""Purpose: an unused int + float that overflows still raises OverflowError.

Guards dead-op elimination against purity facts that call it non-throwing (HF-214).
"""

def f(n):
    x = 10 ** n
    x + 1.0
    return "no error"


try:
    print(f(400))
except OverflowError as e:
    print("OverflowError:", e)
