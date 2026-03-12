# Generated from Lean theorem: index_adjust_semantic
# Source: formal/lean/MoltTIR/Backend/LuauCorrect.lean
# Property: adjustIndex(intLit(n)) evaluates to n+1 (semantic evaluation).
# Validates that 0-based to 1-based index conversion works at evaluation time.

# For Python (0-based), element at index i in a list of size n
# corresponds to 1-based index i+1. Test that list[i] works correctly.
lst = list(range(1, 11))  # [1, 2, ..., 10]

# Verify each 0-based index accesses the correct element
for i in range(10):
    # Element at 0-based index i should be i+1 (by construction)
    print(lst[i] == i + 1)

# Verify slice semantics preserve index arithmetic
print(lst[0:3])
print(lst[3:7])
print(lst[7:10])

# Verify that index i gives the (i+1)-th element (1-based)
for i in range(len(lst)):
    print(f"0-based index {i} -> value {lst[i]} (1-based position {i + 1})")
