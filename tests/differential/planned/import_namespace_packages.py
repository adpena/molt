# MOLT_ENV: PYTHONPATH=src:tests/differential/planned/ns_part_a:tests/differential/planned/ns_part_b
# TODO(import-system, owner:stdlib, milestone:TC3, priority:P1, status:planned): support namespace packages in project-root builds.
import ns_pkg.mod_a
import ns_pkg.mod_b

print("A", ns_pkg.mod_a.VALUE_A)
print("B", ns_pkg.mod_b.VALUE_B)
print("pkg", ns_pkg.__name__)
