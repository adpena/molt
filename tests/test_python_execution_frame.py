"""Source-frame argument zero and PEP 709 storage ownership."""

import ast
from dataclasses import dataclass

import pytest

from molt.frontend import MoltOp, MoltValue, SimpleTIRGenerator
from molt.frontend._types import BUILTIN_TYPE_TAGS
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
    # Exercise the named dispatch without a module binding index as well as the
    # assembled frontend. Both paths must acquire the callee and evaluate args.
    generator = SimpleTIRGenerator(target_python=target)
    result = generator.visit(ast.parse(expression, mode="eval").body)
    assert result is not None
    assert any(
        op.kind in {"CALL_BIND", "CALL_INDIRECT"} for op in generator.current_ops
    )
    assert not runtime_calls(generator.current_ops, "molt_super_from_frame")
    compiled, _ = compile_source(f"def probe(): return {expression}\n", target)
    assert any(
        builtin_calls(function["ops"]) for function in compiled.funcs_map.values()
    )


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


def builtin_calls(ops: list[MoltOp], name: str = "super") -> list[int]:
    """Find direct and ordinary builtin consumers in these unshadowed fixtures."""
    producers = {op.result.name: op for op in ops}
    calls = []
    for index, op in enumerate(ops):
        if (
            name == "super"
            and op.kind == "CALL"
            and op.args[0] == "molt_super_from_frame"
        ):
            calls.append(index)
            continue
        if op.kind not in {"CALL_BIND", "CALL_INDIRECT"}:
            continue
        callee = producers.get(op.args[0].name)
        if callee is None:
            continue
        if callee.kind == "MODULE_GET_GLOBAL":
            name_op = producers[callee.args[1].name]
            if name_op.kind == "CONST_STR" and name_op.args == [name]:
                calls.append(index)
        elif callee.kind == "BUILTIN_TYPE":
            tag = producers[callee.args[0].name]
            if tag.kind == "CONST" and tag.args == [BUILTIN_TYPE_TAGS.get(name)]:
                calls.append(index)
    return calls


def publications(ops: list[MoltOp]) -> list[FramePublication]:
    producers = {op.result.name: op for op in ops}
    result = []
    for index in runtime_calls(ops, "molt_frame_context_set"):
        _, argument, kind, class_cell = ops[index].args
        assert isinstance(argument, MoltValue)
        assert isinstance(kind, MoltValue)
        assert isinstance(class_cell, MoltValue)
        kind_op = producers[kind.name]
        assert kind_op.kind == "CONST"
        assert kind_op.args[0] in (0, 1, 2)
        result.append(FramePublication(index, argument, kind_op.args[0], class_cell))
    assert result, "the executing function never published its semantic frame"
    return result


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
    if producer.kind == "LOAD_VAR":
        return ("local", producer.metadata["var"])
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
    generator, _ = compile_source(f"def function({signature}): return super()\n")
    ops = super_consumer_ops(generator)
    frame = publications(ops)[0]
    assert frame.kind == (0 if expected is None else 1)
    if expected is None:
        assert (
            next(op for op in ops if op.result == frame.argument).kind == "CONST_NONE"
        )
    else:
        assert storage_owner(ops, frame.argument)[-1] == expected


def test_source_frame_argument_is_reset_and_restored():
    generator = SimpleTIRGenerator()
    generator.start_function("outer", params=["receiver"], python_first_arg="receiver")
    generator.python_frame_context_active = True
    saved = generator._capture_function_state()
    generator.start_function("poll", params=["self"], compiler_params={"self"})
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
    inner = frame_before(ops, inner_call)
    outer = frame_before(ops, outer_call)
    entry = publications(ops)[0]
    assert inner.kind == outer.kind == entry.kind == 1
    assert storage_owner(ops, outer.argument) == storage_owner(ops, entry.argument)
    assert storage_owner(ops, inner.argument) != storage_owner(ops, entry.argument)
    # The shadow publication is the actual loop-target value, and its scoped
    # STORE_VAR/STORE_CLOSURE follows publication before releasing an old value.
    assert any(
        op.kind in {"STORE_VAR", "STORE_CLOSURE"} and inner.argument in op.args
        for op in ops[inner.index + 1 : inner_call]
    )


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


def test_arithmetic_generator_reduction_remains_fusible():
    _, ir = compile_source(
        "def total(values): return sum(item * item for item in values)\n"
    )
    assert not any("genexpr_" in fn["name"] for fn in ir["functions"])


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


def test_argument_replacement_and_deletion_publish_before_releasing_old_storage():
    generator, _ = compile_source(
        "def frame(receiver, replacement):\n"
        "    receiver = replacement\n"
        "    del receiver\n"
        "    return super()\n"
    )
    ops = super_consumer_ops(generator)
    frames = publications(ops)
    assert all(frame.kind == 1 for frame in frames)
    delete_index = next(
        index for index, op in enumerate(ops) if op.kind == "DELETE_VAR"
    )
    delete = ops[delete_index]
    deletion = frame_before(ops, delete_index)
    assert deletion.argument == delete.args[0]
    assert next(op for op in ops if op.result == deletion.argument).kind == "MISSING"
    replacements = [
        (index, op)
        for index, op in enumerate(ops[:delete_index])
        if op.kind == "STORE_VAR"
        and op.metadata.get("var") == "receiver"
        and op.args[0] != frames[0].argument
    ]
    assert replacements
    replacement_index, replacement = replacements[-1]
    publication = frame_before(ops, replacement_index)
    alias = next(op for op in ops if op.result == replacement.args[0])
    assert alias.kind == "BINDING_ALIAS"
    assert alias.args == [publication.argument]
    assert (
        frames[0].index
        < publication.index
        < replacement_index
        < deletion.index
        < delete_index
    )
    # The eventual runtime super consumer sees the missing marker, not the
    # original argument or a snapshot loaded before deletion.
    consumer = builtin_calls(ops)[0]
    assert frame_before(ops, consumer).argument == deletion.argument


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
    assert all(frame.kind == 2 for frame in frames)
    cell = frames[0].argument
    cell_producer = next(op for op in ops if op.result == cell)
    assert cell_producer.kind == "LIST_NEW"
    assert any(op.kind == "TUPLE_NEW" and cell in op.args for op in ops)
    assert any(op.kind == "STORE_INDEX" and op.args[0] == cell for op in ops)
    assert all(frame.argument == cell for frame in frames)
    # __class__ travels as the closure tuple's cell, not INDEX(cell, 0)'s
    # current class object. Subsequent cell replacement must remain visible.
    class_cell = frames[0].class_cell
    producer = next(op for op in ops if op.result == class_cell)
    assert class_cell.type_hint == "list"
    assert producer.kind == "INDEX"
    assert isinstance(producer.args[0], MoltValue)
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
    assert normal_restore.kind == exceptional_restore.kind == 1
    assert storage_owner(ops, normal_restore.argument) == storage_owner(
        ops, exceptional_restore.argument
    )
    assert ops[normal_restore.index + 1].kind == "JUMP"
    assert ops[exceptional_restore.index + 1].kind == "JUMP"
    assert ops[normal_restore.index + 1].args != ops[exceptional_restore.index + 1].args
    for consumer in builtin_calls(ops):
        assert frame_before(ops, consumer).kind == 1


@pytest.mark.parametrize(
    ("prefix", "body", "required_boundaries"),
    [
        ("", "yield receiver\nreturn super()", {"STATE_LABEL"}),
        ("async ", "await values\nreturn super()", {"STATE_LABEL", "STATE_TRANSITION"}),
        (
            "async ",
            "molt_chan_send(values, receiver)\nmolt_chan_recv(values)\nreturn super()",
            {"STATE_LABEL", "CHAN_SEND_YIELD"},
        ),
        (
            "async ",
            "molt_chan_recv(values)\nreturn super()",
            {"STATE_LABEL", "CHAN_RECV_YIELD"},
        ),
    ],
)
def test_every_resume_entry_republishes_live_task_storage_before_continuing(
    prefix, body, required_boundaries
):
    source = f"class Subject:\n    {prefix}def method(receiver, values):\n" + "".join(
        f"        {line}\n" for line in body.splitlines()
    )
    generator, _ = compile_source(source)
    ops = super_consumer_ops(generator)
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
    if "molt_chan_send" in body:
        # The send invalidates the next global callee's static identity. Its
        # discarded receive result must not erase the actual dynamic call.
        (receive,) = builtin_calls(ops, "molt_chan_recv")
        assert ops[receive].kind == "CALL_INDIRECT"
        assert frame_before(ops, receive).kind == 1


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
    assert frame.kind == (1 if target >= (3, 14) else 0)
    definitions = {op.result.name: op for op in ops}
    if target >= (3, 14):
        assert frame.argument.name == "format"
    else:
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
    assert frame.kind == 1
    assert frame.argument == MoltValue("format", type_hint="Any")
    # Even a class-owned "format" descriptor/namespace hook is a body lookup,
    # never an operand of the mandatory entry publication.
    assert all(
        index > frame.index for index in runtime_calls(ops, "molt_namespace_get")
    )
    pairs = [op for op in ops if op.kind == "TUPLE_NEW" and len(op.args) == 2]
    assert pairs
    assert all(op.args[0] != frame.argument for op in pairs)


def test_source_argument_named_like_closure_transport_keeps_its_binding():
    generator, _ = compile_source(
        "class Owner:\n    def method(__molt_closure__):\n        return super()\n"
    )
    ops = super_consumer_ops(generator)
    frame = publications(ops)[0]
    assert frame.kind == 1
    assert frame.argument.name != "__molt_closure__"
    assert frame.argument != frame.class_cell


def test_explicit_evaluator_argument_identity_resets_and_restores():
    generator = SimpleTIRGenerator()
    argument = MoltValue("format", type_hint="int")
    generator.start_function("evaluator", params=["format"], python_first_arg=argument)
    saved = generator._capture_function_state()
    generator.start_function("other", params=["format"])
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
    comparisons = [
        (index, op)
        for index, op in enumerate(ops)
        if op.kind == "GT" and op.args[0] == frame.argument
    ]
    assert len(comparisons) == 1
    index, comparison = comparisons[0]
    assert frame.argument.type_hint == comparison.result.type_hint == "Any"
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
    assert not any(op.kind in {"EQ", "NE"} and frame.argument in op.args for op in ops)
    serialized = next(fn for fn in ir["functions"] if fn["name"] == name)
    guards = [
        op
        for op in serialized["ops"]
        if op["kind"] == "gt" and op.get("out") == comparison.result.name
    ]
    assert len(guards) == 1
    assert not guards[0].get("fast_int") and not guards[0].get("fast_float")
