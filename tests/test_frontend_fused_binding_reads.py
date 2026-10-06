"""Fused loop consumers: admission, decline, and the bindings they read early.

A fused reduction runs its loop in bounded chunks, each in one runtime kernel
call inside a chunk loop whose back edge observes pending asynchronous work;
the ordinary loop then continues on the same iterator. The split/count kernel
and the fused statement ``d[k] = d.get(k, 0) + delta`` run whole or decline
before any effect. The frontend only selects the shape; the kernel admits the
values the loop reads or declines. Where a fused op reads a binding before the
loop does, it must read the binding's current value: from Python 3.13 a
callback can rebind a running frame's local through ``frame.f_locals`` (PEP
667), so a cached view serves only where the binding analysis proves the
source read clean; 3.12 keeps it. A read the fused op moves ahead of the
source's own must also be unable to raise: a callback may rebind a fast local
but never delete it, while a global or a cell it may delete.

Fills of ``bytearray(n)`` need an exact builtin identity for ``bytearray``,
which the binding analysis does not give these fixtures; they are covered here
by decline only. The CPython-observable consequences are pinned by
``tests/differential/basic/fused_loop_semantics.py`` and
``tests/differential/basic/locals_mutation_semantics.py``.
"""

from __future__ import annotations

import ast
import inspect
from typing import Literal

import pytest

from molt.frontend import SimpleTIRGenerator

TARGETS = [(3, 12), (3, 13), (3, 14)]
_ALIASES = {"binding_alias", "copy", "copy_var", "identity_alias"}


def _module_ops(
    source: str,
    target_python: tuple[int, int],
    *,
    midend_stage: Literal["pre-midend", "post-midend"] = "post-midend",
) -> dict[str, list[dict]]:
    gen = SimpleTIRGenerator(module_name="__main__", target_python=target_python)
    gen.visit(ast.parse(source))
    functions = gen.to_json(midend_stage=midend_stage)["functions"]
    return {func["name"]: func["ops"] for func in functions}


def _function_ops(
    source: str,
    name: str,
    target_python: tuple[int, int],
    *,
    midend_stage: Literal["pre-midend", "post-midend"] = "post-midend",
) -> list[dict]:
    ops = _module_ops(source, target_python, midend_stage=midend_stage)
    return ops[f"__main____{name}"]


def _kinds(ops: list[dict]) -> set[str]:
    return {op["kind"] for op in ops}


def _vector_kinds(ops: list[dict]) -> set[str]:
    return {kind for kind in _kinds(ops) if kind.startswith("vec_")}


def _source(ops: list[dict], name: str) -> dict:
    """The op producing ``name``, through value aliases."""
    producers = {op["out"]: op for op in ops if isinstance(op.get("out"), str)}
    op = producers[name]
    while op["kind"] in _ALIASES and op.get("args"):
        op = producers[op["args"][0]]
    return op


def _source_kind(ops: list[dict], name: str) -> str:
    return _source(ops, name)["kind"]


def _branches(ops: list[dict], guard: int) -> tuple[range, range]:
    """The then and else op ranges of the ``if`` at ``guard``."""
    depth = 0
    otherwise: int | None = None
    for index in range(guard + 1, len(ops)):
        kind = ops[index]["kind"]
        if kind == "if":
            depth += 1
        elif kind == "else" and depth == 0:
            otherwise = index
        elif kind == "end_if":
            if depth == 0:
                split = index if otherwise is None else otherwise
                return range(guard + 1, split), range(split + 1, index)
            depth -= 1
    raise AssertionError("unterminated if")


REDUCTIONS = """
def reduce_list(callback):
    seq = [1, 2, 3]
    total = 0
    for item in seq:
        total += item
    return total, item

def reduce_tuple_product(callback):
    seq = (2, 3, 4)
    product = 1
    for item in seq:
        product = product * item
    return product

def reduce_min(callback):
    seq = [3, 1, 2]
    best = 10
    for item in seq:
        if item < best:
            best = item
    return best

def reduce_max(callback):
    seq = [3, 1, 2]
    best = -10
    for item in seq:
        if best < item:
            best = item
    return best

def reduce_list_after_call(callback):
    seq = [1, 2, 3]
    total = 0
    callback()
    for item in seq:
        total += item
    return total

def total_after_call(callback):
    total = 0.5
    callback()
    seq = [1, 2, 3]
    for item in seq:
        total += item
    return total
"""


@pytest.mark.parametrize("target_python", TARGETS)
@pytest.mark.parametrize(
    ("name", "kind"),
    [
        ("reduce_list", "vec_sum"),
        ("reduce_tuple_product", "vec_prod"),
        ("reduce_min", "vec_min"),
        ("reduce_max", "vec_max"),
    ],
)
def test_sequence_reduction_selects_its_fused_op(
    name: str, kind: str, target_python: tuple[int, int]
) -> None:
    assert _vector_kinds(_function_ops(REDUCTIONS, name, target_python)) == {kind}


@pytest.mark.parametrize("target_python", TARGETS)
def test_sequence_reduction_fuses_only_on_a_clean_sequence_read(
    target_python: tuple[int, int],
) -> None:
    ops = _function_ops(REDUCTIONS, "reduce_list_after_call", target_python)
    # From 3.13 the call may have rebound ``seq``: the loop reads the
    # binding's current value, whose type the frontend no longer knows.
    expected = set() if target_python >= (3, 13) else {"vec_sum"}
    assert _vector_kinds(ops) == expected


@pytest.mark.parametrize("target_python", TARGETS)
def test_fused_reduction_starts_from_the_accumulator_binding_itself(
    target_python: tuple[int, int],
) -> None:
    ops = _function_ops(
        REDUCTIONS, "total_after_call", target_python, midend_stage="pre-midend"
    )
    (fused,) = [op for op in ops if op["kind"] == "vec_sum"]
    accumulator = _source_kind(ops, fused["args"][1])
    # From 3.13 a callback may have rebound ``total`` (to an int, say): the
    # kernel must start from the binding, never the float cached before.
    if target_python >= (3, 13):
        assert accumulator == "frame_home_load"
    else:
        assert accumulator != "frame_home_load"


@pytest.mark.parametrize("target_python", TARGETS)
def test_fused_reduction_publishes_each_chunk_and_rereads_its_bindings(
    target_python: tuple[int, int],
) -> None:
    ops = _function_ops(
        REDUCTIONS, "reduce_list", target_python, midend_stage="pre-midend"
    )
    kinds = [op["kind"] for op in ops]
    (fused,) = [index for index, kind in enumerate(kinds) if kind == "vec_sum"]
    chunk_start = max(index for index in range(fused) if kinds[index] == "loop_start")
    chunk_end = kinds.index("loop_end", fused)
    it, accumulator, target = ops[fused]["args"]
    # One iterator, acquired before the chunk loop, serves the chunks and then
    # the ordinary loop, which continues where the last chunk stopped.
    assert kinds.count("iter") == 1
    assert kinds.index("iter") < chunk_start
    assert _source_kind(ops, it) == "iter"
    assert "loop_start" in kinds[chunk_end:]
    # The accumulator and loop target are reread on every pass: a signal
    # handler or pending call serviced at the back edge may have rebound them.
    for name in (accumulator, target):
        assert chunk_start < ops.index(_source(ops, name)) < fused
    if target_python >= (3, 13):
        assert _source_kind(ops, accumulator) == "frame_home_load"
    # After a chunk the loop target and then the accumulator are published,
    # before the chunk loop's back edge.
    stores = [
        ops[index].get("var")
        for index in range(fused, chunk_end)
        if kinds[index] == "store_var"
    ]
    assert stores.index("item") < stores.index("total")


SCOPES = """
seq = [1, 2, 3]
total = 0
for item in seq:
    total += item

class Namespace:
    values = [1, 2, 3]
    acc = 0
    for value in values:
        acc += value

async def reduce_async():
    seq = [1, 2, 3]
    total = 0
    for item in seq:
        total += item
    return total
"""


def test_reductions_fuse_only_where_the_loop_target_is_a_frame_local() -> None:
    # A module's target is a module global, a class body's an entry of a
    # namespace mapping that may observe every store, and a coroutine's lives
    # across suspensions: one final store cannot stand for a store per item.
    for name, ops in _module_ops(SCOPES, (3, 12)).items():
        assert not _vector_kinds(ops), name


SHADOWED = """
def reduce_indexed(range, len):
    seq = [1, 2, 3]
    total = 0
    for i in range(len(seq)):
        total += seq[i]
    return total

def fill(bytearray):
    data = bytearray(16)
    i = 0
    while i < 16:
        data[i] = 97
        i += 1
    return data
"""


@pytest.mark.parametrize("target_python", TARGETS)
def test_fusion_never_follows_a_shadowed_builtin_spelling(
    target_python: tuple[int, int],
) -> None:
    for name in ("reduce_indexed", "fill"):
        ops = _function_ops(SHADOWED, name, target_python)
        assert not _vector_kinds(ops), name
        assert "bytearray_fill_range" not in _kinds(ops), name


COUNTED = """
def counted(callback):
    i = 0
    last = -1
    while i < 10:
        last = 7
        i += 1
    return i, last

def counted_with_call(callback):
    i = 0
    while i < 10:
        callback()
        i += 1
    return i

def counted_from(start, callback):
    i = start
    while i < 10:
        i += 1
    return i
"""


def test_counted_while_fuses_where_no_callback_can_rebind_the_index() -> None:
    for name in ("counted", "counted_with_call", "counted_from"):
        ops = _function_ops(COUNTED, name, (3, 12))
        assert "loop_index_start" in _kinds(ops), name


@pytest.mark.parametrize("target_python", [(3, 13), (3, 14)])
def test_counted_while_never_fuses_an_index_a_body_callback_may_rebind(
    target_python: tuple[int, int],
) -> None:
    ops = _function_ops(COUNTED, "counted_with_call", target_python)
    assert "loop_index_start" not in _kinds(ops)


def test_counted_while_counts_only_from_an_exact_int_start() -> None:
    ops = _function_ops(COUNTED, "counted_from", (3, 12), midend_stage="pre-midend")
    (type_check,) = [op for op in ops if op["kind"] == "type_of"]
    (exact,) = [
        op["out"]
        for op in ops
        if op["kind"] == "is" and type_check["out"] in op["args"]
    ]
    (guard,) = [
        index
        for index, op in enumerate(ops)
        if op["kind"] == "if" and op["args"] == [exact]
    ]
    counted_branch, ordinary_branch = _branches(ops, guard)
    (counted,) = [
        ops[i] for i in counted_branch if ops[i]["kind"] == "loop_index_start"
    ]
    # The counted loop keeps the index in an int lane: it starts from the
    # value whose exact type was checked, and a float, bool or int subclass
    # start runs the ordinary loop in the else branch instead.
    assert _source(ops, counted["args"][0]) is _source(ops, type_check["args"][0])
    ordinary = {ops[i]["kind"] for i in ordinary_branch}
    assert "loop_break_if_false" in ordinary
    assert "loop_index_start" not in ordinary


SPLIT_COUNTS = """
def count_words(line, callback):
    counts = {}
    for word in line.split():
        counts[word] = counts.get(word, 0) + 1
    return counts

def count_fields_after_call(line, callback):
    counts = {}
    step = 2
    callback()
    for field in line.split("|"):
        counts[field] = counts.get(field, 0) + step
    return counts

def count_maybe_unbound(line, flag):
    if flag:
        counts = {}
    for word in line.split():
        counts[word] = counts.get(word, 0) + 1
    return counts

def count_reversed(line, callback):
    counts = {}
    for word in line.split():
        counts[word] = 1 + counts.get(word, 0)
    return counts

def count_by_target(line, callback):
    counts = {}
    for word in line.split():
        counts[word] = counts.get(word, 0) + word
    return counts
"""

_SPLIT_KINDS = {"string_split_ws_dict_inc", "string_split_sep_dict_inc"}


@pytest.mark.parametrize("target_python", TARGETS)
def test_split_count_fuses_with_its_loop_as_fallback(
    target_python: tuple[int, int],
) -> None:
    for name, kind in (
        ("count_words", "string_split_ws_dict_inc"),
        # A callback may rebind ``counts`` or ``step`` but never unbind them:
        # the fused op reads their current values, which cannot raise.
        ("count_fields_after_call", "string_split_sep_dict_inc"),
    ):
        ops = _function_ops(SPLIT_COUNTS, name, target_python)
        kinds = [op["kind"] for op in ops]
        assert _kinds(ops) & _SPLIT_KINDS == {kind}, name
        assert kinds.index(kind) < kinds.index("loop_start"), name


@pytest.mark.parametrize("target_python", TARGETS)
def test_split_count_declines_shapes_it_cannot_run_unobservably(
    target_python: tuple[int, int],
) -> None:
    # A possibly unbound dict would raise before ``line.split()`` ran; the
    # reversed sum calls ``delta.__add__`` first; a delta the loop rebinds
    # changes per word.
    for name in ("count_maybe_unbound", "count_reversed", "count_by_target"):
        ops = _function_ops(SPLIT_COUNTS, name, target_python)
        assert not _kinds(ops) & _SPLIT_KINDS, name


INCREMENTS = """
COUNTS = {}

def increment(counts, key, step):
    counts[key] = counts.get(key, 0) + step

def increment_after_call(counts, key, callback):
    callback()
    counts[key] = counts.get(key, 0) + 1

def increment_maybe_unbound(key, flag):
    if flag:
        counts = {}
    counts[key] = counts.get(key, 0) + 1

def increment_reversed(counts, key):
    counts[key] = 1 + counts.get(key, 0)

def increment_cell(key, callback):
    counts = {}
    def reset():
        nonlocal counts
        del counts
    callback()
    counts[key] = counts.get(key, 0) + 1
    return reset

def increment_global(key, callback):
    callback()
    COUNTS[key] = COUNTS.get(key, 0) + 1
"""


@pytest.mark.parametrize("target_python", TARGETS)
def test_dict_increment_fuses_with_its_statement_as_fallback(
    target_python: tuple[int, int],
) -> None:
    for name in ("increment", "increment_after_call"):
        ops = _function_ops(INCREMENTS, name, target_python)
        kinds = [op["kind"] for op in ops]
        assert kinds.count("dict_str_int_inc") == 1, name


@pytest.mark.parametrize("target_python", TARGETS)
def test_dict_increment_declines_reads_it_cannot_move(
    target_python: tuple[int, int],
) -> None:
    # The fused op reads the dict, key and delta before ``d.get``: none of
    # those reads may raise. A callback may delete a cell or a global.
    for name in (
        "increment_maybe_unbound",
        "increment_reversed",
        "increment_cell",
        "increment_global",
    ):
        ops = _function_ops(INCREMENTS, name, target_python)
        assert "dict_str_int_inc" not in _kinds(ops), name


@pytest.mark.parametrize("target_python", TARGETS)
@pytest.mark.parametrize(
    "source",
    [
        """
def after_empty():
    total = 0
    for item in []:
        total += item
    return item
""",
        """
def after_empty():
    for item in []:
        pass
    return item
""",
        """
def after_empty():
    for item in range(0):
        pass
    return item
""",
        """
def after_empty():
    item = 1
    if True:
        del item
    return item
""",
        """
def after_empty():
    def capture():
        return item
    for item in []:
        pass
    return item
""",
        """
async def after_empty():
    for item in []:
        pass
    return item
""",
    ],
)
def test_source_binding_fact_guards_empty_loop_and_deleted_local(source, target_python):
    # The source program establishes the oracle independently of emitter state.
    # Lowering must preserve that local-read failure across every storage form.
    namespace = {}
    exec(compile(source, "<binding-oracle>", "exec"), namespace)
    function = namespace["after_empty"]
    if inspect.iscoroutinefunction(function):
        coroutine = function()
        try:
            with pytest.raises(UnboundLocalError):
                coroutine.send(None)
        finally:
            coroutine.close()
    else:
        with pytest.raises(UnboundLocalError):
            function()
    guarded = []

    class RecordingGenerator(SimpleTIRGenerator):
        def _emit_unbound_local_guard(self, value, name):
            guarded.append(name)
            super()._emit_unbound_local_guard(value, name)

    gen = RecordingGenerator(module_name="__main__", target_python=target_python)
    gen.visit(ast.parse(source))
    assert "item" in guarded
