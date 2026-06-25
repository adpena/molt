# The overflow peel can carry multiple pure loop phis. A multiply carrier and
# an add carrier in the same loop must deopt coherently, with the failed
# iteration re-executed from pre-iteration values.

product = 1
total = 0
step = 3
for i in range(1, 48):
    product = product * step
    total = total + product + i
print("mixed", product, total)
