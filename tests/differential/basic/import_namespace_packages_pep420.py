# MOLT_ENV: PYTHONPATH=src:tests/differential/basic/ns_part_a:tests/differential/basic/ns_part_b
"""Purpose: differential coverage for PEP 420 namespace package __path__."""

import ns_pkg
import ns_pkg.mod_a
import ns_pkg.mod_b


print("pkg", ns_pkg.__name__)
print("path", sorted(list(ns_pkg.__path__)))
print("A", ns_pkg.mod_a.VALUE_A)
print("B", ns_pkg.mod_b.VALUE_B)
