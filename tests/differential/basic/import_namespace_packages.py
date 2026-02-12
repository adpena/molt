# MOLT_ENV: PYTHONPATH=src:tests/differential/basic/ns_part_a:tests/differential/basic/ns_part_b
"""Purpose: differential coverage for import namespace packages."""

import ns_pkg.mod_a
import ns_pkg.mod_b

print("A", ns_pkg.mod_a.VALUE_A)
print("B", ns_pkg.mod_b.VALUE_B)
print("pkg", ns_pkg.__name__)
