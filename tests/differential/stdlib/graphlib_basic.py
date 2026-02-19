"""Purpose: differential coverage for graphlib basic API surface."""

import graphlib

sorter = graphlib.TopologicalSorter({"a": {"b"}, "b": set()})
print(list(sorter.static_order()))

graph = {"a": {"b", "c"}, "b": {"c"}, "c": set()}
sorter = graphlib.TopologicalSorter(graph)
print(list(sorter.static_order()))

sorter = graphlib.TopologicalSorter()
sorter.add("a", "b")
sorter.add("b")
sorter.prepare()
print(sorter.get_ready())
sorter.done("b")
print(sorter.get_ready())
sorter.done("a")
print(sorter.is_active())

sorter = graphlib.TopologicalSorter({"a": {"b"}, "b": {"a"}})
try:
    sorter.prepare()
except graphlib.CycleError as exc:
    print(type(exc).__name__)
    print(exc.args[0])
    cycle = exc.args[1]
    print(cycle[0] == cycle[-1], len(cycle))
else:
    print("no cycle")

sorter = graphlib.TopologicalSorter({"a": set()})
sorter.prepare()
sorter.get_ready()
try:
    sorter.prepare()
except ValueError as exc:
    print(str(exc))

sorter = graphlib.TopologicalSorter()
sorter.add("a")
sorter.prepare()
try:
    sorter.done("missing")
except ValueError as exc:
    print(str(exc))
