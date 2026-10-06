# List comprehension over a string. It stays inside every backend's exact
# subset, including Luau, which has no arbitrary-precision Python int.
letters = [ch for ch in "ab"]
print(letters)
