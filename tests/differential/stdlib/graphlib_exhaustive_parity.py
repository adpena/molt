"""Purpose: exhaustive CPython 3.12 parity coverage for graphlib."""

import graphlib


def expect_raises(exc_type, msg_substr, fn, *args, **kwargs):
    try:
        fn(*args, **kwargs)
    except exc_type as exc:
        text = str(exc)
        if msg_substr is not None:
            assert msg_substr in text, (msg_substr, text)
        return exc
    raise AssertionError(f"expected {exc_type.__name__}")


def static_order_groups(ts):
    ts.prepare()
    groups = []
    while ts.is_active():
        nodes = ts.get_ready()
        for node in nodes:
            ts.done(node)
        groups.append(tuple(sorted(nodes)))
    return groups


def assert_graph_groups(graph, expected_groups):
    ts = graphlib.TopologicalSorter(graph)
    assert static_order_groups(ts) == list(expected_groups)

    ts = graphlib.TopologicalSorter(graph)
    it = iter(ts.static_order())
    for group in expected_groups:
        got = {next(it) for _ in group}
        assert got == set(group), (got, set(group))


def assert_cycle(graph, cycle):
    ts = graphlib.TopologicalSorter()
    for node, predecessors in graph.items():
        ts.add(node, *predecessors)
    exc = expect_raises(graphlib.CycleError, "nodes are in a cycle", ts.prepare)
    _, seq = exc.args
    joined = " ".join(map(str, seq * 2))
    assert " ".join(map(str, cycle)) in joined


# CPython 3.12 Lib/test/test_graphlib.py::test_simple_cases
assert_graph_groups(
    {2: {11}, 9: {11, 8}, 10: {11, 3}, 11: {7, 5}, 8: {7, 3}},
    [(3, 5, 7), (8, 11), (2, 9, 10)],
)
assert_graph_groups({1: {}}, [(1,)])
assert_graph_groups({x: {x + 1} for x in range(10)}, [(x,) for x in range(10, -1, -1)])
assert_graph_groups(
    {2: {3}, 3: {4}, 4: {5}, 5: {1}, 11: {12}, 12: {13}, 13: {14}, 14: {15}},
    [(1, 15), (5, 14), (4, 13), (3, 12), (2, 11)],
)
assert_graph_groups(
    {
        0: [1, 2],
        1: [3],
        2: [5, 6],
        3: [4],
        4: [9],
        5: [3],
        6: [7],
        7: [8],
        8: [4],
        9: [],
    },
    [(9,), (4,), (3, 8), (1, 5, 7), (6,), (2,), (0,)],
)
assert_graph_groups({0: [1, 2], 1: [], 2: [3], 3: []}, [(1, 3), (2,), (0,)])
assert_graph_groups(
    {0: [1, 2], 1: [], 2: [3], 3: [], 4: [5], 5: [6], 6: []},
    [(1, 3, 6), (2, 5), (0, 4)],
)
assert_graph_groups({1: {2}, 3: {4}, 5: {6}}, [(2, 4, 6), (1, 3, 5)])
assert_graph_groups({1: set(), 3: set(), 5: set()}, [(1, 3, 5)])
assert_graph_groups({1: {2}, 3: {4}, 0: [2, 4, 4, 4, 4, 4]}, [(2, 4), (0, 1, 3)])
assert_graph_groups({}, [])

# CPython 3.12 Lib/test/test_graphlib.py::test_the_node_multiple_times
ts = graphlib.TopologicalSorter()
ts.add(1, 2)
ts.add(1, 2)
ts.add(1, 2)
assert [*ts.static_order()] == [2, 1]

# CPython 3.12 Lib/test/test_graphlib.py::test_graph_with_iterables
dependson = (2 * x + 1 for x in range(5))
ts = graphlib.TopologicalSorter({0: dependson})
assert list(ts.static_order()) == [1, 3, 5, 7, 9, 0]

# CPython 3.12 Lib/test/test_graphlib.py::test_add_dependencies_for_same_node_incrementally
ts = graphlib.TopologicalSorter()
ts.add(1, 2)
ts.add(1, 3)
ts.add(1, 4)
ts.add(1, 5)
ts2 = graphlib.TopologicalSorter({1: {2, 3, 4, 5}})
assert [*ts.static_order()] == [*ts2.static_order()]

# CPython 3.12 Lib/test/test_graphlib.py::test_cycle
assert_cycle({1: {1}}, [1, 1])
assert_cycle({1: {2}, 2: {1}}, [1, 2, 1])
assert_cycle({1: {2}, 2: {3}, 3: {1}}, [1, 3, 2, 1])
assert_cycle({1: {2}, 2: {3}, 3: {1}, 5: {4}, 4: {6}}, [1, 3, 2, 1])
assert_cycle({1: {2}, 2: {1}, 3: {4}, 4: {5}, 6: {7}, 7: {6}}, [1, 2, 1])
assert_cycle({1: {2}, 2: {3}, 3: {2, 4}, 4: {5}}, [3, 2])

# CPython 3.12 Lib/test/test_graphlib.py::test_calls_before_prepare
for method in (
    lambda s: s.get_ready(),
    lambda s: s.done(3),
    lambda s: s.is_active(),
    lambda s: bool(s),
):
    ts = graphlib.TopologicalSorter()
    expect_raises(ValueError, "prepare() must be called first", method, ts)

ts = graphlib.TopologicalSorter()
ts.prepare()
expect_raises(ValueError, "cannot prepare() more than once", ts.prepare)

# CPython 3.12 Lib/test/test_graphlib.py::test_prepare_multiple_times + post-static_order behavior
ts = graphlib.TopologicalSorter({1: {2}})
assert list(ts.static_order()) == [2, 1]
expect_raises(ValueError, "cannot prepare() more than once", ts.prepare)

# CPython 3.12 static_order laziness nuance: error is raised on first iteration.
ts = graphlib.TopologicalSorter({1: {2}})
ts.prepare()
it = ts.static_order()
assert type(it).__name__ == "generator"
expect_raises(ValueError, "cannot prepare() more than once", lambda: next(it))

# CPython 3.12 Lib/test/test_graphlib.py::test_invalid_nodes_in_done
ts = graphlib.TopologicalSorter()
ts.add(1, 2, 3, 4)
ts.add(2, 3, 4)
ts.prepare()
ts.get_ready()
expect_raises(ValueError, "node 2 was not passed out", ts.done, 2)
expect_raises(ValueError, "node 24 was not added using add()", ts.done, 24)

# CPython 3.12 Lib/test/test_graphlib.py::test_done
ts = graphlib.TopologicalSorter()
ts.add(1, 2, 3, 4)
ts.add(2, 3)
ts.prepare()
assert ts.get_ready() == (3, 4)
assert ts.get_ready() == ()
ts.done(3)
assert ts.get_ready() == (2,)
assert ts.get_ready() == ()
ts.done(4)
ts.done(2)
assert ts.get_ready() == (1,)
assert ts.get_ready() == ()
ts.done(1)
assert ts.get_ready() == ()
assert ts.is_active() is False

# CPython 3.12 done(*nodes) accepts empty node lists as a no-op.
ts.done()
assert ts.get_ready() == ()
assert ts.is_active() is False

# CPython 3.12 duplicate done-node edge (message parity)
ts = graphlib.TopologicalSorter({1: {2}})
ts.prepare()
ready = ts.get_ready()
assert ready == (2,)
expect_raises(ValueError, "node 2 was already marked done", ts.done, 2, 2)

# CPython 3.12 Lib/test/test_graphlib.py::test_is_active
ts = graphlib.TopologicalSorter()
ts.add(1, 2)
ts.prepare()
assert ts.is_active() is True
assert ts.get_ready() == (2,)
assert ts.is_active() is True
ts.done(2)
assert ts.is_active() is True
assert ts.get_ready() == (1,)
assert ts.is_active() is True
ts.done(1)
assert ts.is_active() is False

# CPython 3.12 Lib/test/test_graphlib.py::test_not_hashable_nodes
ts = graphlib.TopologicalSorter()
expect_raises(TypeError, "unhashable type", ts.add, {}, 1)
expect_raises(TypeError, "unhashable type", ts.add, 1, {})
expect_raises(TypeError, "unhashable type", ts.add, {}, {})


# CPython 3.12 Lib/test/test_graphlib.py::test_order_of_insertion_does_not_matter_between_groups
def groups(ts):
    ts.prepare()
    out = []
    while ts.is_active():
        nodes = ts.get_ready()
        ts.done(*nodes)
        out.append(set(nodes))
    return out


ts = graphlib.TopologicalSorter()
ts.add(3, 2, 1)
ts.add(1, 0)
ts.add(4, 5)
ts.add(6, 7)
ts.add(4, 7)

ts2 = graphlib.TopologicalSorter()
ts2.add(1, 0)
ts2.add(3, 2, 1)
ts2.add(4, 7)
ts2.add(6, 7)
ts2.add(4, 5)
assert groups(ts) == groups(ts2)

# CPython 3.12 GenericAlias behavior for TopologicalSorter[T]
alias = graphlib.TopologicalSorter[int]
assert alias.__origin__ is graphlib.TopologicalSorter

# CPython 3.12 cycle handling after prepare() failure: ready nodes remain consumable.
ts = graphlib.TopologicalSorter({1: {2}, 2: {1}, 3: set()})
expect_raises(graphlib.CycleError, "nodes are in a cycle", ts.prepare)
assert ts.get_ready() == (3,)
ts.done(3)
assert ts.is_active() is False
assert ts.get_ready() == ()

# CPython 3.12 module namespace shape.
assert graphlib.__all__ == ["TopologicalSorter", "CycleError"]
assert hasattr(graphlib, "_NodeInfo")
assert graphlib._NODE_OUT == -1
assert graphlib._NODE_DONE == -2

print("graphlib_exhaustive_parity_ok")
