"""UTF-8 side caches across string free, address reuse and in-place growth.

Indexing, slicing, `len`, `find` and `count` on long non-ASCII strings build
per-string caches keyed by the string's address. A freed string must leave no
entry a later string at the same address could read, and a string grown in
place must not keep the entry of its old contents. Results match CPython.
"""


def long_text(seed):
    unit = "aé漢" + chr(0x1F600 + seed % 16) + "z" * (seed % 5)
    return unit * (300 + seed)


def probe(text):
    n = len(text)
    return (
        n,
        text[n // 2],
        text[n // 3 : n // 3 + 5],
        text.find("z"),
        text.count("é"),
        text.count("漢", n // 4, n // 2),
        text[-7:],
    )


# Many cached strings created and freed in turn, so addresses are reused.
results = []
for seed in range(40):
    text = long_text(seed)
    results.append(probe(text))
    del text
print("reuse", len(results), results[0], results[17], results[-1])

# The same contents rebuilt after a free give the same answers.
first = probe(long_text(3))
again = probe(long_text(3))
print("rebuild", first == again, first)

# ASCII strings with count caches, freed and replaced by non-ASCII ones.
for seed in range(20):
    plain = "abcab" * (200 + seed)
    print("ascii", seed, plain.count("ab"), plain.count("ca", 7, 400), end=" ")
    del plain
    wide = "é" * (500 + seed) + "ab"
    print(wide.count("ab"), len(wide), wide[500 + seed])
    del wide


# A cached string grown in place by += keeps no stale entry.
def grow():
    text = "é漢" * 400
    seen = [len(text), text[401], text.count("漢")]
    for step in range(30):
        text += "x" * step + "é"
        seen.append((len(text), text[len(text) // 2], text.count("é"), text.find("x")))
    return seen


print("grow", grow())

# Strings that were never cached are freed as before.
total = 0
for i in range(5000):
    s = "k" + str(i) + "é"
    total += len(s)
print("plain", total)
