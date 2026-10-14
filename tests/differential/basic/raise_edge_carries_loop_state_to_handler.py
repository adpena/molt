"""Purpose: a raise inside a loop gives the handler the current iteration's state.

Guards raise-edge canonicalization that drops block-argument stores (HF-214).
"""

def f(n):
    acc = 0
    last = None
    try:
        for i in range(n):
            acc += i
            last = i
            if i == 3:
                raise ValueError(acc)
    except ValueError as e:
        return acc, last, e.args
    return acc, last, None


print(f(10))
print(f(2))
