# RC drop-insertion regression — generators consumed by reductions / nested
# generators / dict comprehension (adversarial-review P0 #2 required variants).
#
# Exercises the consumer-side ownership boundary for `sum(g())`, `min(g())`,
# a dict comprehension over a generator, and a generator that consumes ANOTHER
# generator (nested). Each consumer drives `IterNextUnboxed` to exhaustion, so
# the yielded-element edge-drop fix (see generator_consumer_drops.py) and the
# owned-generator-object accounting (`iter()` increfs; consumer drops once) must
# both hold.
#
# Byte-identical to CPython on NATIVE. On LLVM blocked by the SAME pre-existing,
# drop-independent generator-codegen segfault (bare generator creation crashes on
# LLVM with `MOLT_DROPINS_OFF=1`); the drop-pass placement itself is correct.
def squares(n):
    i = 0
    while i < n:
        yield i * i
        i = i + 1


def doubled(g):
    # Nested: a generator consuming another generator.
    for v in g:
        yield v + v


print(sum(squares(6)))
print(min(squares(6)))
print(sum(doubled(squares(5))))
print({k: k + 1 for k in squares(4)})
