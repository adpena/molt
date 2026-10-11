"""Matched ChainMap lookup/update workload; protocol-invalid rows stay separate."""

from collections import ChainMap

# Both maps retain external owners; cached numeric values avoid the old
# registry's borrowed-result lifetime defect in the baseline comparison.
first = {"front": 7}
second = {"back": 11}
chain = ChainMap(first, second)
iterations = 500_000
total = 0
hits = 0
index = 0
while index < iterations:
    total += chain["front"]
    total += chain["back"]
    total += chain.get("missing", 3)
    if "front" in chain:
        hits += 1
    chain["front"] = 7
    index += 1

assert total == 21 * iterations
assert hits == iterations
assert first["front"] == 7 and second["back"] == 11
print(total, hits, first["front"], second["back"])
