"""Model-based tests derived from the Quint build and runtime determinism specs.

Encodes invariants from:

  - ``formal/quint/molt_build_determinism.qnt`` — build-order independence
  - ``formal/quint/molt_runtime_determinism.qnt`` — runtime task determinism

Build determinism invariants:
  - Dependencies respected (no module built before its deps)
  - Cache correctness (cached entries match deterministic compile output)
  - Hash seed pinning (PYTHONHASHSEED always 0)
  - Final determinism (same input + same seed = same output)
  - Artifact correctness (every artifact correctly compiled)
  - No duplicate artifacts

Runtime determinism invariants:
  - Seeds pinned (time, random, hash seeds never drift from 0)
  - Results deterministic (same task always produces same result)
  - No phantom completions
  - All results present when all tasks complete
  - Task conservation (pending + running + completed = all tasks)
  - No duplicate results

These tests are self-contained and do not require Quint to be installed.

Usage::

    uv run pytest tests/model_based/test_model_determinism.py -v
"""

from __future__ import annotations

import itertools
from dataclasses import dataclass

import pytest


# ---------------------------------------------------------------------------
# Build determinism model (mirrors Quint specification)
# ---------------------------------------------------------------------------

N_MODULES = 4
MODULES = set(range(N_MODULES))


def deps(m: int) -> set[int]:
    """Dependency graph from the Quint model."""
    if m == 0:
        return set()
    elif m == 1:
        return {0}
    elif m == 2:
        return {0}
    elif m == 3:
        return {1, 2}
    return set()


def compile_digest(m: int) -> int:
    """Content-addressed compilation: same module -> same digest.

    Models _cache_key() in cli.py: SHA256(IR_payload | target | fingerprints).
    """
    return (m * 1103515245 + 12345) % 2147483647


def link_digest(modules: set[int]) -> int:
    """Canonical link digest: order-independent combination of module digests."""
    result = 0
    for i in range(N_MODULES):
        if i in modules:
            result += compile_digest(i)
    return result


def deps_ready(m: int, built: set[int]) -> bool:
    return deps(m).issubset(built)


def ready_set(remaining: set[int], built: set[int]) -> set[int]:
    return {m for m in remaining if deps_ready(m, built)}


@dataclass
class BuildState:
    todo: set[int]
    done: set[int]
    cache: dict[int, int | None]  # module -> digest or None
    artifacts: dict[int, int]  # module -> digest
    hash_seed: int


def _make_build_state() -> BuildState:
    return BuildState(
        todo=set(MODULES),
        done=set(),
        cache={m: None for m in MODULES},
        artifacts={},
        hash_seed=0,
    )


def build_one(state: BuildState, m: int) -> BuildState:
    """Build one module whose dependencies are satisfied."""
    assert m in state.todo
    assert deps_ready(m, state.done)

    cached = state.cache[m]
    digest = cached if cached is not None else compile_digest(m)

    new_state = BuildState(
        todo=state.todo - {m},
        done=state.done | {m},
        cache={**state.cache, m: digest} if cached is None else dict(state.cache),
        artifacts={**state.artifacts, m: digest},
        hash_seed=state.hash_seed,  # always pinned
    )
    return new_state


def build_all(order: list[int]) -> BuildState:
    """Build all modules in the given order."""
    state = _make_build_state()
    for m in order:
        state = build_one(state, m)
    return state


# ---------------------------------------------------------------------------
# Runtime determinism model (mirrors Quint specification)
# ---------------------------------------------------------------------------

N_TASKS = 4
TASKS = set(range(N_TASKS))


def task_result(t: int) -> int:
    """Deterministic result based on task ID (from Quint model)."""
    return t * 7 + 3


@dataclass
class RuntimeState:
    pending: set[int]
    running: set[int]
    completed: set[int]
    results: dict[int, int]  # task -> result
    time_seed: int
    random_seed: int
    hash_seed: int
    exec_order: list[int]


def _make_runtime_state() -> RuntimeState:
    return RuntimeState(
        pending=set(TASKS),
        running=set(),
        completed=set(),
        results={},
        time_seed=0,
        random_seed=0,
        hash_seed=0,
        exec_order=[],
    )


def schedule_task(state: RuntimeState, t: int) -> RuntimeState:
    assert t in state.pending
    return RuntimeState(
        pending=state.pending - {t},
        running=state.running | {t},
        completed=state.completed,
        results=dict(state.results),
        time_seed=state.time_seed,
        random_seed=state.random_seed,
        hash_seed=state.hash_seed,
        exec_order=list(state.exec_order),
    )


def complete_task(state: RuntimeState, t: int) -> RuntimeState:
    assert t in state.running
    return RuntimeState(
        pending=state.pending,
        running=state.running - {t},
        completed=state.completed | {t},
        results={**state.results, t: task_result(t)},
        time_seed=state.time_seed,
        random_seed=state.random_seed,
        hash_seed=state.hash_seed,
        exec_order=state.exec_order + [t],
    )


# ---------------------------------------------------------------------------
# Build determinism tests
# ---------------------------------------------------------------------------


class TestBuildReproducibility:
    """Same input + same seed = same output (I4: finalDeterministic)."""

    def _valid_build_orders(self) -> list[list[int]]:
        """Generate all valid topological orderings of the dependency graph."""
        valid = []
        for perm in itertools.permutations(range(N_MODULES)):
            built: set[int] = set()
            ok = True
            for m in perm:
                if not deps_ready(m, built):
                    ok = False
                    break
                built.add(m)
            if ok:
                valid.append(list(perm))
        return valid

    def test_all_orderings_produce_same_digest(self) -> None:
        """Every valid build order produces the same final link digest."""
        orderings = self._valid_build_orders()
        assert len(orderings) > 1, "Need multiple orderings to test"

        canonical = link_digest(MODULES)
        for order in orderings:
            state = build_all(order)
            computed = sum(state.artifacts[m] for m in state.done)
            assert computed == canonical, (
                f"Order {order} produced digest {computed}, expected {canonical}"
            )

    @pytest.mark.parametrize(
        "order",
        [
            [0, 1, 2, 3],
            [0, 2, 1, 3],
        ],
    )
    def test_specific_orderings_match(self, order: list[int]) -> None:
        state = build_all(order)
        assert state.done == MODULES
        assert sum(state.artifacts[m] for m in MODULES) == link_digest(MODULES)

    def test_repeated_builds_identical(self) -> None:
        """Building the same order twice produces identical results."""
        order = [0, 1, 2, 3]
        state1 = build_all(order)
        state2 = build_all(order)
        assert state1.artifacts == state2.artifacts


class TestHashMapOrderingDoesNotLeak:
    """Test that HashMap/dict iteration ordering doesn't affect output."""

    def test_dict_merge_order_independent(self) -> None:
        """Simulates module artifacts merged in different orders."""
        artifacts = {m: compile_digest(m) for m in MODULES}

        # Merge in different orders, canonical output via sorted keys
        orders = [
            [0, 1, 2, 3],
            [3, 2, 1, 0],
            [2, 0, 3, 1],
        ]
        results = []
        for order in orders:
            merged: dict[int, int] = {}
            for m in order:
                merged[m] = artifacts[m]
            # Canonical output: sorted by key
            canonical = tuple(merged[k] for k in sorted(merged.keys()))
            results.append(canonical)

        assert all(r == results[0] for r in results)

    def test_set_operations_order_independent(self) -> None:
        """Set union is commutative -- build sets from different orders."""
        s1: set[int] = set()
        s2: set[int] = set()
        for m in [0, 1, 2, 3]:
            s1.add(compile_digest(m))
        for m in [3, 2, 1, 0]:
            s2.add(compile_digest(m))
        assert s1 == s2
        assert sum(sorted(s1)) == sum(sorted(s2))


class TestParallelBuildDeterminism:
    """Test that parallel (layer-based) builds produce identical artifacts."""

    def _layer_build(self) -> list[set[int]]:
        """Compute layers: modules whose deps are all in previous layers."""
        built: set[int] = set()
        remaining = set(MODULES)
        layers = []
        while remaining:
            layer = ready_set(remaining, built)
            assert layer, "Deadlock: no modules ready"
            layers.append(layer)
            built |= layer
            remaining -= layer
        return layers

    def test_layer_decomposition(self) -> None:
        """Verify the dependency graph produces expected layers."""
        layers = self._layer_build()
        # Layer 0: module 0 (no deps)
        # Layer 1: modules 1, 2 (depend only on 0)
        # Layer 2: module 3 (depends on 1, 2)
        assert layers == [{0}, {1, 2}, {3}]

    def test_parallel_vs_sequential_same_result(self) -> None:
        """Parallel (layer) build produces same artifacts as sequential."""
        layers = self._layer_build()

        # Parallel: build all modules in each layer
        parallel_artifacts = {}
        for layer in layers:
            for m in layer:
                parallel_artifacts[m] = compile_digest(m)

        # Sequential: build in topological order
        sequential_artifacts = {}
        for m in [0, 1, 2, 3]:
            sequential_artifacts[m] = compile_digest(m)

        assert parallel_artifacts == sequential_artifacts

    @pytest.mark.parametrize(
        "layer_order",
        [
            # Different orderings within layer 1 (modules 1 and 2)
            [[0], [1, 2], [3]],
            [[0], [2, 1], [3]],
        ],
    )
    def test_within_layer_order_irrelevant(self, layer_order: list[list[int]]) -> None:
        """Order within a parallel layer does not affect the output."""
        artifacts = {}
        for layer in layer_order:
            for m in layer:
                artifacts[m] = compile_digest(m)

        canonical = link_digest(MODULES)
        assert sum(artifacts[m] for m in MODULES) == canonical


class TestDependenciesRespected:
    """I1: no module built before its dependencies."""

    def test_all_valid_orderings_respect_deps(self) -> None:
        for perm in itertools.permutations(range(N_MODULES)):
            built: set[int] = set()
            valid = True
            for m in perm:
                if not deps_ready(m, built):
                    valid = False
                    break
                built.add(m)
            if valid:
                # Verify: at each step, deps were satisfied
                built2: set[int] = set()
                for m in perm:
                    assert deps(m).issubset(built2)
                    built2.add(m)

    def test_invalid_order_detected(self) -> None:
        """Building module 3 before module 1 violates dependency."""
        assert not deps_ready(3, {0})  # needs 1 and 2
        assert not deps_ready(3, {0, 1})  # needs 2
        assert deps_ready(3, {0, 1, 2})  # all deps met


class TestCacheCorrectness:
    """I2: cached entries match deterministic compile output."""

    def test_cache_hit_matches_fresh_compile(self) -> None:
        state = _make_build_state()
        # Build module 0 (populates cache)
        state = build_one(state, 0)
        cached_digest = state.cache[0]
        fresh_digest = compile_digest(0)
        assert cached_digest == fresh_digest

    def test_all_modules_cache_correctly(self) -> None:
        state = build_all([0, 1, 2, 3])
        for m in MODULES:
            assert state.cache[m] == compile_digest(m)


class TestHashSeedPinning:
    """I3: PYTHONHASHSEED always pinned to 0."""

    def test_seed_never_drifts(self) -> None:
        state = _make_build_state()
        assert state.hash_seed == 0
        for m in [0, 1, 2, 3]:
            state = build_one(state, m)
            assert state.hash_seed == 0


class TestNoDuplicateArtifacts:
    """I6: each module produces exactly one artifact."""

    def test_no_duplicate_digests_per_module(self) -> None:
        state = build_all([0, 1, 2, 3])
        # Each module appears exactly once
        assert len(state.artifacts) == N_MODULES
        for m in MODULES:
            assert m in state.artifacts


# ---------------------------------------------------------------------------
# Runtime determinism tests
# ---------------------------------------------------------------------------


class TestRuntimeReproducibility:
    """Same task always produces same result regardless of execution order."""

    _ORDERINGS = [
        [0, 1, 2, 3],
        [3, 2, 1, 0],
        [2, 0, 3, 1],
        [1, 3, 0, 2],
    ]

    @pytest.mark.parametrize(
        "exec_order",
        _ORDERINGS,
        ids=[f"order_{''.join(map(str, o))}" for o in _ORDERINGS],
    )
    def test_results_deterministic(self, exec_order: list[int]) -> None:
        """I2: task results are deterministic regardless of execution order."""
        state = _make_runtime_state()
        for t in exec_order:
            state = schedule_task(state, t)
            state = complete_task(state, t)

        for t in TASKS:
            assert state.results[t] == task_result(t), (
                f"Task {t} result {state.results[t]} != expected {task_result(t)}"
            )

    @pytest.mark.parametrize(
        "exec_order",
        _ORDERINGS,
        ids=[f"order_{''.join(map(str, o))}" for o in _ORDERINGS],
    )
    def test_all_results_present(self, exec_order: list[int]) -> None:
        """I4: when all tasks complete, every task has exactly one result."""
        state = _make_runtime_state()
        for t in exec_order:
            state = schedule_task(state, t)
            state = complete_task(state, t)

        assert state.completed == TASKS
        for t in TASKS:
            assert t in state.results


class TestSeedsPinned:
    """I1: seeds never drift from zero in deterministic mode."""

    def test_seeds_zero_after_all_tasks(self) -> None:
        state = _make_runtime_state()
        for t in [0, 1, 2, 3]:
            state = schedule_task(state, t)
            state = complete_task(state, t)
            assert state.time_seed == 0
            assert state.random_seed == 0
            assert state.hash_seed == 0


class TestTaskConservation:
    """I5: pending + running + completed = all tasks at every step."""

    def test_conservation_holds_throughout(self) -> None:
        state = _make_runtime_state()
        assert state.pending | state.running | state.completed == TASKS

        for t in [0, 1, 2, 3]:
            state = schedule_task(state, t)
            assert state.pending | state.running | state.completed == TASKS

            state = complete_task(state, t)
            assert state.pending | state.running | state.completed == TASKS


class TestNoPhantomCompletions:
    """I3: no task completed without being scheduled."""

    def test_no_phantom(self) -> None:
        state = _make_runtime_state()
        # Schedule and complete tasks one by one
        for t in [0, 1, 2, 3]:
            # Before scheduling, t is in pending
            assert t in state.pending
            assert t not in state.completed
            state = schedule_task(state, t)
            state = complete_task(state, t)
            # Now completed
            assert t in state.completed
            assert t not in state.pending


class TestNoDuplicateResults:
    """I6: no duplicate results for the same task."""

    def test_same_task_same_result(self) -> None:
        """Running same tasks in different orders yields same per-task results."""
        results_by_order = []
        for order in [[0, 1, 2, 3], [3, 2, 1, 0], [2, 0, 3, 1]]:
            state = _make_runtime_state()
            for t in order:
                state = schedule_task(state, t)
                state = complete_task(state, t)
            results_by_order.append(dict(state.results))

        for r in results_by_order[1:]:
            assert r == results_by_order[0]


class TestOrderDependentBug:
    """Negative test: demonstrates the known-bad model from the Quint spec.

    A polynomial hash chain (acc * 31 + digest) is order-dependent.
    This test verifies that the model checker would catch this bug.
    """

    def test_polynomial_hash_is_order_dependent(self) -> None:
        """Polynomial hash(A, B, C) != hash(C, A, B)."""

        def poly_hash(order: list[int]) -> int:
            acc = 0
            for m in order:
                acc = acc * 31 + (m + 1)  # compileDigest = m + 1
            return acc

        h012 = poly_hash([0, 1, 2])
        h210 = poly_hash([2, 1, 0])
        h120 = poly_hash([1, 2, 0])

        # These SHOULD be different -- proving the bug
        assert h012 != h210, "Polynomial hash should be order-dependent"
        assert h012 != h120, "Polynomial hash should be order-dependent"

    def test_additive_hash_is_order_independent(self) -> None:
        """Additive combination (the correct approach) is order-independent."""

        def additive_hash(order: list[int]) -> int:
            return sum(compile_digest(m) for m in order)

        h012 = additive_hash([0, 1, 2])
        h210 = additive_hash([2, 1, 0])
        h120 = additive_hash([1, 2, 0])

        assert h012 == h210
        assert h012 == h120
