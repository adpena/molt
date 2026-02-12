"""Purpose: differential coverage for graphlib basic API surface."""

import graphlib

sorter = graphlib.TopologicalSorter({"a": {"b"}, "b": set()})
print(list(sorter.static_order()))
