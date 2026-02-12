"""Purpose: differential coverage for random basic."""

import random


def show(label, value):
    print(label, value)


show("randrange_single", random.randrange(1))
items = [1]
random.shuffle(items)
show("shuffle_single", items)
try:
    random.randrange(0)
except Exception as exc:
    print("randrange_empty", type(exc).__name__)
