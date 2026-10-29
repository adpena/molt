from __future__ import annotations

import textwrap
from pathlib import Path

import pytest

from molt.frontend import MoltOp, SimpleTIRGenerator
from molt.frontend._types import MoltValue
from molt.frontend.lowering.op_kinds_generated import (
    FRONTEND_WIRE_KIND,
    FRONTEND_REGISTERED_KINDS,
    SIMPLEIR_BACKEND_PRIVATE_KINDS,
    SIMPLEIR_REGISTERED_KINDS,
    frontend_wire_kind,
    validate_serialized_kind,
)
from tools.op_kinds.census import FrontendCensus, frontend_census
from tools.op_kinds.errors import OpKindTableError
from tools.op_kinds.registration import validate_registration


ROOT = Path(__file__).resolve().parents[1]


@pytest.fixture(scope="module")
def census() -> FrontendCensus:
    return frontend_census(ROOT)


def test_every_frontend_kind_expression_resolves(census: FrontendCensus) -> None:
    assert len(census.produced) > 100, "census must discover MoltOp emission sites"
    assert not census.unresolved_producers, "unresolved MoltOp kinds:\n" + "\n".join(
        str(site) for site in census.unresolved_producers
    )


def test_every_spelled_frontend_kind_is_produced(census: FrontendCensus) -> None:
    # A literal that spells a registered kind nothing builds is a consumer left
    # behind when its producer was removed (HF-197): a dead serializer arm, a
    # set member, a table key or a registry override. Delete it with the
    # producer, or restore the producer.
    stale = [
        f"{site}: {kind}"
        for kind, sites in sorted(census.mentioned.items())
        if kind in FRONTEND_REGISTERED_KINDS and kind not in census.produced
        for site in sites
    ]
    assert not stale, "frontend kinds spelled but never produced:\n" + "\n".join(stale)


def test_direct_wire_mapping_matches_registry() -> None:
    from tools.op_kinds import load_table
    from tools.op_kinds.registration import (
        backend_private_kinds,
        frontend_kinds_for_wire_kind,
        registered_simpleir_kinds,
    )

    data = load_table()
    expected = {
        frontend: wire
        for wire in registered_simpleir_kinds(data) - backend_private_kinds(data).keys()
        for frontend in frontend_kinds_for_wire_kind(data, wire)
    }
    for row in data["kind"]:
        if row.get("group") == "gpu":
            expected.update(
                {wire: wire for wire in (row["canonical"], *row.get("aliases", []))}
            )
    assert FRONTEND_WIRE_KIND == expected
    assert {kind: frontend_wire_kind(kind) for kind in FRONTEND_WIRE_KIND} == expected


@pytest.mark.parametrize("kind", ["ADD", "INPLACE_ADD", "DIV", "FLOORDIV", "MUL"])
def test_serializer_uses_registry_wire_names_in_json_and_messagepack(kind) -> None:
    import json
    import msgpack

    ops = SimpleTIRGenerator().map_ops_to_json(
        [
            MoltOp(
                kind=kind,
                args=[MoltValue("left"), MoltValue("right")],
                result=MoltValue("out"),
            )
        ],
        run_midend=False,
    )
    assert ops[0]["kind"] == FRONTEND_WIRE_KIND[kind]
    assert {op["kind"] for op in ops} <= SIMPLEIR_REGISTERED_KINDS
    assert json.loads(json.dumps(ops)) == ops
    assert msgpack.unpackb(msgpack.packb(ops), raw=False) == ops


def test_census_counts_named_kind_consumers(tmp_path: Path) -> None:
    census = _census_of(
        tmp_path,
        """
        def visit(op):
            if op.kind == FrontendKinds.CONST:
                return op
        """,
    )
    assert "CONST" in census.mentioned
    assert not census.produced


def test_raise_has_one_producer_that_always_leaves(census: FrontendCensus) -> None:
    # RAISE only sets the pending exception. `_emit_raise` pairs it with the
    # exit to the nearest handler, so any other producer could let operations
    # after a raise run with the exception pending (HF-213).
    producers = {site.function for site in census.produced["RAISE"]}
    assert producers == {"_emit_raise"}, census.produced["RAISE"]


def _census_of(tmp_path: Path, source: str) -> FrontendCensus:
    package = tmp_path / "src/molt/frontend"
    package.mkdir(parents=True)
    (package / "module.py").write_text(textwrap.dedent(source), encoding="utf-8")
    return frontend_census(tmp_path)


def test_census_resolves_literal_conditional_and_qualified_constructors(
    tmp_path: Path,
) -> None:
    census = _census_of(
        tmp_path,
        """
        def emit(frontend, flag):
            MoltOp(kind="ALPHA", args=[])
            frontend.MoltOp(
                kind="BETA" if flag else "GAMMA",
                args=[],
            )
            MoltOp("DELTA", [])
            MoltOp(kind=op.kind, args=[])
        """,
    )
    assert set(census.produced) == {"ALPHA", "BETA", "GAMMA", "DELTA"}
    assert not census.unresolved_producers


def test_census_resolves_parameters_through_calls_partials_and_tables(
    tmp_path: Path,
) -> None:
    census = _census_of(
        tmp_path,
        """
        from functools import partial

        SPLIT = {"str": "SPLIT_STR", "bytes": "SPLIT_BYTES"}


        class Emitter:
            HOMES = {"cell": "HOME_CELL"}

            def _emit(self, kind, value):
                return MoltOp(kind=kind, args=[value])

            def _choose(self, name):
                return SPLIT[name]

            def visit(self, value, name):
                self._emit("RETURN", value)
                self._emit(self._choose(name), value)
                restore = partial(self._emit, kind=self.HOMES["cell"])
                kinds = {key: "COMPREHENDED" for key in value}
                self._emit(kinds[name], value)
                self._emit(SPLIT.get(name, "SPLIT_OTHER"), value)
                return restore
        """,
    )
    assert set(census.produced) == {
        "RETURN",
        "SPLIT_STR",
        "SPLIT_BYTES",
        "HOME_CELL",
        "COMPREHENDED",
        "SPLIT_OTHER",
    }
    assert not census.unresolved_producers


@pytest.mark.parametrize(
    ("source", "reason"),
    [
        ('MoltOp(kind=f"{name.upper()}_NEW")', "JoinedStr"),
        (
            """
            def emit(name):
                kind = "SPLIT"
                kind += "_MAX"
                MoltOp(kind=kind)
            """,
            "AugAssign",
        ),
        (
            """
            def _emit(kind):
                MoltOp(kind=kind)

            handlers = [_emit]
            """,
            "referenced as a value",
        ),
        (
            """
            def _emit(kind):
                MoltOp(kind=kind)
            """,
            "no direct caller",
        ),
        (
            """
            from functools import partial

            def _emit(value, kind):
                MoltOp(kind=kind)

            later = partial(_emit, 1)
            """,
            "leaves kind unbound",
        ),
    ],
)
def test_census_rejects_kinds_it_cannot_prove(
    tmp_path: Path, source: str, reason: str
) -> None:
    census = _census_of(tmp_path, source)
    assert not census.produced
    (site,) = census.unresolved_producers
    assert reason in str(site)


def test_census_records_every_identifier_literal_as_a_mention(
    tmp_path: Path,
) -> None:
    census = _census_of(
        tmp_path,
        """
        _CONSTANTS = frozenset({"CONST", "CONST_FLOAT"})
        LANES = {"ADD": "add"}

        def visit(op, kind):
            if op.kind == "SUB" or kind in {"MUL"}:
                return "not an identifier"
        """,
    )
    assert set(census.mentioned) == {
        "CONST",
        "CONST_FLOAT",
        "ADD",
        "add",
        "SUB",
        "MUL",
    }
    assert not census.produced


@pytest.mark.parametrize("run_midend", [False, True])
@pytest.mark.parametrize("kind", ["CONST_MISSING", "NEW_UNREGISTERED_OP"])
@pytest.mark.parametrize("has_result", [False, True])
def test_serialization_rejects_unknown_kinds_before_optimization(
    run_midend: bool, kind: str, has_result: bool
) -> None:
    operation = MoltOp(
        kind=kind,
        args=[],
        result=MoltValue("unused_result") if has_result else MoltValue("none"),
    )
    with pytest.raises(ValueError) as error:
        SimpleTIRGenerator().map_ops_to_json(
            [operation], function_name="broken_function", run_midend=run_midend
        )
    assert kind in str(error.value)
    assert "broken_function" in str(error.value)
    assert "unregistered frontend op kind" in str(error.value)


def test_registered_kind_cannot_be_dropped_by_missing_serializer(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    generator = SimpleTIRGenerator()
    for method in (
        "_serialize_basic_op",
        "_serialize_function_op",
        "_serialize_exception_op",
        "_serialize_object_attr_op",
        "_serialize_collection_op",
        "_serialize_loop_string_async_op",
    ):
        monkeypatch.setattr(generator, method, lambda operation, context: False)
    with pytest.raises(ValueError, match="broken_function.*no serializer.*CONST"):
        generator.map_ops_to_json(
            [MoltOp(kind="CONST", args=[1], result=MoltValue("result"))],
            function_name="broken_function",
            run_midend=False,
        )


def test_serializer_cannot_emit_unregistered_wire_kind(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    def serialize(operation, context):
        context.json_ops.append({"kind": "new_unregistered_wire_op"})
        return True

    generator = SimpleTIRGenerator()
    monkeypatch.setattr(generator, "_serialize_basic_op", serialize)
    with pytest.raises(
        ValueError,
        match="broken_function.*unregistered SimpleIR.*new_unregistered_wire_op",
    ):
        generator.map_ops_to_json(
            [MoltOp(kind="CONST", args=[1], result=MoltValue("result"))],
            function_name="broken_function",
            run_midend=False,
        )


def test_midend_cannot_introduce_unregistered_frontend_kind(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    generator = SimpleTIRGenerator()
    monkeypatch.setattr(
        generator,
        "_run_ir_midend_passes",
        lambda operations: [
            MoltOp(kind="MIDEND_TYPO", args=[], result=MoltValue("none"))
        ],
    )
    with pytest.raises(
        ValueError, match="broken_function.*unregistered frontend.*MIDEND_TYPO"
    ):
        generator.map_ops_to_json([], function_name="broken_function")


@pytest.mark.parametrize("preserved", [["cast", "cast"], [None], "cast"])
def test_preserved_registration_rejects_malformed_members(preserved) -> None:
    with pytest.raises(OpKindTableError, match="simpleir_preserved_kinds"):
        validate_registration({"simpleir_preserved_kinds": preserved})


def test_frontend_lowering_registration_requires_registered_wire_kind() -> None:
    with pytest.raises(OpKindTableError, match="unregistered wire kind"):
        validate_registration(
            {
                "frontend_lowering_kind": [{"kinds": ["TYPO"], "wire_kind": "typo"}],
            }
        )


@pytest.mark.parametrize(
    "rows",
    [
        [{"kind": "EQ", "wire_kind": "eq"}],
        [{"kinds": "EQ", "wire_kind": "eq"}],
        [{"kinds": [None], "wire_kind": "eq"}],
        [{"kinds": ["EQ", "EQ"], "wire_kind": "eq"}],
        [
            {"kinds": [], "wire_kind": "eq"},
            {"kinds": ["EQ"], "wire_kind": "eq"},
        ],
        [
            {"kinds": ["EQ"], "wire_kind": "eq"},
            {"kinds": ["EQ"], "wire_kind": "ne"},
        ],
    ],
)
def test_frontend_lowering_rejects_ambiguous_or_retired_projection(rows) -> None:
    with pytest.raises(OpKindTableError, match="frontend_lowering_kind"):
        validate_registration(
            {
                "kind": [{"canonical": "eq"}, {"canonical": "ne"}],
                "frontend_lowering_kind": rows,
            }
        )


@pytest.mark.parametrize("kind", sorted(SIMPLEIR_BACKEND_PRIVATE_KINDS))
def test_backend_private_kinds_cannot_escape_frontend_serialization(kind: str) -> None:
    assert kind in SIMPLEIR_REGISTERED_KINDS
    assert kind.upper() not in FRONTEND_REGISTERED_KINDS
    with pytest.raises(ValueError, match="backend-private SimpleIR op kind"):
        validate_serialized_kind(kind, "frontend_function")


@pytest.mark.parametrize(
    "rows",
    [
        "luau",
        [{}],
        [{"backend": "luau", "kinds": []}],
        [{"backend": "luau", "kinds": [None]}],
        [{"backend": "luau", "kinds": ["private", "private"]}],
        [{"backend": "luau", "kinds": ["cast"]}],
        [
            {"backend": "luau", "kinds": ["private"]},
            {"backend": "other", "kinds": ["private"]},
        ],
    ],
)
def test_backend_private_registration_rejects_malformed_or_shared_members(rows) -> None:
    with pytest.raises(OpKindTableError, match="simpleir_backend_private_kinds"):
        validate_registration(
            {
                "simpleir_preserved_kinds": ["cast"],
                "simpleir_backend_private_kinds": rows,
            }
        )


def test_frontend_lowering_cannot_claim_backend_private_operation() -> None:
    with pytest.raises(OpKindTableError, match="unregistered wire kind"):
        validate_registration(
            {
                "simpleir_backend_private_kinds": [
                    {"backend": "luau", "kinds": ["private"]}
                ],
                "frontend_lowering_kind": [
                    {"kinds": ["PRIVATE"], "wire_kind": "private"}
                ],
            }
        )
