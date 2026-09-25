"""Adversarial boundaries of the canonical streaming Python source identity."""

from __future__ import annotations

import ast
import hashlib
import os
from pathlib import Path
import struct
import sys

import pytest

from molt.compiler_analysis.python_source_keys import (
    _AST_BUFFER_SIZE,
    _AstDigestStream,
    _PythonAstDigestAdmission,
    python_ast_digest,
)
from tests.native_process_guard import run_native_test_process


class _SyntheticNode(ast.AST):
    _fields = ("value",)
    _attributes = ()


@pytest.mark.parametrize("schema", ["_fields", "_attributes"])
def test_instance_schema_overrides_and_absent_schema_members_are_identity(
    schema: str,
) -> None:
    tree = _SyntheticNode(value=1)
    baseline = python_ast_digest(tree)
    original_schema = getattr(tree, schema)
    setattr(tree, schema, (*original_schema, "missing"))
    declared_missing = python_ast_digest(tree)
    assert declared_missing != baseline
    tree.missing = None
    assert python_ast_digest(tree) != declared_missing
    del tree.missing
    assert python_ast_digest(tree) == declared_missing
    setattr(tree, schema, original_schema)
    assert python_ast_digest(tree) == baseline


def test_schema_order_and_field_attribute_partition_are_identity() -> None:
    tree = _SyntheticNode()
    tree._fields = ("first", "second")
    ordered = python_ast_digest(tree)
    tree._fields = ("second", "first")
    assert python_ast_digest(tree) != ordered
    tree._fields = ()
    tree._attributes = ("first", "second")
    assert python_ast_digest(tree) != ordered


@pytest.mark.parametrize(
    "module,qualname",
    [("other.module", "Node"), ("identity.module", "Outer.Node")],
)
def test_ast_class_module_and_qualified_name_are_identity(
    module: str, qualname: str
) -> None:
    def node_type(module_name: str, qualified_name: str) -> type[ast.AST]:
        return type(
            "Node",
            (ast.AST,),
            {
                "__module__": module_name,
                "__qualname__": qualified_name,
                "_fields": ("value",),
                "_attributes": (),
            },
        )

    first = node_type("identity.module", "Node")(value=1)
    second = node_type(module, qualname)(value=1)
    equivalent = node_type("identity.module", "Node")(value=1)
    assert python_ast_digest(first) != python_ast_digest(second)
    assert python_ast_digest(first) == python_ast_digest(equivalent)


@pytest.mark.parametrize(
    "left,right",
    [
        (["ab", "c"], ["a", "bc"]),
        (["a\0", "b"], ["a", "\0b"]),
        ([b"ab", b"c"], [b"a", b"bc"]),
        ([[], [1]], [[[]], 1]),
        ([1, 2], [(1, 2)]),
        (frozenset(("ab", "c")), frozenset(("a", "bc"))),
    ],
)
def test_stream_boundaries_cannot_alias_payload_or_nesting(
    left: object, right: object
) -> None:
    assert python_ast_digest(ast.Constant(value=left)) != python_ast_digest(
        ast.Constant(value=right)
    )


def test_arbitrary_precision_integer_sign_and_low_bits_are_identity() -> None:
    huge = 1 << 20000
    digests = {
        python_ast_digest(ast.Constant(value=value))
        for value in (huge, huge + 1, -huge, -huge - 1)
    }
    assert len(digests) == 4


def test_nan_payload_and_sign_bits_are_identity_for_float_and_complex() -> None:
    bit_patterns = (0x7FF8000000000001, 0x7FF8000000000002, 0xFFF8000000000001)
    values = [struct.unpack("!d", bits.to_bytes(8, "big"))[0] for bits in bit_patterns]
    for wrap in (lambda value: value, lambda value: complex(0.0, value)):
        digests = [
            python_ast_digest(ast.Constant(value=wrap(value))) for value in values
        ]
        assert len(set(digests)) == len(bit_patterns)
        for bits, digest in zip(bit_patterns, digests, strict=True):
            recreated = struct.unpack("!d", bits.to_bytes(8, "big"))[0]
            assert python_ast_digest(ast.Constant(value=wrap(recreated))) == digest


def test_lone_surrogates_preserve_exact_string_content_and_type() -> None:
    values = (
        "\ud800\0",
        "\ud801\0",
        "\ud800",
        "\ud800\0".encode("utf-8", "surrogatepass"),
    )
    assert len({python_ast_digest(ast.Constant(value=value)) for value in values}) == 4


@pytest.mark.parametrize(
    "kind", [int, float, complex, str, bytes, list, tuple, frozenset]
)
def test_scalar_and_container_subclasses_are_not_silently_coerced(kind: type) -> None:
    subclass = type("NonCanonicalValue", (kind,), {})
    with pytest.raises(TypeError, match="unsupported Python AST identity value"):
        python_ast_digest(ast.Constant(value=subclass()))


def test_unordered_container_keeps_ast_ancestors_in_active_cycle_path() -> None:
    tree = _SyntheticNode()
    tree.value = frozenset((tree,))
    with pytest.raises(ValueError, match="cyclic Python AST identity value"):
        python_ast_digest(tree)
    tree.value = 1
    assert python_ast_digest(tree) == python_ast_digest(_SyntheticNode(value=1))


@pytest.mark.parametrize("kind", [list, tuple, frozenset, _SyntheticNode])
def test_deep_ordered_and_unordered_inputs_are_stack_safe(kind: type) -> None:
    def nested(leaf: int) -> ast.AST:
        value: object = leaf
        for _ in range(2048):
            value = kind(value=value) if kind is _SyntheticNode else kind((value,))
        return ast.Constant(value=value)

    first = python_ast_digest(nested(1))
    assert first == python_ast_digest(nested(1))
    assert first != python_ast_digest(nested(2))


def test_nested_unordered_members_are_independent_of_process_hash_seed() -> None:
    root = Path(__file__).resolve().parents[1]
    script = (
        "import ast, sys\n"
        f"sys.path.insert(0, {str(root / 'src')!r})\n"
        "from molt.compiler_analysis.python_source_keys import python_ast_digest\n"
        "members = frozenset((name, frozenset((name, name[::-1], b'bytes'))) "
        "for name in ('alpha', 'beta', 'gamma', 'delta', 'epsilon'))\n"
        "print(python_ast_digest(ast.Constant(value=members)))\n"
    )
    digests = []
    for seed in ("0", "17", "314159"):
        result = run_native_test_process(
            [sys.executable, "-c", script],
            cwd=root,
            env={**os.environ, "PYTHONHASHSEED": seed},
            capture_output=True,
            text=True,
            timeout=20,
        )
        assert result.returncode == 0, result.stderr
        digests.append(result.stdout.strip())
    assert len(digests[0]) == 64
    assert len(set(digests)) == 1


def test_admission_is_exact_tree_scoped_and_new_admission_sees_mutation() -> None:
    tree = ast.parse("value = 1\n")
    admission = _PythonAstDigestAdmission.for_tree(tree)
    captured = admission.digest
    assert _PythonAstDigestAdmission.for_tree(tree, admission) is admission
    equivalent = ast.parse("value = 1\n")
    assert python_ast_digest(equivalent) == captured
    with pytest.raises(ValueError, match="different tree"):
        _PythonAstDigestAdmission.for_tree(equivalent, admission)

    constant = next(node for node in ast.walk(tree) if isinstance(node, ast.Constant))
    constant.value = 2
    fresh = _PythonAstDigestAdmission.for_tree(tree)
    assert admission.digest == captured
    assert fresh.digest == python_ast_digest(tree) != captured
    constant.value = 1
    assert python_ast_digest(tree) == captured


def test_shared_subtree_crossing_unordered_member_boundary_has_value_identity() -> None:
    shared = _SyntheticNode(value=7)
    aliased = ast.Constant(value=[frozenset((shared,)), shared])
    copied = ast.Constant(
        value=[frozenset((_SyntheticNode(value=7),)), _SyntheticNode(value=7)]
    )
    assert python_ast_digest(aliased) == python_ast_digest(copied)


def test_stream_flush_boundaries_and_large_payloads_preserve_bytes() -> None:
    stream = _AstDigestStream(b"test-domain\0")
    expected = hashlib.sha256(b"test-domain\0")
    for size in (0, 1, 255, 256, _AST_BUFFER_SIZE - 1, _AST_BUFFER_SIZE, 100_000):
        payload = b"x" * size
        stream.framed(payload)
        expected.update(size.to_bytes(8, "big"))
        expected.update(payload)
        assert len(stream.buffer) < _AST_BUFFER_SIZE
    assert stream.finish() == expected.digest()


def test_machine_integer_boundaries_and_bigint_encodings_are_distinct() -> None:
    values = (-(1 << 63) - 1, -(1 << 63), -1, 0, 1, (1 << 63) - 1, 1 << 63)
    assert len({python_ast_digest(ast.Constant(value=n)) for n in values}) == len(
        values
    )


def test_instance_overrides_do_not_borrow_sibling_schema_memo() -> None:
    first = _SyntheticNode(value=1)
    second = _SyntheticNode(value=1)
    tree = ast.Constant(value=[first, second, first])
    baseline = python_ast_digest(tree)
    second._fields = ("value", "missing")
    changed = python_ast_digest(tree)
    assert changed != baseline
    second._fields = ("value",)
    assert python_ast_digest(tree) == baseline
    second._attributes = ("missing",)
    assert python_ast_digest(tree) not in (baseline, changed)


def test_v3_encoding_vector_requires_an_explicit_domain_change() -> None:
    tree = ast.Constant(
        value=(1, -2.5, "a", b"b", frozenset((1, "x"))),
        kind=None,
        lineno=1,
        col_offset=0,
        end_lineno=1,
        end_col_offset=2,
    )
    assert python_ast_digest(tree) == (
        "1828c5a661d05af091c09a08e762aeeb24651ecf32414589444313153990c491"
    )
