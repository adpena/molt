# Regression: builtins reached inside a function (range/len/print/sum reduction)
# must not fail with "builtins module cache missing" on the LLVM backend, and a
# vectorized sum reduction over range(n) must compute the real total — not
# silently return the sequence object (range_new + vec_sum_*_range_iter were
# unhandled in LLVM's direct-TIR lowering; native/WASM round-trip via SimpleIR).
def sum_range(n):
    total = 0
    for i in range(n):
        total = total + i
    return total


def prod_range(n):
    p = 1
    for i in range(1, n):
        p = p * i
    return p


def range_to_list(n):
    out = []
    for x in range(n):
        out.append(x)
    return out


def range_index(n):
    r = range(n)
    return r[2]


def range_len(n):
    return len(range(n))


def builtins_len(xs):
    return len(xs)


def builtins_str(x):
    return str(x)


print(sum_range(10))
print(sum_range(0))
print(sum_range(1))
print(prod_range(6))
print(range_to_list(4))
print(range_index(5))
print(range_len(7))
print(builtins_len([1, 2, 3, 4]))
print(builtins_str(123))
# range at module scope (control: already worked)
ms = 0
for i in range(5):
    ms = ms + i
print(ms)
