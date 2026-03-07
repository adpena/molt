from functools import reduce

# Test reduce with various operations
nums = [1, 2, 3, 4, 5]
print(reduce(lambda a, b: a + b, nums))
print(reduce(lambda a, b: a * b, nums))
print(reduce(lambda a, b: a + b, nums, 100))

# partial
from functools import partial

def power(base, exp):
    return base ** exp

square = partial(power, exp=2)
print(square(5))
print(square(10))
