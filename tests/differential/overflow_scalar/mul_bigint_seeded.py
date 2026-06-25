# A multiply accumulator that is already bigint-shaped must remain exact across
# additional loop-carried multiplies; CheckedMul must not force it back through
# a raw i64 carrier.

acc = 1 << 80
for n in range(2, 18):
    acc = acc * n
print("seeded", acc)

neg = -(1 << 80)
for n in range(2, 12):
    neg = neg * n
print("seeded_negative", neg)
