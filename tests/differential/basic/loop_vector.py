"""Purpose: differential coverage for loop vector."""

nums = [1, 2, 3, 4]
total = 0
for x in nums:
    total = total + x
print(total)

acc = 0
for x in (5, 6):
    acc = acc + x
print(acc)

floats = [1.0, 2.5, 3.5]
fsum = 0
for x in floats:
    fsum = fsum + x
print(fsum)

prod = 1
for x in nums:
    prod *= x
print(prod)

prod_idx = 1
for i in range(len(nums)):
    prod_idx = prod_idx * nums[i]
print(prod_idx)

vals = [3, -1, 2, 7]
min_val = vals[0]
for x in vals:
    if x < min_val:
        min_val = x
print(min_val)

max_val = vals[0]
for x in vals:
    if max_val < x:
        max_val = x
print(max_val)

min_idx = vals[0]
for i in range(len(vals)):
    if vals[i] < min_idx:
        min_idx = vals[i]
print(min_idx)

max_idx = vals[0]
for i in range(len(vals)):
    if max_idx < vals[i]:
        max_idx = vals[i]
print(max_idx)

sum_offset = 0
for i in range(1, len(nums)):
    sum_offset += nums[i]
print(sum_offset)

sum_init = 10
for x in nums:
    sum_init += x
print(sum_init)

prod_offset = 1
for i in range(1, len(nums)):
    prod_offset *= nums[i]
print(prod_offset)

min_offset = vals[0]
for i in range(1, len(vals)):
    if vals[i] < min_offset:
        min_offset = vals[i]
print(min_offset)

max_offset = vals[0]
for i in range(1, len(vals)):
    if max_offset < vals[i]:
        max_offset = vals[i]
print(max_offset)

offset = 1
sum_dyn = 0
for i in range(offset, len(nums)):
    sum_dyn += nums[i]
print(sum_dyn)

sum_dyn_init = 5
for i in range(offset, len(nums)):
    sum_dyn_init = sum_dyn_init + nums[i]
print(sum_dyn_init)
