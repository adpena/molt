"""Built-in bound-method existence checks preserve CPython semantics."""

mapping = {"value": 1}
print("items-present", hasattr(mapping, "items"))
print("missing-absent", hasattr(mapping, "definitely_missing"))

# Exercise the owned-result discard path repeatedly. The exact reference-count
# invariant is asserted by the colocated runtime unit test; this differential
# pins observable CPython behavior on the supported built-in surface.
for _ in range(1000):
    if not hasattr(mapping, "items"):
        raise AssertionError("dict.items disappeared")
print("repeated-present", True)
