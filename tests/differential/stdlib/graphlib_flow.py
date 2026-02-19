"""Purpose: exercise graphlib TopologicalSorter flow + errors."""

import graphlib

sorter = graphlib.TopologicalSorter()
sorter.add("a", "b")
sorter.add("b")
sorter.prepare()
print("ready-1", sorter.get_ready())
sorter.done("b")
print("ready-2", sorter.get_ready())
print("active-1", sorter.is_active())
sorter.done("a")
print("active-2", sorter.is_active())

order_sorter = graphlib.TopologicalSorter({"a": {"b"}, "b": {"c"}, "c": set()})
print("static-order", list(order_sorter.static_order()))

try:
    graphlib.TopologicalSorter().get_ready()
except Exception as exc:  # noqa: BLE001
    print("get_ready_before_prepare", type(exc).__name__, str(exc))

try:
    graphlib.TopologicalSorter().done("a")
except Exception as exc:  # noqa: BLE001
    print("done_before_prepare", type(exc).__name__, str(exc))

try:
    prepared = graphlib.TopologicalSorter({"a": []})
    prepared.prepare()
    prepared.add("b")
except Exception as exc:  # noqa: BLE001
    print("add_after_prepare", type(exc).__name__, str(exc))

try:
    cyclic = graphlib.TopologicalSorter({"a": {"b"}, "b": {"a"}})
    cyclic.prepare()
except Exception as exc:  # noqa: BLE001
    print("cycle_prepare", type(exc).__name__, exc.args)
