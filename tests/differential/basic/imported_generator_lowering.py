# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: imported generator functions lower through module bindings correctly."""

import os
import sys

sys.path.insert(0, os.path.dirname(__file__))

import imported_generator_lowering_mod as mod
from imported_generator_lowering_mod import consume, ladder, nested


def consume_factory(factory, limit):
    total = 0
    for value in factory(limit):
        total += value
    return total


print("from-list", list(ladder(4)))
print("from-consume", consume(ladder(5)))
print("alias-list", list(mod.ladder(3)))
print("alias-nested", list(mod.nested(3)))
print("factory", consume_factory(ladder, 6), consume_factory(mod.ladder, 6))
