"""Purpose: differential coverage for difflib basic functions."""

import difflib


# --- SequenceMatcher.ratio ---
s = difflib.SequenceMatcher(None, "abcde", "abcde")
print("identical ratio:", s.ratio())

s2 = difflib.SequenceMatcher(None, "abcde", "fghij")
print("no match ratio:", s2.ratio())

s3 = difflib.SequenceMatcher(None, "abcde", "abdce")
r = s3.ratio()
print("partial ratio:", r)
print("partial ratio > 0.5:", r > 0.5)

# --- get_close_matches ---
matches = difflib.get_close_matches("appel", ["ape", "apple", "peach", "puppy"])
print("close matches:", matches)

matches2 = difflib.get_close_matches("xyz", ["abc", "def", "ghi"])
print("no close matches:", matches2)

# --- unified_diff ---
a = ["one\n", "two\n", "three\n"]
b = ["one\n", "tree\n", "three\n"]
diff = list(difflib.unified_diff(a, b, fromfile="a.txt", tofile="b.txt"))
print("unified_diff lines:", len(diff))
for line in diff:
    print(line, end="")
if diff:
    print()

# --- ndiff ---
a2 = ["one\n", "two\n", "three\n"]
b2 = ["one\n", "two\n", "four\n"]
nd = list(difflib.ndiff(a2, b2))
print("ndiff lines:", len(nd))
for line in nd:
    print(line, end="")
if nd:
    print()
