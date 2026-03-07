from graphlib import TopologicalSorter

# Simple DAG
ts = TopologicalSorter()
ts.add("D", "B", "C")
ts.add("C", "A")
ts.add("B", "A")
order = list(ts.static_order())
print(order)

# Another graph
ts2 = TopologicalSorter({"B": {"A"}, "C": {"A", "B"}, "D": {"C"}})
print(list(ts2.static_order()))

# Empty graph
ts3 = TopologicalSorter()
print(list(ts3.static_order()))
