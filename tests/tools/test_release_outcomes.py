"""Independent graph and fail-closed controls for release outcome declarations."""

from __future__ import annotations

import copy
from dataclasses import FrozenInstanceError

import pytest

from tools import finding_status, release_outcomes


LEDGER = """## Open findings
| HF-1 | Producer | Evidence mentions HF-5; it declares no dependency. |
| HF-2 | Consumer | Evidence mentions native; it declares no proof family. |
| HF-3 | Left | observation |
| HF-4 | Right | observation |
| HF-5 | Join | observation |
| V1-1 (part) | Undeclared | observation |
## Fixed after the handoff
| HF-F6 (was HF-6) | Historical correction | recorded fix evidence |
"""
FAMILIES = ("native", "wasm")


def entry(key: str = "producer", findings: tuple[str, ...] = ("HF-1",), **changes):
    return {
        "id": key,
        "findings": list(findings),
        "depends_on": [],
        "proof_families": ["native"],
        "inputs": ["runtime/**/*.rs"],
        **changes,
    }


def parse(entries):
    return release_outcomes.parse_release_outcomes(
        entries, findings=finding_status.project(LEDGER), proof_families=FAMILIES
    )


def test_multi_finding_diamond_is_topological_and_retains_only_declared_edges():
    declarations = [
        entry("join", ("HF-5",), depends_on=["right", "left"]),
        entry("right", ("HF-4",), depends_on=["seed"]),
        entry("seed", ("HF-2", "HF-1"), proof_families=["wasm", "native"]),
        entry("left", ("HF-3",), depends_on=["seed"]),
    ]
    graph = parse(declarations)
    assert [outcome.id for outcome in graph.outcomes] == [
        "seed",
        "left",
        "right",
        "join",
    ]
    assert graph.outcomes[0].findings == ("HF-1", "HF-2")
    assert graph.outcomes[0].proof_families == ("native", "wasm")
    assert graph.outcomes[-1].depends_on == ("left", "right")
    assert sum(len(outcome.depends_on) for outcome in graph.outcomes) == 4
    assert graph.project() == parse(list(reversed(declarations))).project()
    assert graph.unplanned_findings == ("V1-1 (part)",)


def test_all_findings_remain_visible_and_graph_never_attests_execution_phases():
    graph = parse([entry()])
    assert graph.project()["findings"] == [
        {
            "key": "HF-1",
            "recorded_state": "open",
            "outcome_id": "producer",
            "planning": "PLANNED",
        },
        {
            "key": "HF-2",
            "recorded_state": "open",
            "outcome_id": None,
            "planning": "UNPLANNED",
        },
        {
            "key": "HF-3",
            "recorded_state": "open",
            "outcome_id": None,
            "planning": "UNPLANNED",
        },
        {
            "key": "HF-4",
            "recorded_state": "open",
            "outcome_id": None,
            "planning": "UNPLANNED",
        },
        {
            "key": "HF-5",
            "recorded_state": "open",
            "outcome_id": None,
            "planning": "UNPLANNED",
        },
        {
            "key": "HF-F6",
            "recorded_state": "fixed",
            "outcome_id": None,
            "planning": None,
        },
        {
            "key": "V1-1 (part)",
            "recorded_state": "open",
            "outcome_id": None,
            "planning": "UNPLANNED",
        },
    ]
    assert graph.unplanned_findings == ("HF-2", "HF-3", "HF-4", "HF-5", "V1-1 (part)")
    # Prose references in the ledger cannot manufacture edges or proof families.
    assert graph.project()["outcomes"] == [
        {
            "id": "producer",
            "findings": ["HF-1"],
            "depends_on": [],
            "proof_families": ["native"],
            "inputs": ["runtime/**/*.rs"],
        }
    ]


def test_empty_declarations_are_discovery_only_and_fixed_binding_has_no_phase():
    graph = parse([])
    assert graph.outcomes == ()
    assert graph.unplanned_findings == (
        "HF-1",
        "HF-2",
        "HF-3",
        "HF-4",
        "HF-5",
        "V1-1 (part)",
    )
    historical = parse([entry("history", ("HF-F6",))])
    binding = next(finding for finding in historical.findings if finding.key == "HF-F6")
    assert (binding.recorded_state, binding.outcome_id, binding.planning) == (
        "fixed",
        "history",
        None,
    )


def test_models_own_immutable_snapshots_and_projection_mutation_cannot_change_them():
    declarations = [entry()]
    original = copy.deepcopy(declarations)
    graph = parse(declarations)
    assert declarations == original
    declarations[0]["findings"].append("HF-2")
    declarations[0]["inputs"].clear()
    projection = graph.project()
    projection["outcomes"][0]["depends_on"].append("foreign")
    assert graph.outcomes[0].findings == ("HF-1",)
    assert graph.outcomes[0].inputs == ("runtime/**/*.rs",)
    assert graph.outcomes[0].depends_on == ()
    with pytest.raises(FrozenInstanceError):
        setattr(graph.outcomes[0], "id", "changed")


@pytest.mark.parametrize("entries", [None, {}, "producer", [None], [[]], [True]])
def test_requires_explicit_array_of_objects(entries):
    with pytest.raises(ValueError, match="(array of objects|must be an object)"):
        parse(entries)


@pytest.mark.parametrize(
    "field", ["id", "findings", "depends_on", "proof_families", "inputs"]
)
def test_all_fields_are_explicit(field):
    declaration = entry()
    del declaration[field]
    with pytest.raises(ValueError, match="missing="):
        parse([declaration])


@pytest.mark.parametrize(
    "field", ["reviewed", "receipt", "implementation_ready", "finding"]
)
def test_unknown_fields_cannot_smuggle_readiness_or_an_alternate_contract(field):
    declaration = entry()
    declaration[field] = True
    with pytest.raises(ValueError, match="unknown="):
        parse([declaration])


@pytest.mark.parametrize(
    "key", [None, True, 12, "", "two words", "../other", " x", "x\n"]
)
def test_rejects_malformed_outcome_ids(key):
    with pytest.raises(ValueError, match="outcome identifier"):
        parse([entry(key)])


def test_rejects_duplicate_outcome_ids_and_overlapping_finding_bindings():
    with pytest.raises(ValueError, match="duplicate release outcome ID"):
        parse([entry(), entry(findings=("HF-2",))])
    with pytest.raises(ValueError, match="binds both"):
        parse([entry(), entry("other", ("HF-1", "HF-2"))])


@pytest.mark.parametrize(
    "field", ["findings", "depends_on", "proof_families", "inputs"]
)
@pytest.mark.parametrize(
    "value", [None, "native", {}, [True], [""], [" value"], ["x\x00y"], ["x", "x"]]
)
def test_array_fields_reject_coercion_empty_values_and_duplicates(field, value):
    declaration = entry()
    declaration[field] = value
    with pytest.raises(ValueError, match="(array|canonical strings|repeats)"):
        parse([declaration])


@pytest.mark.parametrize("field", ["findings", "proof_families", "inputs"])
def test_only_dependency_array_can_be_empty(field):
    declaration = entry()
    declaration[field] = []
    with pytest.raises(ValueError, match="nonempty"):
        parse([declaration])


@pytest.mark.parametrize("finding", ["HF-999", "HF-6", "V1-1", "HF-1 (part)"])
def test_finding_keys_must_match_canonical_rows_including_qualifiers(finding):
    with pytest.raises(ValueError, match="unknown finding"):
        parse([entry(findings=(finding,))])


def test_only_supplied_proof_family_catalog_is_admitted():
    with pytest.raises(ValueError, match="unknown proof family 'native.fast'"):
        parse([entry(proof_families=["native.fast"])])
    with pytest.raises(ValueError, match="unknown proof family 'native'"):
        release_outcomes.parse_release_outcomes(
            [entry()], findings=finding_status.project(LEDGER), proof_families=[]
        )


@pytest.mark.parametrize(
    "pattern",
    [
        "/etc/*",
        "C:/work/*",
        "C:relative",
        "\\\\server\\share",
        "src\\*.py",
        "../*",
        "src/../*",
        "./src/*",
        "src//*.py",
        "src/",
        ".",
    ],
)
def test_input_scopes_are_portable_repository_coordinates(pattern):
    with pytest.raises(ValueError, match="repository-relative path globs"):
        parse([entry(inputs=[pattern])])


def test_valid_unicode_and_glob_scopes_need_no_filesystem_expansion():
    graph = parse([entry(inputs=["src/é/[ab]?.py", "runtime/**", "**/Cargo.toml"])])
    assert graph.outcomes[0].inputs == ("**/Cargo.toml", "runtime/**", "src/é/[ab]?.py")


@pytest.mark.parametrize(
    "dependency, message",
    [
        ("producer", "depends on itself"),
        ("absent", "unknown dependency"),
        ("two words", "outcome identifier"),
    ],
)
def test_self_dangling_and_malformed_edges_are_rejected(dependency, message):
    with pytest.raises(ValueError, match=message):
        parse([entry(depends_on=[dependency])])


def test_cycle_is_rejected_even_with_independent_root_and_blocked_descendant():
    with pytest.raises(ValueError, match="dependency cycle blocks: a, b, tail"):
        parse(
            [
                entry("root", ("HF-1",)),
                entry("a", ("HF-2",), depends_on=["b"]),
                entry("b", ("HF-3",), depends_on=["a"]),
                entry("tail", ("HF-4",), depends_on=["b"]),
            ]
        )


def test_broken_ledger_authority_cannot_be_hidden_by_an_empty_graph():
    broken = finding_status.project(LEDGER + "| HF-F6 | Duplicate | fix evidence |\n")
    assert broken.problems
    with pytest.raises(ValueError, match="invalid finding authority"):
        release_outcomes.parse_release_outcomes(
            [], findings=broken, proof_families=FAMILIES
        )


def test_deep_graph_is_iterative_and_projects_direct_edges_only():
    # A legitimate chain beyond Python's ordinary recursion depth must not need
    # recursive traversal or a quadratic transitive-dependency projection.
    count = 1100
    ledger = finding_status.project(
        "\n".join(
            ["## Open"] + [f"| HF-{n} | Finding | observation |" for n in range(count)]
        )
    )
    declarations = [
        entry(str(n), (f"HF-{n}",), depends_on=[str(n - 1)] if n else [])
        for n in range(count)
    ]
    graph = release_outcomes.parse_release_outcomes(
        list(reversed(declarations)), findings=ledger, proof_families=FAMILIES
    )
    assert [outcome.id for outcome in graph.outcomes] == [str(n) for n in range(count)]
    projection = graph.project()
    assert len(projection["findings"]) == count
    assert (
        sum(len(outcome["depends_on"]) for outcome in projection["outcomes"])
        == count - 1
    )
