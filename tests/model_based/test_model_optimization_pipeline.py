"""Model-based tests derived from the Quint optimization pipeline specification.

Encodes invariants from ``formal/quint/molt_optimization_pipeline.qnt`` as
executable Python tests.  The Quint model verifies that the TIR optimization
pipeline (constFold, SCCP, DCE, edgeThread, CSE, guardHoist, joinCanon)
satisfies:

  - Bounded convergence (terminates within MAX_ROUNDS)
  - Monotonicity (node count never increases)
  - Soundness (no new node IDs introduced)
  - Idempotency (applying any pass twice = once)
  - Fixed-point stability
  - DCE correctness
  - CSE correctness
  - Non-negative uses
  - Unique node IDs

These tests are self-contained and do not require Quint to be installed.

Usage::

    uv run pytest tests/model_based/test_model_optimization_pipeline.py -v
"""

from __future__ import annotations

import copy
from dataclasses import dataclass
from enum import Enum, auto
from typing import Callable

import pytest


# ---------------------------------------------------------------------------
# Abstract IR model (mirrors the Quint specification)
# ---------------------------------------------------------------------------


class NodeKind(Enum):
    Const = auto()
    Arith = auto()
    Branch = auto()
    Phi = auto()
    Guard = auto()
    Dead = auto()
    Redundant = auto()


@dataclass
class IRNode:
    id: int
    kind: NodeKind
    uses: int
    is_constant: bool
    is_invariant: bool
    canonical: int

    def _key(self) -> tuple:
        return (
            self.id,
            self.kind,
            self.uses,
            self.is_constant,
            self.is_invariant,
            self.canonical,
        )

    def __eq__(self, other: object) -> bool:
        if not isinstance(other, IRNode):
            return NotImplemented
        return self._key() == other._key()

    def __hash__(self) -> int:
        return hash(self._key())


def _make_initial_nodes() -> list[IRNode]:
    """Initial program from the Quint model's init action."""
    return [
        IRNode(
            id=0,
            kind=NodeKind.Const,
            uses=2,
            is_constant=True,
            is_invariant=False,
            canonical=0,
        ),
        IRNode(
            id=1,
            kind=NodeKind.Arith,
            uses=1,
            is_constant=False,
            is_invariant=False,
            canonical=1,
        ),
        IRNode(
            id=2,
            kind=NodeKind.Branch,
            uses=0,
            is_constant=False,
            is_invariant=False,
            canonical=2,
        ),
        IRNode(
            id=3,
            kind=NodeKind.Phi,
            uses=1,
            is_constant=False,
            is_invariant=False,
            canonical=3,
        ),
        IRNode(
            id=4,
            kind=NodeKind.Guard,
            uses=1,
            is_constant=False,
            is_invariant=True,
            canonical=4,
        ),
        IRNode(
            id=5,
            kind=NodeKind.Arith,
            uses=0,
            is_constant=False,
            is_invariant=False,
            canonical=1,
        ),  # CSE duplicate of node 1
    ]


# ---------------------------------------------------------------------------
# Pass implementations (mirrors Quint pure functions)
# ---------------------------------------------------------------------------


def apply_const_fold(nodes: list[IRNode]) -> list[IRNode]:
    result = []
    for n in nodes:
        n2 = copy.copy(n)
        if n.kind == NodeKind.Arith and not n.is_constant:
            n2.kind = NodeKind.Const
            n2.is_constant = True
        result.append(n2)
    return result


def apply_sccp(nodes: list[IRNode]) -> list[IRNode]:
    result = []
    for n in nodes:
        n2 = copy.copy(n)
        if not n.is_constant and n.kind == NodeKind.Arith:
            n2.is_constant = True
        result.append(n2)
    return result


def apply_dce(nodes: list[IRNode]) -> list[IRNode]:
    return [
        copy.copy(n)
        for n in nodes
        if n.uses > 0 or n.kind in (NodeKind.Branch, NodeKind.Guard)
    ]


def apply_edge_thread(nodes: list[IRNode]) -> list[IRNode]:
    result = []
    for n in nodes:
        n2 = copy.copy(n)
        if n.kind == NodeKind.Branch and n.is_constant:
            n2.kind = NodeKind.Dead
            n2.uses = 0
        result.append(n2)
    return result


def apply_cse(nodes: list[IRNode]) -> list[IRNode]:
    result = []
    for n in nodes:
        n2 = copy.copy(n)
        if n.kind == NodeKind.Arith and n.canonical != n.id:
            n2.kind = NodeKind.Redundant
            n2.uses = 0
        result.append(n2)
    return result


def apply_guard_hoist(nodes: list[IRNode]) -> list[IRNode]:
    result = []
    for n in nodes:
        n2 = copy.copy(n)
        if n.kind == NodeKind.Guard and n.is_invariant:
            n2.is_constant = True
        result.append(n2)
    return result


def apply_join_canon(nodes: list[IRNode]) -> list[IRNode]:
    result = []
    for n in nodes:
        n2 = copy.copy(n)
        if n.kind == NodeKind.Phi:
            n2.canonical = n.id
        result.append(n2)
    return result


ALL_PASSES: list[tuple[str, Callable[[list[IRNode]], list[IRNode]]]] = [
    ("ConstFold", apply_const_fold),
    ("SCCP", apply_sccp),
    ("DCE", apply_dce),
    ("EdgeThread", apply_edge_thread),
    ("CSE", apply_cse),
    ("GuardHoist", apply_guard_hoist),
    ("JoinCanon", apply_join_canon),
]

MAX_ROUNDS = 6
MAX_NODES = 6


def _nodes_equal(a: list[IRNode], b: list[IRNode]) -> bool:
    return set(a) == set(b)


def _run_pipeline(ordering: list[str], max_rounds: int = MAX_ROUNDS) -> list[IRNode]:
    """Run the pipeline with a given pass ordering until fixed point."""
    pass_map = dict(ALL_PASSES)
    nodes = _make_initial_nodes()
    for _round in range(max_rounds):
        changed = False
        for name in ordering:
            fn = pass_map[name]
            new_nodes = fn(nodes)
            if not _nodes_equal(new_nodes, nodes):
                changed = True
            nodes = new_nodes
        if not changed:
            break
    return nodes


# ---------------------------------------------------------------------------
# Tests
# ---------------------------------------------------------------------------


class TestIdempotency:
    """I4 from the Quint model: applying any pass twice yields same result as once."""

    @pytest.mark.parametrize(
        "pass_name,pass_fn", ALL_PASSES, ids=[p[0] for p in ALL_PASSES]
    )
    def test_pass_is_idempotent(
        self,
        pass_name: str,
        pass_fn: Callable[[list[IRNode]], list[IRNode]],
    ) -> None:
        nodes = _make_initial_nodes()
        once = pass_fn(nodes)
        twice = pass_fn(once)
        assert _nodes_equal(once, twice), (
            f"Pass {pass_name} is not idempotent: "
            f"once produced {len(once)} nodes, twice produced {len(twice)} nodes"
        )

    @pytest.mark.parametrize(
        "pass_name,pass_fn", ALL_PASSES, ids=[p[0] for p in ALL_PASSES]
    )
    def test_idempotent_after_full_pipeline(
        self,
        pass_name: str,
        pass_fn: Callable[[list[IRNode]], list[IRNode]],
    ) -> None:
        """After running the full pipeline, each individual pass is still idempotent."""
        ordering = [name for name, _ in ALL_PASSES]
        nodes = _run_pipeline(ordering)
        once = pass_fn(nodes)
        twice = pass_fn(once)
        assert _nodes_equal(once, twice)


class TestFixedPointConvergence:
    """I1 from the Quint model: pipeline terminates within MAX_ROUNDS."""

    _ORDERINGS = [
        [name for name, _ in ALL_PASSES],
        list(reversed([name for name, _ in ALL_PASSES])),
        ["DCE", "ConstFold", "SCCP", "CSE", "EdgeThread", "GuardHoist", "JoinCanon"],
        ["CSE", "DCE", "ConstFold", "JoinCanon", "GuardHoist", "SCCP", "EdgeThread"],
    ]

    @pytest.mark.parametrize(
        "ordering", _ORDERINGS, ids=[f"order_{i}" for i in range(len(_ORDERINGS))]
    )
    def test_converges_within_budget(self, ordering: list[str]) -> None:
        pass_map = dict(ALL_PASSES)
        nodes = _make_initial_nodes()
        for round_num in range(MAX_ROUNDS + 1):
            changed = False
            for name in ordering:
                fn = pass_map[name]
                new_nodes = fn(nodes)
                if not _nodes_equal(new_nodes, nodes):
                    changed = True
                nodes = new_nodes
            if not changed:
                return  # converged
        pytest.fail(
            f"Pipeline did not converge within {MAX_ROUNDS} rounds "
            f"with ordering {ordering}"
        )

    @pytest.mark.parametrize(
        "ordering", _ORDERINGS, ids=[f"order_{i}" for i in range(len(_ORDERINGS))]
    )
    def test_fixed_point_is_stable(self, ordering: list[str]) -> None:
        """Once at fixed point, no pass changes the program (I5)."""
        nodes = _run_pipeline(ordering)
        for name, fn in ALL_PASSES:
            after = fn(nodes)
            assert _nodes_equal(after, nodes), (
                f"Pass {name} changed the program after fixed point"
            )


class TestMonotonicity:
    """I2: node count never increases across passes."""

    _ORDERINGS = [
        [name for name, _ in ALL_PASSES],
        list(reversed([name for name, _ in ALL_PASSES])),
        ["DCE", "CSE", "ConstFold", "SCCP", "EdgeThread", "GuardHoist", "JoinCanon"],
    ]

    @pytest.mark.parametrize(
        "ordering", _ORDERINGS, ids=[f"order_{i}" for i in range(len(_ORDERINGS))]
    )
    def test_node_count_never_increases(self, ordering: list[str]) -> None:
        pass_map = dict(ALL_PASSES)
        nodes = _make_initial_nodes()
        initial_count = len(nodes)
        assert initial_count <= MAX_NODES

        for _round in range(MAX_ROUNDS):
            for name in ordering:
                fn = pass_map[name]
                new_nodes = fn(nodes)
                assert len(new_nodes) <= len(nodes), (
                    f"Pass {name} increased node count from {len(nodes)} "
                    f"to {len(new_nodes)}"
                )
                nodes = new_nodes

    @pytest.mark.parametrize(
        "ordering", _ORDERINGS, ids=[f"order_{i}" for i in range(len(_ORDERINGS))]
    )
    def test_size_bounded_by_max_nodes(self, ordering: list[str]) -> None:
        nodes = _run_pipeline(ordering)
        assert len(nodes) <= MAX_NODES


class TestSoundness:
    """I3: no pass introduces new node IDs not in the original program."""

    def test_no_new_ids_after_pipeline(self) -> None:
        original_ids = {n.id for n in _make_initial_nodes()}
        ordering = [name for name, _ in ALL_PASSES]
        nodes = _run_pipeline(ordering)
        final_ids = {n.id for n in nodes}
        assert final_ids.issubset(original_ids), (
            f"New IDs introduced: {final_ids - original_ids}"
        )

    @pytest.mark.parametrize(
        "pass_name,pass_fn", ALL_PASSES, ids=[p[0] for p in ALL_PASSES]
    )
    def test_no_new_ids_per_pass(
        self,
        pass_name: str,
        pass_fn: Callable[[list[IRNode]], list[IRNode]],
    ) -> None:
        nodes = _make_initial_nodes()
        original_ids = {n.id for n in nodes}
        result = pass_fn(nodes)
        result_ids = {n.id for n in result}
        assert result_ids.issubset(original_ids), (
            f"Pass {pass_name} introduced new IDs: {result_ids - original_ids}"
        )


class TestDCECorrectness:
    """I6: after DCE, no dead nodes survive except branches/guards."""

    def test_no_dead_nodes_after_dce(self) -> None:
        nodes = _make_initial_nodes()
        after_dce = apply_dce(nodes)
        for n in after_dce:
            assert n.uses > 0 or n.kind in (NodeKind.Branch, NodeKind.Guard), (
                f"Dead node survived DCE: id={n.id}, kind={n.kind}, uses={n.uses}"
            )

    def test_dce_after_full_pipeline(self) -> None:
        ordering = [name for name, _ in ALL_PASSES]
        nodes = _run_pipeline(ordering)
        after_dce = apply_dce(nodes)
        for n in after_dce:
            assert n.uses > 0 or n.kind in (NodeKind.Branch, NodeKind.Guard)


class TestCSECorrectness:
    """I7: after CSE, no two live nodes share the same canonical representative."""

    def test_no_duplicate_canonicals_after_cse(self) -> None:
        nodes = _make_initial_nodes()
        after_cse = apply_cse(nodes)
        live = [n for n in after_cse if n.uses > 0]
        for i, n1 in enumerate(live):
            for n2 in live[i + 1 :]:
                if (
                    n1.kind == NodeKind.Arith
                    and n2.kind == NodeKind.Arith
                    and n1.canonical == n2.canonical
                ):
                    pytest.fail(
                        f"Duplicate canonical {n1.canonical} for "
                        f"live nodes {n1.id} and {n2.id}"
                    )

    def test_cse_removes_redundant_duplicate(self) -> None:
        """Node 5 is a CSE duplicate of node 1 (canonical=1, id=5)."""
        nodes = _make_initial_nodes()
        after_cse = apply_cse(nodes)
        node5 = next(n for n in after_cse if n.id == 5)
        assert node5.kind == NodeKind.Redundant
        assert node5.uses == 0


class TestConstFoldSoundness:
    """I8: only Arith nodes become Const; structural nodes are never folded."""

    def test_only_arith_nodes_folded(self) -> None:
        original = {n.id: n for n in _make_initial_nodes()}
        folded = apply_const_fold(list(original.values()))
        for n in folded:
            orig = original[n.id]
            if n.is_constant and not orig.is_constant:
                assert orig.kind in (NodeKind.Arith, NodeKind.Guard), (
                    f"Node {n.id} (kind={orig.kind}) was incorrectly folded to constant"
                )

    def test_branches_never_folded(self) -> None:
        nodes = _make_initial_nodes()
        folded = apply_const_fold(nodes)
        for n in folded:
            if n.id == 2:  # Branch node
                # Branch should not become Const via constFold
                assert n.kind == NodeKind.Branch


class TestNonNegativeUses:
    """I9: no node has negative use count at any point."""

    @pytest.mark.parametrize(
        "pass_name,pass_fn", ALL_PASSES, ids=[p[0] for p in ALL_PASSES]
    )
    def test_uses_non_negative_per_pass(
        self,
        pass_name: str,
        pass_fn: Callable[[list[IRNode]], list[IRNode]],
    ) -> None:
        nodes = _make_initial_nodes()
        result = pass_fn(nodes)
        for n in result:
            assert n.uses >= 0, (
                f"Pass {pass_name} produced negative uses for node {n.id}: {n.uses}"
            )


class TestUniqueNodeIds:
    """I10: node IDs are unique within the program after any pass."""

    @pytest.mark.parametrize(
        "pass_name,pass_fn", ALL_PASSES, ids=[p[0] for p in ALL_PASSES]
    )
    def test_unique_ids_per_pass(
        self,
        pass_name: str,
        pass_fn: Callable[[list[IRNode]], list[IRNode]],
    ) -> None:
        nodes = _make_initial_nodes()
        result = pass_fn(nodes)
        ids = [n.id for n in result]
        assert len(ids) == len(set(ids)), (
            f"Pass {pass_name} produced duplicate node IDs: {ids}"
        )


class TestPassOrderingDeterminism:
    """Pass ordering produces deterministic results (key model property)."""

    def test_different_orderings_all_reach_fixed_point(self) -> None:
        """Every pass ordering reaches a stable fixed point."""
        orderings = [
            [name for name, _ in ALL_PASSES],
            list(reversed([name for name, _ in ALL_PASSES])),
            [
                "DCE",
                "ConstFold",
                "SCCP",
                "CSE",
                "EdgeThread",
                "GuardHoist",
                "JoinCanon",
            ],
            [
                "JoinCanon",
                "GuardHoist",
                "EdgeThread",
                "CSE",
                "SCCP",
                "ConstFold",
                "DCE",
            ],
            [
                "CSE",
                "DCE",
                "ConstFold",
                "JoinCanon",
                "GuardHoist",
                "SCCP",
                "EdgeThread",
            ],
        ]
        for ordering in orderings:
            nodes = _run_pipeline(ordering)
            # Verify fixed-point stability: no pass changes the result
            for name, fn in ALL_PASSES:
                after = fn(nodes)
                assert _nodes_equal(after, nodes), (
                    f"Pass {name} changes the program after fixed point "
                    f"with ordering {ordering}"
                )

    def test_same_ordering_is_deterministic(self) -> None:
        """Running the same ordering twice produces identical results."""
        ordering = [name for name, _ in ALL_PASSES]
        nodes1 = _run_pipeline(ordering)
        nodes2 = _run_pipeline(ordering)
        assert _nodes_equal(nodes1, nodes2)

    def test_all_orderings_preserve_live_node_ids(self) -> None:
        """All orderings produce the same set of surviving node IDs after DCE."""
        orderings = [
            [name for name, _ in ALL_PASSES],
            list(reversed([name for name, _ in ALL_PASSES])),
            [
                "DCE",
                "ConstFold",
                "SCCP",
                "CSE",
                "EdgeThread",
                "GuardHoist",
                "JoinCanon",
            ],
        ]
        live_id_sets = []
        for ordering in orderings:
            nodes = _run_pipeline(ordering)
            live_ids = {
                n.id
                for n in nodes
                if n.uses > 0 or n.kind in (NodeKind.Branch, NodeKind.Guard)
            }
            live_id_sets.append(live_ids)

        for i, ids in enumerate(live_id_sets[1:], 1):
            assert ids == live_id_sets[0], (
                f"Ordering {i} has different live node IDs: {ids} vs {live_id_sets[0]}"
            )
