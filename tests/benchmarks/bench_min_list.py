nums = list(range(1_000_000, 0, -1))
acc = nums[0]
for x in nums:
    if x < acc:
        acc = x
print(acc)
