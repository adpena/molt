from __future__ import annotations

import re
import textwrap
from pathlib import Path

import pytest

from molt.frontend import MoltOp, SimpleTIRGenerator
from molt.frontend._types import MoltValue
from molt.frontend.lowering.op_kinds_generated import (
    FRONTEND_REGISTERED_KINDS,
    SIMPLEIR_BACKEND_PRIVATE_KINDS,
    SIMPLEIR_REGISTERED_KINDS,
    validate_serialized_kind,
)
from tools.audit_op_kinds import (
    _rust_function_source,
    extract_frontend_kinds,
    extract_match_arms,
)
from tools.gen_op_kinds import load_table
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


def test_every_frontend_produced_kind_is_registered(census: FrontendCensus) -> None:
    unregistered = [
        f"{site}: {kind}"
        for kind, sites in sorted(census.produced.items())
        if kind not in FRONTEND_REGISTERED_KINDS
        for site in sites
    ]
    assert not unregistered, "unregistered frontend op kinds:\n" + "\n".join(
        unregistered
    )


def test_every_frontend_kind_test_names_a_produced_kind(
    census: FrontendCensus,
) -> None:
    # A test for a kind that nothing builds is dead code: the producer was
    # removed and this consumer was left behind (HF-197). Delete it with the
    # producer, or restore the producer.
    stale = [
        f"{site}: {kind}"
        for kind, sites in sorted(census.consumed.items())
        if kind in FRONTEND_REGISTERED_KINDS and kind not in census.produced
        for site in sites
    ]
    assert not stale, "frontend tests for kinds nothing produces:\n" + "\n".join(stale)


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


def test_census_records_kind_tests_in_comparisons_and_named_tables(
    tmp_path: Path,
) -> None:
    census = _census_of(
        tmp_path,
        """
        _CONSTANTS = frozenset({"CONST", "CONST_FLOAT"})

        def visit(op):
            if op.kind == "ADD":
                pass
            elif op.kind in {"SUB", "MUL"}:
                pass
            elif op.kind not in _CONSTANTS:
                pass
        """,
    )
    assert set(census.consumed) == {"ADD", "SUB", "MUL", "CONST", "CONST_FLOAT"}
    assert not census.produced


def test_every_resolved_serialized_kind_is_registered() -> None:
    emitted = extract_frontend_kinds(ROOT)
    assert not emitted.unresolved, emitted.unresolved
    assert emitted.all
    assert not (emitted.all - SIMPLEIR_REGISTERED_KINDS)


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
                "frontend_lowering_kind": [{"kind": "TYPO", "wire_kind": "typo"}],
            }
        )


@pytest.mark.parametrize("kind", sorted(SIMPLEIR_BACKEND_PRIVATE_KINDS))
def test_backend_private_kinds_cannot_escape_frontend_serialization(kind: str) -> None:
    assert kind in SIMPLEIR_REGISTERED_KINDS
    assert kind.upper() not in FRONTEND_REGISTERED_KINDS
    with pytest.raises(ValueError, match="backend-private SimpleIR op kind"):
        validate_serialized_kind(kind, "frontend_function")


def test_luau_dispatch_and_synthetic_operations_are_registered() -> None:
    # Use actual dispatch arms, excluding nested builtin-name/operator matches.
    # This checks the consumer vocabulary independently of registry projection.
    root = ROOT / "runtime/molt-backend-luau/src"
    discovered: set[str] = set()
    for path in sorted((root / "luau").glob("op_*.rs")):
        source = path.read_text(encoding="utf-8")
        for function in re.findall(r"fn (emit_\w+op)\(", source):
            # Dispatch wrappers/helpers do not own a kind match.
            body = _rust_function_source(path, function)
            if "match op.kind.as_str()" in body:
                discovered.update(
                    extract_match_arms(path, function, "match op.kind.as_str()")
                )
    rewrites = (root / "luau_backend/ir_rewrites.rs").read_text(encoding="utf-8")
    discovered.update(re.findall(r'kind: "([a-z_]+)"\.into\(\)', rewrites))
    assert {"pcall_wrap_begin", "string_splitlines", "string_rfind_slice"} <= discovered
    assert not discovered - SIMPLEIR_REGISTERED_KINDS
    private = load_table()["simpleir_backend_private_kinds"]
    assert {row["backend"] for row in private} == {"luau"}
    assert not SIMPLEIR_BACKEND_PRIVATE_KINDS - discovered


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
                "frontend_lowering_kind": [{"kind": "PRIVATE", "wire_kind": "private"}],
            }
        )
