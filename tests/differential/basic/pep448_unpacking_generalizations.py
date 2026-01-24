"""Purpose: differential coverage for PEP 448 unpacking generalizations."""


def packer(*args, **kwargs):
    return args, kwargs


nums = [1, 2]
more = (3, 4)
base = {"x": 1}
override = {"x": 9}
extra = {"y": 2}

print([0, *nums, *more, 5])
print({**base, "x": 0, **extra})
print({**base, **override})
print(packer(0, *nums, *more, z=3, **extra))
