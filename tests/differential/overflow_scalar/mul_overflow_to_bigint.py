# CheckedMul correctness gate: a loop-carried int multiply accumulator MUST promote
# to an exact Python bigint on i64 overflow, never silently wrap at 2^63. If the
# CheckedMul fast/slow deopt is wrong, the printed values diverge from CPython.
# Passes on current (boxed) molt AND must keep passing after the CheckedMul peel.


def factorial(n):
    s = 1
    for i in range(1, n + 1):
        s = s * i
    return s


for n in (10, 20, 21, 25, 40, 100):
    print(n, factorial(n))  # 21! > 2^63 -> exact bigint required

acc = 1
for _ in range(80):
    acc = acc * 3
print("pow3_80", acc)  # 3^80, far past i64

# Accumulator that crosses 2^63 mid-loop with the value observed every step.
p = 1
last = 0
for k in range(1, 64):
    p = p * 2
    last = last + p
print("pow2_partial_sums", p, last)
