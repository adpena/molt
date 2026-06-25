# Guards the 64-bit-EXACT (not 47-bit) CheckedMul overflow flag. A 47-bit flag would
# deopt early (a perf bug); a wrong 64-bit flag would silently WRAP a product that
# fits i64 but exceeds the 2^46 inline window, or fail to promote one exceeding 2^63.
# Products are observed (printed), so any wrap diverges from CPython.

total = 0
i = 1
while i < 200_000_000:
    total = total + i * i  # i*i crosses 2^46 (~8.4M) then 2^63
    i = i * 3 + 1
print("sum_isq", total, "last_i", i)

# i*j straddling the inline-47 window from both sides.
vals = []
for i in range(1, 6):
    base = 1 << (44 + i)  # 2^45 .. 2^49, around the 2^46 boundary
    vals.append(base * 3)  # product crosses i64? no -> must stay exact i64-or-bigint
print("inline47_straddle", vals)

# product accumulator stepping just past 2^63 (the i64 overflow boundary).
p = 1
crossed = None
for k in range(1, 66):
    p = p * 2
    if p > (1 << 63) and crossed is None:
        crossed = k
print("two_pow", p, "crossed_2^63_at_k", crossed)
