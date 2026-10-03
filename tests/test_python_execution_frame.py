"""Source-frame argument zero and PEP 709 storage ownership."""

import ast
from dataclasses import dataclass

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend._types import BUILTIN_TYPE_TAGS, CodeSlotDeclaration
from molt.frontend.lowering.function_lifecycle import FunctionLifecycleMixin
from tools.check_ir_structure import verify_frontend_tir


def compile_source(source, target=(3, 14)):
    generator = SimpleTIRGenerator(target_python=target)
    generator.visit(ast.parse(source))
    ir = generator.to_json()
    verification = verify_frontend_tir(ir)
    assert verification.ok, verification.errors
    return generator, ir


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "expression",
    ["super(int, 1)", "super(*(int, 1))", "super(type=int, object=1)", "super(**{})"],
)
def test_explicit_super_uses_ordinary_call_dispatch(expression, target):
    # Exercise module and function scope through their real semantic analysis.
    # Expanded calls require the module's argument plan before lowering.
    generator = SimpleTIRGenerator(target_python=target)
    generator.visit(ast.parse(expression))
    assert_explicit_super_arguments(generator.current_ops, expression)
    assert not runtime_calls(generator.current_ops, "molt_super_from_frame")
    compiled, _ = compile_source(f"def probe(): return {expression}\n", target)
    ops = super_consumer_ops(compiled)
    assert_explicit_super_arguments(ops, expression)
    assert not runtime_calls(ops, "molt_super_from_frame")


@dataclass(frozen=True)
class FramePublication:
    index: int
    argument: MoltValue
    kind: int
    class_cell: MoltValue


def runtime_calls(ops: list[MoltOp], symbol: str) -> list[int]:
    return [
        index
        for index, op in enumerate(ops)
        if op.kind == "CALL" and op.args and op.args[0] == symbol
    ]


def names_callable(producers: dict[str, MoltOp], value: MoltValue, name: str) -> bool:
    """Match the actual callable operand, never an unrelated nearby name load."""
    callee = producers.get(value.name)
    if callee is None:
        return False
    if callee.kind == "MODULE_GET_GLOBAL":
        name_op = producers[callee.args[1].name]
        return name_op.kind == "CONST_STR" and name_op.args == [name]
    if callee.kind == "BUILTIN_TYPE" and name in BUILTIN_TYPE_TAGS:
        tag = producers[callee.args[0].name]
        return tag.kind == "CONST" and tag.args == [BUILTIN_TYPE_TAGS[name]]
    return False


def builtin_calls(ops: list[MoltOp], name: str = "super") -> list[int]:
    """Resolve retained callable operands or the admitted frame-super primitive."""
    producers: dict[str, MoltOp] = {}
    calls = []
    for index, op in enumerate(ops):
        if (
            name == "super"
            and op.kind == "CALL"
            and op.args == ["molt_super_from_frame"]
        ):
            calls.append(index)
        elif op.kind in {"CALL_FUNC", "CALL_BIND", "CALL_INDIRECT"}:
            assert op.args and isinstance(op.args[0], MoltValue)
            if names_callable(producers, op.args[0], name):
                calls.append(index)
        producers[op.result.name] = op
    return calls


def assert_explicit_super_arguments(ops: list[MoltOp], expression: str) -> MoltOp:
    (index,) = builtin_calls(ops)
    call = ops[index]
    producers = {op.result.name: op for op in ops[:index]}
    assert names_callable(producers, call.args[0], "super")

    def assert_pair(values):
        assert len(values) == 2
        assert names_callable(producers, values[0], "int")
        assert producers[values[1].name].kind == "CONST"
        assert producers[values[1].name].args == [1]

    if expression == "super(int, 1)":
        assert call.kind == "CALL_FUNC"
        assert_pair(call.args[1:])
        return call

    assert call.kind in {"CALL_BIND", "CALL_INDIRECT"}
    assert len(call.args) == 2
    builder = call.args[1]
    assert producers[builder.name].kind == "CALLARGS_NEW"
    steps = [
        op
        for op in ops[:index]
        if op.kind.startswith("CALLARGS_") and op.args and op.args[0] == builder
    ]
    if expression == "super(*(int, 1))":
        (expand,) = steps
        assert expand.kind == "CALLARGS_EXPAND_STAR"
        materialized = producers[expand.args[1].name]
        assert materialized.kind == "CALL_FUNC"
        assert len(materialized.args) == 2
        assert names_callable(producers, materialized.args[0], "tuple")
        pair = producers[materialized.args[1].name]
        assert pair.kind == "TUPLE_NEW"
        assert_pair(pair.args)
    elif expression == "super(type=int, object=1)":
        assert len(steps) == 2
        assert all(op.kind == "CALLARGS_PUSH_KW" for op in steps)
        assert [producers[op.args[1].name].args for op in steps] == [
            ["type"],
            ["object"],
        ]
        assert_pair([op.args[2] for op in steps])
    else:
        assert expression == "super(**{})"
        (expand,) = steps
        assert expand.kind == "CALLARGS_EXPAND_KWSTAR"
        mapping = producers[expand.args[1].name]
        assert mapping.kind == "DICT_NEW" and mapping.args == []
    return call


@pytest.mark.parametrize("kind", ["CALL_FUNC", "CALL_BIND", "CALL_INDIRECT"])
def test_builtin_consumer_requires_the_actual_preceding_callee_definition(kind):
    key = MoltValue("key", type_hint="str")
    named = MoltValue("named")
    unrelated = MoltValue("unrelated")
    builder = MoltValue("builder", type_hint="callargs")
    prefix = [
        MoltOp(kind="CONST_STR", args=["super"], result=key),
        MoltOp(kind="CALLARGS_NEW", args=[], result=builder),
    ]
    lookup = MoltOp(
        kind="MODULE_GET_GLOBAL", args=[MoltValue("module"), key], result=named
    )
    tail = [] if kind == "CALL_FUNC" else [builder]
    wrong_call = MoltOp(kind=kind, args=[unrelated, *tail], result=MoltValue("wrong"))
    call = MoltOp(kind=kind, args=[named, *tail], result=MoltValue("result"))
    assert builtin_calls([*prefix, lookup, wrong_call]) == []
    assert builtin_calls([*prefix, call, lookup]) == []
    assert builtin_calls([*prefix, lookup, call]) == [len(prefix) + 1]


def publications(ops: list[MoltOp]) -> list[FramePublication]:
    producers = {op.result.name: op for op in ops}
    result = []
    for index, op in enumerate(ops):
        if op.kind != "FRAME_CONTEXT_SET":
            continue
        argument, kind, class_cell = op.args
        assert isinstance(argument, MoltValue)
        assert isinstance(kind, MoltValue)
        assert isinstance(class_cell, MoltValue)
        kind_op = producers[kind.name]
        assert kind_op.kind == "CONST"
        # 3: a synchronous frame's argument zero is its first code slot's home.
        assert kind_op.args[0] in (0, 1, 2, 3)
        result.append(FramePublication(index, argument, kind_op.args[0], class_cell))
    assert result, "the executing function never published its semantic frame"
    return result


def home_stores(
    ops: list[MoltOp], slot: int, kind: str = "FRAME_HOME_STORE"
) -> list[int]:
    return [
        index
        for index, op in enumerate(ops)
        if op.kind == kind and op.metadata == {"slot": slot}
    ]


def assert_home_argument_zero(ops: list[MoltOp], frame: FramePublication) -> None:
    """Kind 3 carries no value: super() reads code slot 0 when it runs."""
    assert frame.kind == 3
    producer = next(op for op in ops if op.result == frame.argument)
    assert producer.kind == "CONST_NONE"


def super_consumer_ops(
    generator: SimpleTIRGenerator, *, generator_frame: bool = False
) -> list[MoltOp]:
    matches = [
        function["ops"]
        for name, function in generator.funcs_map.items()
        if ("genexpr_" in name) == generator_frame and builtin_calls(function["ops"])
    ]
    assert len(matches) == 1
    return matches[0]


def storage_owner(ops: list[MoltOp], value: MoltValue) -> tuple[object, ...]:
    """Compare actual slot/cell transport, not incidental SSA extraction names."""
    producer = next((op for op in ops if op.result.name == value.name), None)
    if producer is None:
        return ("parameter", value.name)
    if producer.kind in {"BINDING_ALIAS", "IDENTITY_ALIAS"}:
        return storage_owner(ops, producer.args[0])
    if producer.kind == "LOAD_VAR":
        return ("local", producer.metadata["var"])
    if producer.kind in {"FRAME_HOME_STORE", "FRAME_HOME_LOAD"}:
        return ("frame-home", producer.metadata["slot"])
    if producer.kind == "LOAD_CLOSURE":
        return ("task", producer.args[0], producer.args[1])
    return ("value", value.name)


def frame_before(ops: list[MoltOp], index: int) -> FramePublication:
    return next(frame for frame in reversed(publications(ops)) if frame.index < index)


@pytest.mark.parametrize(
    ("signature", "expected"),
    [
        ("receiver, /, value", "receiver"),
        ("receiver, value=1", "receiver"),
        ("*args", None),
        ("*, receiver", None),
        ("**kwargs", None),
        ("", None),
    ],
)
def test_argzero_is_source_positional_not_transport_or_keyword(signature, expected):
    node = ast.parse(f"def function({signature}): pass").body[0]
    assert FunctionLifecycleMixin._python_first_positional_arg(node.args) == expected
    source = f"def function({signature}): return super()\n"
    generator, _ = compile_source(source)
    ops = super_consumer_ops(generator)
    frame = publications(ops)[0]
    if expected is None:
        assert frame.kind == 0
        assert (
            next(op for op in ops if op.result == frame.argument).kind == "CONST_NONE"
        )
    else:
        assert_home_argument_zero(ops, frame)
        # Code slot 0, CPython's localsplus[0], is the positional parameter.
        code = next(
            const
            for const in compile(source, "<argzero>", "exec").co_consts
            if hasattr(const, "co_varnames")
        )
        assert code.co_varnames[0] == expected
        store = ops[home_stores(ops, 0)[0]]
        assert storage_owner(ops, store.args[0])[-1] == expected


def test_source_frame_argument_is_reset_and_restored():
    generator = SimpleTIRGenerator()
    generator.start_function(
        "outer",
        params=["receiver"],
        python_first_arg="receiver",
        code_slots=CodeSlotDeclaration(("receiver",), ("receiver",), (), ()),
    )
    generator.python_frame_context_active = True
    saved = generator._capture_function_state()
    generator.start_function(
        "poll",
        params=["self"],
        compiler_params={"self"},
        code_slots=CodeSlotDeclaration((), (), (), ()),
    )
    assert generator.current_python_first_arg is None
    assert not generator.python_frame_context_active
    generator._restore_function_state(saved)
    assert generator.current_python_first_arg == "receiver"
    assert generator.python_frame_context_active


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "expression",
    [
        "[super() for item in values]",
        "{super() for item in values}",
        "{item: super() for item in values}",
        "[super() for item in values for inner in values]",
        "[[super() for inner in values] for item in values]",
        "[super() for (item, *rest) in values]",
    ],
)
def test_materialized_shapes_never_manufacture_generator_frames(expression, target):
    _, ir = compile_source(
        "class Subject:\n"
        "    def method(receiver, values):\n"
        f"        return {expression}\n",
        target,
    )
    assert not any("genexpr_" in fn["name"] for fn in ir["functions"])


def test_pep709_super_reads_shadowed_slot_and_restores_owner():
    generator, _ = compile_source(
        "class Subject:\n"
        "    def method(receiver):\n"
        "        values = [super() for receiver in (0,)]\n"
        "        return super()\n"
    )
    ops = super_consumer_ops(generator)
    inner_call, outer_call = builtin_calls(ops)
    # One publication: argument zero is code slot 0, which the comprehension's
    # binding takes over and then gives back, so no scope republishes it.
    (entry,) = publications(ops)
    assert_home_argument_zero(ops, entry)
    (take,) = home_stores(ops, 0, "FRAME_HOME_TAKE")
    restores = [
        index for index in home_stores(ops, 0) if ops[index].args == [ops[take].result]
    ]
    assert len(restores) == 2, "normal and exceptional exits both give it back"
    shadow = next(
        index for index in home_stores(ops, 0) if take < index and index not in restores
    )
    assert entry.index < take < shadow < inner_call < min(restores)
    assert max(restores) < outer_call


def test_real_generator_super_reads_hidden_iterator_not_enclosing_self():
    generator, ir = compile_source(
        "class Subject:\n"
        "    def method(receiver):\n"
        "        return (super() for item in (1,))\n"
    )
    assert any("genexpr_" in fn["name"] for fn in ir["functions"])
    ops = super_consumer_ops(generator, generator_frame=True)
    frame = publications(ops)[0]
    assert frame.kind == 1
    owner = storage_owner(ops, frame.argument)
    assert owner[0] == "task", "generator argument zero must come from its payload"
    assert frame.argument.name != "self", "task transport is not Python argument zero"
    # The payload is copied to the loop's persistent iterator slot. Follow the
    # real store/load chain instead of requiring identical payload offsets.
    iterator_index, iterator = next(
        (index, op) for index, op in enumerate(ops) if op.kind == "ITER_NEXT"
    )
    loop_owner = storage_owner(ops, iterator.args[0])
    assert loop_owner[0] == "task"
    initializers = [
        op
        for op in ops[:iterator_index]
        if op.kind == "STORE_CLOSURE" and tuple(op.args[:2]) == loop_owner[1:]
    ]
    assert len(initializers) == 1
    assert storage_owner(ops, initializers[0].args[2]) == owner


@pytest.mark.parametrize("reducer", ["sum", "any", "all"])
def test_frame_observing_generator_reductions_retain_real_frame(reducer):
    _, ir = compile_source(
        "class Subject:\n"
        "    def method(receiver):\n"
        f"        return {reducer}(super() for item in range(2))\n"
    )
    assert any("genexpr_" in fn["name"] for fn in ir["functions"])


def test_deferred_arithmetic_reduction_retains_live_callable_and_generator():
    generator, ir = compile_source(
        "def total(values): return sum(item * item for item in values)\n"
    )
    assert any("genexpr_" in fn["name"] for fn in ir["functions"])
    ((ops, index),) = [
        (function["ops"], index)
        for function in generator.funcs_map.values()
        for index in builtin_calls(function["ops"], "sum")
    ]
    call = ops[index]
    assert call.kind == "CALL_FUNC" and len(call.args) == 2
    producers = {op.result.name: op for op in ops[:index]}
    argument = producers[call.args[1].name]
    assert argument.kind == "CALL_FUNC"
    constructor = producers[argument.args[0].name]
    assert constructor.kind in {"FUNC_NEW", "FUNC_NEW_CLOSURE"}
    assert "genexpr_" in constructor.args[0]


@pytest.mark.parametrize(
    "expression",
    [
        "[lambda: item for item in values for inner in values]",
        "[[lambda: item for inner in values] for item in values]",
        "[lambda: item async for item in values]",
    ],
)
def test_scoped_capture_storage_and_async_transport_verify(expression):
    prefix = "async " if "async for" in expression else ""
    generator, ir = compile_source(
        f"{prefix}def make(values):\n    return {expression}\n"
    )
    assert any("lambda" in fn["name"] for fn in ir["functions"])
    assert not any("genexpr_" in fn["name"] for fn in ir["functions"])
    assert generator.comprehension_bindings == {}


def test_async_materialized_super_retains_method_frame():
    generator, ir = compile_source(
        "class Subject:\n"
        "    async def method(receiver, values):\n"
        "        return [super() async for item in values]\n"
    )
    ops = super_consumer_ops(generator)
    frames = publications(ops)
    owner = storage_owner(ops, frames[0].argument)
    assert owner[0] == "task"
    assert all(frame.kind == 1 for frame in frames)
    assert all(storage_owner(ops, frame.argument) == owner for frame in frames)
    assert not any("genexpr_" in fn["name"] for fn in ir["functions"])


def test_projection_mask_restores_only_its_source_names():
    from molt.frontend.lowering.local_bindings import _mask_binding_projection

    projection = {"target": "outer", "unrelated": "before"}
    restore = _mask_binding_projection(projection, {"target", "new_target"})
    assert projection == {"unrelated": "before"}
    projection.update(target="inner", new_target="inner", unrelated="after")
    restore()
    assert projection == {"target": "outer", "unrelated": "after"}


@pytest.mark.parametrize("outer_super", [False, True])
def test_class_name_target_does_not_replace_frame_cell(outer_super):
    prefix = "        before = super()\n" if outer_super else ""
    _, ir = compile_source(
        "class Subject:\n"
        "    def method(receiver):\n"
        + prefix
        + "        return [super() for __class__ in (int,)]\n"
    )
    assert not any("genexpr_" in fn["name"] for fn in ir["functions"])


def test_argument_replacement_and_deletion_write_only_the_home():
    generator, _ = compile_source(
        "def frame(receiver, replacement):\n"
        "    receiver = replacement\n"
        "    del receiver\n"
        "    return super()\n"
    )
    ops = super_consumer_ops(generator)
    # super() reads argument zero from its home when it runs, so it sees the
    # deletion without any republication or snapshot.
    (entry,) = publications(ops)
    assert_home_argument_zero(ops, entry)
    parameter, replacement = home_stores(ops, 0)
    (clear,) = home_stores(ops, 0, "FRAME_HOME_CLEAR")
    (consumer,) = builtin_calls(ops)
    assert parameter < entry.index < replacement < clear < consumer
    assert storage_owner(ops, ops[parameter].args[0])[-1] == "receiver"
    assert not any(op.kind in {"DELETE_VAR", "DEL_BOUNDARY"} for op in ops)


def test_captured_argument_publishes_the_real_mutable_cell_and_class_cell():
    generator, _ = compile_source(
        "class Subject:\n"
        "    def method(receiver, replacement):\n"
        "        capture = lambda: receiver\n"
        "        receiver = replacement\n"
        "        return super(), capture\n"
    )
    ops = super_consumer_ops(generator)
    frames = publications(ops)
    for frame in frames:
        assert_home_argument_zero(ops, frame)
    # Code slot 0's home holds the receiver's real cell, whose contents
    # super() reads; the frame reaches the cell through the home's view.
    (home,) = home_stores(ops, 0, "FRAME_HOME_CELL")
    cell = ops[home].result
    cell_producer = next(op for op in ops if op.result == ops[home].args[0])
    assert cell.type_hint == "cell"
    assert cell_producer.kind == "CALL"
    assert cell_producer.args[0] == "molt_cell_new"
    assert len(cell_producer.args) == 2
    (constructor,) = [
        op for op in ops if op.kind == "FUNC_NEW_CLOSURE" and "lambda" in op.args[0]
    ]
    captures = next(op for op in ops if op.result == constructor.args[2])
    assert captures.kind == "TUPLE_NEW" and captures.args == [cell]
    lambda_ops = generator.funcs_map[constructor.args[0]]["ops"]
    (read,) = runtime_calls(lambda_ops, "molt_cell_get")
    loaded_cell = next(op for op in lambda_ops if op.result == lambda_ops[read].args[1])
    assert loaded_cell.kind == "INDEX"
    assert loaded_cell.args[0].name == "__molt_closure__"
    slot = next(op for op in lambda_ops if op.result == loaded_cell.args[1])
    assert slot.kind == "CONST" and slot.args == [0]
    writes = [
        ops[index]
        for index in runtime_calls(ops, "molt_cell_set")
        if ops[index].args[1] == cell
    ]
    assert writes
    # The replacement parameter's value, read through its own home's view.
    (replacement_home,) = home_stores(ops, 1)
    assert storage_owner(ops, ops[replacement_home].args[0])[-1] == "replacement"
    # A source read is an independent captured value, not the entry store's
    # SSA result. It must read this parameter's live local or canonical home.
    assert any(
        storage_owner(ops, write.args[2])
        in {("local", "replacement"), ("frame-home", 1)}
        for write in writes
    )
    assert all(not write.args[2].borrows_binding for write in writes)
    # __class__ travels as the closure tuple's cell, not molt_cell_get(cell)'s
    # current class object. Subsequent cell replacement must remain visible.
    class_cell = frames[0].class_cell
    producer = next(op for op in ops if op.result == class_cell)
    assert class_cell.type_hint == "cell"
    assert producer.kind == "INDEX"
    assert producer.args[0].name == "__molt_closure__"
    assert not any(op.result == producer.args[0] and op.kind == "INDEX" for op in ops)


def test_class_namespace_prefix_suffix_and_both_exits_use_correct_frame_owner():
    generator, _ = compile_source(
        "class Subject:\n"
        "    def method(receiver, meta):\n"
        "        try:\n"
        "            class Inner(metaclass=meta):\n"
        "                marker = 1\n"
        "                def witness(self):\n"
        "                    return __class__\n"
        "        except Exception:\n"
        "            return super()\n"
        "        return super()\n"
    )
    ops = super_consumer_ops(generator)
    keys = {op.result.name: op.args[0] for op in ops if op.kind == "CONST_STR"}
    expected = {
        "__module__",
        "__qualname__",
        "__firstlineno__",
        "marker",
        "witness",
        "__static_attributes__",
        "__classcell__",
    }
    writes = [
        (index, keys.get(op.args[1].name))
        for index, op in enumerate(ops)
        if op.kind == "STORE_INDEX"
        and len(op.args) == 3
        and isinstance(op.args[1], MoltValue)
        and keys.get(op.args[1].name) in expected
    ]
    assert {key for _, key in writes} == expected
    cleanup_targets = set()
    for index, _ in writes:
        assert frame_before(ops, index).kind == 0
        check = ops[index + 1]
        assert check.kind == "CHECK_EXCEPTION"
        cleanup_targets.add(check.args[0])
    assert len(cleanup_targets) == 1
    cleanup_label = cleanup_targets.pop()
    cleanup_index = next(
        index
        for index, op in enumerate(ops)
        if op.kind == "LABEL" and op.args[0] == cleanup_label
    )
    frames = publications(ops)
    normal_restore = next(
        frame for frame in frames if writes[-1][0] < frame.index < cleanup_index
    )
    exceptional_restore = next(frame for frame in frames if frame.index > cleanup_index)
    # Both exits restore the method's own argument zero: its first home.
    assert_home_argument_zero(ops, normal_restore)
    assert_home_argument_zero(ops, exceptional_restore)
    assert ops[normal_restore.index + 1].kind == "JUMP"
    assert ops[exceptional_restore.index + 1].kind == "JUMP"
    assert ops[normal_restore.index + 1].args != ops[exceptional_restore.index + 1].args
    for consumer in builtin_calls(ops):
        assert frame_before(ops, consumer).kind == 3


def assert_resume_frame_ownership(ops: list[MoltOp], required_boundaries: set[str]):
    frames = publications(ops)
    entry_owner = storage_owner(ops, frames[0].argument)
    assert entry_owner[0] == "task"
    observed = set()
    for index, op in enumerate(ops):
        if op.kind not in required_boundaries:
            continue
        observed.add(op.kind)
        frame = next(frame for frame in frames if frame.index > index)
        assert frame.kind == 1
        assert storage_owner(ops, frame.argument) == entry_owner
        # Only reconstruct the two operands between a resume boundary and its
        # publication. In particular no resumed body call or release can run
        # with the invocation's default NoArg context.
        assert all(
            item.kind in {"CONST_NONE", "CONST", "LOAD_CLOSURE", "LOAD_VAR", "INDEX"}
            for item in ops[index + 1 : frame.index]
        )
    assert observed == required_boundaries


@pytest.mark.parametrize(
    ("prefix", "body", "required_boundaries"),
    [
        ("", "yield receiver\nreturn super()", {"STATE_LABEL"}),
        ("async ", "await values\nreturn super()", {"STATE_LABEL", "STATE_TRANSITION"}),
    ],
)
def test_every_source_resume_entry_republishes_live_task_storage_before_continuing(
    prefix, body, required_boundaries
):
    source = f"class Subject:\n    {prefix}def method(receiver, values):\n" + "".join(
        f"        {line}\n" for line in body.splitlines()
    )
    generator, _ = compile_source(source)
    assert_resume_frame_ownership(super_consumer_ops(generator), required_boundaries)


@pytest.mark.parametrize("send_first", [False, True])
@pytest.mark.parametrize("bound", [False, True])
def test_channel_names_retain_live_calls_without_implicit_suspension(send_first, bound):
    body = "molt_chan_recv(values)\nreturn super()"
    if send_first:
        body = "molt_chan_send(values, receiver)\n" + body
    source = "class Subject:\n    async def method(receiver, values):\n" + "".join(
        f"        {line}\n" for line in body.splitlines()
    )
    if bound:
        source = (
            "from _intrinsics import require_intrinsic\n"
            "molt_chan_send = require_intrinsic('molt_chan_send', globals())\n"
            "molt_chan_recv = require_intrinsic('molt_chan_recv', globals())\n"
        ) + source
    generator, ir = compile_source(source)
    ((function_name, function),) = [
        (name, function)
        for name, function in generator.funcs_map.items()
        if builtin_calls(function["ops"], "molt_chan_recv")
    ]
    ops = function["ops"]
    serialized_ops = next(
        function["ops"]
        for function in ir["functions"]
        if function["name"] == function_name
    )
    plan = function["stateful_frame_plan"]
    expected_arguments = [
        ("molt_chan_recv", ("values",)),
    ]
    if send_first:
        expected_arguments.insert(0, ("molt_chan_send", ("values", "receiver")))
    source_slots = {
        "receiver": plan.async_locals_base,
        "values": plan.async_locals_base + 8,
    }
    calls = []
    for name, arguments in expected_arguments:
        (index,) = builtin_calls(ops, name)
        calls.append(index)
        call = ops[index]
        assert call.kind == "CALL_FUNC" and len(call.args) == len(arguments) + 1
        callee = next(op for op in ops[:index] if op.result == call.args[0])
        assert callee.kind == "MODULE_GET_GLOBAL"
        assert any(
            op["kind"] == "call_func"
            and op["args"] == [value.name for value in call.args]
            and op.get("out") == call.result.name
            for op in serialized_ops
        )
        for value, source_name in zip(call.args[1:], arguments, strict=True):
            assert storage_owner(ops, value) == (
                "task",
                "self",
                source_slots[source_name],
            )
        assert frame_before(ops, index).kind == 1
    assert calls == sorted(calls)
    assert not any(
        op.kind
        in {"STATE_LABEL", "STATE_TRANSITION", "CHAN_SEND_YIELD", "CHAN_RECV_YIELD"}
        for op in ops
    )


def annotation_evaluator_tree(kind, *, in_class):
    source = {
        "alias": "type Alias = super()",
        "bound": "def function[T: super()](): pass",
        "constraints": "def function[T: (super(), int)](): pass",
        "default": "def function[T](): pass",
        "function": "def function(value: super()): pass",
        "variable": "value: super()",
    }[kind]
    if in_class:
        source = "class Owner:\n    " + source
    tree = ast.parse(source)
    if kind == "default":
        owner = tree.body[0].body[0] if in_class else tree.body[0]
        owner.type_params[0].default_value = ast.parse("super()", mode="eval").body
    return tree


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize("in_class", [False, True])
@pytest.mark.parametrize(
    "kind", ["alias", "bound", "constraints", "default", "function", "variable"]
)
def test_annotation_evaluator_publishes_versioned_argument_and_real_class_cell(
    target, in_class, kind
):
    if kind in {"function", "variable"} and target < (3, 14):
        pytest.skip("ordinary annotations use eager evaluation before Python 3.14")
    if kind == "default" and target < (3, 13):
        pytest.skip("type parameter defaults require Python 3.13")
    tree = annotation_evaluator_tree(kind, in_class=in_class)
    generator = SimpleTIRGenerator(target_python=target)
    generator.visit(tree)
    ir = generator.to_json()
    verification = verify_frontend_tir(ir)
    assert verification.ok, verification.errors
    evaluators = [
        (name, fn["ops"])
        for name, fn in generator.funcs_map.items()
        if "__annotate__" in name
    ]
    assert len(evaluators) == 1
    name, ops = evaluators[0]
    frames = publications(ops)
    assert len(frames) == 1
    frame = frames[0]
    definitions = {op.result.name: op for op in ops}
    if target >= (3, 14):
        # PEP 649/749: `format` is the evaluator's first code slot.
        assert_home_argument_zero(ops, frame)
        (store,) = home_stores(ops, 0)
        assert ops[store].args[0].name == "format"
    else:
        assert frame.kind == 0
        assert definitions[frame.argument.name].kind == "CONST_NONE"
    body_start = next(index for index, op in enumerate(ops) if op.kind == "DICT_NEW")
    assert frame.index < body_start
    if not in_class:
        assert definitions[frame.class_cell.name].kind == "CONST_NONE"
        return
    # The evaluator's context uses the exact cell delivered to class creation,
    # not a namespace value, receiver, transport tuple, or loaded class object.
    class_cell_load = definitions[frame.class_cell.name]
    assert class_cell_load.kind == "INDEX"
    assert class_cell_load.args[0].name == "__molt_closure__"
    capture_index = definitions[class_cell_load.args[1].name].args[0]
    main_ops = generator.funcs_map["molt_main"]["ops"]
    main_definitions = {op.result.name: op for op in main_ops}
    constructor = next(
        op for op in main_ops if op.kind == "FUNC_NEW_CLOSURE" and op.args[0] == name
    )
    captures = main_definitions[constructor.args[2].name]
    assert captures.kind == "TUPLE_NEW"
    transported_cell = captures.args[capture_index]

    def is_class_cell_key(value):
        producer = (
            main_definitions.get(value.name) if isinstance(value, MoltValue) else None
        )
        return (
            producer is not None
            and producer.kind == "CONST_STR"
            and producer.args == ["__classcell__"]
        )

    class_definitions = [op for op in main_ops if op.kind == "CLASS_DEF"]
    if class_definitions:
        (class_definition,) = class_definitions
        (published_cell,) = [
            class_definition.args[index + 1]
            for index, arg in enumerate(class_definition.args[:-1])
            if is_class_cell_key(arg)
        ]
    else:
        # Type-alias creation makes class construction dynamic. Follow the
        # actual namespace through argument assembly to its metaclass consumer.
        ((store_index, store),) = [
            (index, op)
            for index, op in enumerate(main_ops)
            if op.kind == "STORE_INDEX" and is_class_cell_key(op.args[1])
        ]
        namespace, _key, published_cell = store.args
        ((push_index, push),) = [
            (index, op)
            for index, op in enumerate(main_ops)
            if op.kind == "CALLARGS_PUSH_POS" and op.args[1] == namespace
        ]
        ((call_index, call),) = [
            (index, op)
            for index, op in enumerate(main_ops)
            if op.kind == "CALL_BIND" and op.args[1] == push.args[0]
        ]
        positional = [
            op.args[1]
            for op in main_ops[:call_index]
            if op.kind == "CALLARGS_PUSH_POS" and op.args[0] == call.args[1]
        ]
        assert len(positional) == 3 and positional[2] == namespace
        assert store_index < push_index < call_index
    assert transported_cell == published_cell


@pytest.mark.parametrize(
    "kind", ["alias", "bound", "constraints", "default", "function", "variable"]
)
def test_annotation_dependency_authority_requests_implicit_class_cell(kind):
    from molt.compiler_analysis.python_lexical_scope import PythonDependencyAuthority

    tree = annotation_evaluator_tree(kind, in_class=True)
    authority = PythonDependencyAuthority(
        eager_annotations=False, future_annotations=False
    )
    assert authority.summary(tree.body[0]).class_cell_required


@pytest.mark.parametrize("scope", ["module", "closure", "class"])
@pytest.mark.parametrize("kind", ["alias", "function"])
@pytest.mark.parametrize(
    "expression", ["(format, super())", "[(format, super()) for format in (42,)]"]
)
def test_annotation_format_transport_is_not_a_source_binding(scope, kind, expression):
    definition = (
        f"type Alias = {expression}"
        if kind == "alias"
        else f"def function(value: {expression}): pass"
    )
    source = {
        "module": f"format = 42\n{definition}\n",
        "closure": f"def factory(format):\n    {definition}\n",
        "class": f"class Owner:\n    format = 42\n    {definition}\n",
    }[scope]
    generator, _ = compile_source(source)
    name, function = next(
        (name, fn) for name, fn in generator.funcs_map.items() if "__annotate__" in name
    )
    ops = function["ops"]
    frames = publications(ops)
    assert len(frames) == 1
    frame = frames[0]
    assert_home_argument_zero(ops, frame)
    # The evaluator's own `format` parameter binds code slot 0 at entry.
    parameter = ops[home_stores(ops, 0)[0]]
    assert parameter.args[0] == MoltValue("format", type_hint="Any")
    # Even a class-owned "format" descriptor/namespace hook is a body lookup,
    # never an operand of the mandatory entry publication.
    assert all(
        index > frame.index for index in runtime_calls(ops, "molt_namespace_get")
    )
    pairs = [op for op in ops if op.kind == "TUPLE_NEW" and len(op.args) == 2]
    assert pairs
    assert all(op.args[0].name not in {"format", parameter.result.name} for op in pairs)


def test_source_argument_named_like_closure_transport_keeps_its_binding():
    generator, _ = compile_source(
        "class Owner:\n    def method(__molt_closure__):\n        return super()\n"
    )
    ops = super_consumer_ops(generator)
    frame = publications(ops)[0]
    assert_home_argument_zero(ops, frame)
    # Code slot 0 binds the source argument, never the closure transport.
    parameter = ops[home_stores(ops, 0)[0]]
    assert parameter.args[0].name != "__molt_closure__"
    assert parameter.args[0] != frame.class_cell


def test_explicit_evaluator_argument_identity_resets_and_restores():
    generator = SimpleTIRGenerator()
    argument = MoltValue("format", type_hint="int")
    slots = CodeSlotDeclaration(("format",), ("format",), (), ())
    generator.start_function(
        "evaluator", params=["format"], python_first_arg=argument, code_slots=slots
    )
    saved = generator._capture_function_state()
    generator.start_function("other", params=["format"], code_slots=slots)
    assert generator.current_python_first_arg is None
    generator._restore_function_state(saved)
    assert generator.current_python_first_arg is argument
    assert generator._load_python_first_arg() is argument


@pytest.mark.parametrize(
    "source", ["def function(value: int): pass", "type Alias = int"]
)
def test_evaluator_format_validation_uses_generic_comparison_and_exact_false(source):
    generator, ir = compile_source(source)
    name, function = next(
        (name, fn) for name, fn in generator.funcs_map.items() if "__annotate__" in name
    )
    ops = function["ops"]
    frame = publications(ops)[0]
    # The guard reads the evaluator's `format` binding: code slot 0's view.
    argument = ops[home_stores(ops, 0)[0]].result
    comparisons = [
        (index, op)
        for index, op in enumerate(ops)
        if op.kind == "GT" and op.args[0] == argument
    ]
    assert len(comparisons) == 1
    index, comparison = comparisons[0]
    assert argument.type_hint == comparison.result.type_hint == "Any"
    assert frame.index < index
    limit = next(op for op in ops if op.result == comparison.args[1])
    assert limit.kind == "CONST" and limit.args == [2]
    identities = [
        op for op in ops if op.kind == "IS" and op.args[0] == comparison.result
    ]
    assert len(identities) == 1
    false_value = next(op for op in ops if op.result == identities[0].args[1])
    assert false_value.kind == "CONST_BOOL" and false_value.args == [False]
    branch = next(op for op in ops if op.kind == "IF")
    assert branch.args == [identities[0].result]
    assert ops[index + 1].kind == "CHECK_EXCEPTION"
    assert not any(op.kind == "BOOL" and op.args == [comparison.result] for op in ops)
    assert not any(op.kind in {"EQ", "NE"} and argument in op.args for op in ops)
    serialized = next(fn for fn in ir["functions"] if fn["name"] == name)
    guards = [
        op
        for op in serialized["ops"]
        if op["kind"] == "gt" and op.get("out") == comparison.result.name
    ]
    assert len(guards) == 1
    assert not guards[0].get("fast_int") and not guards[0].get("fast_float")


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "source",
    [
        "def outer(value):\n    return lambda: value\n",
        "def outer(value):\n    def inner():\n        return value\n    return inner\n",
        "class Base:\n    def method(self):\n        return 1\n"
        "class Derived(Base):\n    def method(self):\n        return super().method()\n",
        "def outer(value):\n    def inner():\n        yield value\n    return inner\n",
        "def outer(value):\n    async def inner():\n        return value\n    return inner\n",
    ],
)
def test_compiled_code_slot_publishes_complete_function_metadata(source, target):
    # Publication caches the runtime plan from the code object's final lexical
    # layout and execution flags. A free-only closure exposes an early publish:
    # its provisional co_varnames has no slots, but its final co_freevars does.
    generator, _ = compile_source(source, target)
    checked = 0
    for function in generator.funcs_map.values():
        ops = function["ops"]
        for position, op in enumerate(ops):
            if op.kind != "CODE_SLOT_SET":
                continue
            metadata = [
                index
                for index, candidate in enumerate(ops)
                if candidate.kind == "CALL"
                and candidate.args[0] == "molt_function_init_metadata_packed"
                and candidate.args[3] == op.args[0]
            ]
            if metadata:
                assert len(metadata) == 1
                assert metadata[0] < position
                checked += 1
    assert checked >= 2


@pytest.mark.parametrize("target", [(3, 12), (3, 13), (3, 14)])
@pytest.mark.parametrize(
    "source",
    [
        "def probe():\n    yield 1\n",
        "def probe():\n    yield 1\n    return 2\n",
        "probe = lambda: (yield 1)\n",
        "probe = (value for value in (1, 2))\n",
        "async def probe():\n    yield 1\n",
        "class Owner:\n    def probe(self):\n        yield 1\n",
        "class Owner:\n    async def probe(self):\n        yield 1\n",
    ],
)
def test_stateful_completion_is_owned_by_the_runtime(source, target):
    from molt.frontend._types import GEN_CLOSED_OFFSET

    # Publishing this flag in generated code skips the runtime's single
    # terminal transition, which clears or retires the activation's bindings.
    generator, _ = compile_source(source, target)
    assert any(
        "stateful_frame_plan" in function for function in generator.funcs_map.values()
    )
    for function in generator.funcs_map.values():
        assert not any(
            op.kind == "STORE_CLOSURE" and op.args[1] == GEN_CLOSED_OFFSET
            for op in function["ops"]
        )
