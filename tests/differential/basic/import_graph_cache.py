# MOLT_ENV: PYTHONPATH=src:tests/differential/basic
"""Purpose: differential coverage for import graph cache."""

from pkg_graph import state
import pkg_graph.node_a as node_a
import pkg_graph.node_b as node_b
import pkg_graph.node_a as node_a_again
import pkg_graph.node_b as node_b_again

_ = (node_a, node_b, node_a_again, node_b_again)

print("order", state.order)
