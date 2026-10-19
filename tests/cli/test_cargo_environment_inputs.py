from __future__ import annotations

import copy
from dataclasses import FrozenInstanceError
import hashlib
import json

import pytest

from molt.cli.cargo_environment_inputs import CargoEnvironmentInputs
from molt.process_environment import NameRule, ProcessEnvironment


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_observed_inputs_retain_absence_empty_and_secret_digests(rule):
    environment = ProcessEnvironment({"EMPTY": "", "TOKEN": "private-token"}, rule=rule)
    record = CargoEnvironmentInputs.capture(
        environment, ["TOKEN", "MISSING", "EMPTY"], local_unit_count=3
    )
    payload = record.to_dict()
    assert payload == {
        "schema": "molt.cargo-environment-inputs.v1",
        "name_rule": rule.value,
        "local_unit_count": 3,
        "variables": [
            {"name": "EMPTY", "sha256": hashlib.sha256(b"").hexdigest()},
            {"name": "MISSING", "sha256": None},
            {"name": "TOKEN", "sha256": hashlib.sha256(b"private-token").hexdigest()},
        ],
    }
    assert "private-token" not in json.dumps(payload)
    assert CargoEnvironmentInputs.from_dict(payload) == record
    record.verify(environment.updated({"UNREAD": "irrelevant"}))
    with pytest.raises(ValueError, match="MISSING"):
        record.verify(environment.updated({"MISSING": ""}))
    with pytest.raises(ValueError, match="EMPTY"):
        record.verify(environment.without(["EMPTY"]))


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_verification_reports_every_changed_name_without_values(rule):
    record = CargoEnvironmentInputs.capture(
        ProcessEnvironment({"A": "old", "B": "old"}, rule=rule),
        ["B", "A", "C"],
        local_unit_count=1,
    )
    with pytest.raises(ValueError) as error:
        record.verify(ProcessEnvironment({"B": "secret", "C": ""}, rule=rule))
    assert str(error.value) == "Cargo environment inputs changed: A, B, C"


def test_digest_is_deterministic_and_independent_of_caller_mutation():
    source = {"B": "second", "A": "first"}
    environment = ProcessEnvironment(source, rule=NameRule.EXACT)
    first = CargoEnvironmentInputs.capture(environment, ["B", "A"], local_unit_count=2)
    second = CargoEnvironmentInputs.capture(
        environment, ["A", "B", "A"], local_unit_count=2
    )
    wire = json.dumps(first.to_dict(), sort_keys=True, separators=(",", ":")).encode()
    assert first.digest == second.digest == hashlib.sha256(wire).hexdigest()
    source["A"] = "different"
    exported = first.to_dict()
    exported["variables"][0]["sha256"] = None
    first.verify(environment)
    assert first.digest == second.digest
    with pytest.raises(FrozenInstanceError):
        first.local_unit_count = 9


def test_windows_names_use_nonexpanding_rule_and_exact_names_remain_distinct():
    windows = CargoEnvironmentInputs.capture(
        ProcessEnvironment({"Path": "one", "ß": "two"}, rule=NameRule.WINDOWS),
        ["path", "PATH", "ß", "SS"],
        local_unit_count=1,
    )
    windows.verify(
        ProcessEnvironment({"PATH": "one", "ß": "two"}, rule=NameRule.WINDOWS)
    )
    assert [row[0] for row in windows.variables] == ["PATH", "SS", "ß"]
    exact = CargoEnvironmentInputs.capture(
        ProcessEnvironment({"Path": "one"}, rule=NameRule.EXACT),
        ["Path", "PATH"],
        local_unit_count=1,
    )
    assert exact.variables[0] == ("PATH", None)
    assert exact.digest != windows.digest


def test_verification_requires_the_original_name_rule_and_frozen_environment():
    record = CargoEnvironmentInputs.capture(
        ProcessEnvironment({}, rule=NameRule.EXACT), ["A"], local_unit_count=1
    )
    with pytest.raises(ValueError, match="name rule.*A"):
        record.verify(ProcessEnvironment({}, rule=NameRule.WINDOWS))
    with pytest.raises(TypeError, match="ProcessEnvironment"):
        record.verify({})
    with pytest.raises(TypeError, match="ProcessEnvironment"):
        CargoEnvironmentInputs.capture({}, [], local_unit_count=1)


def test_var_os_value_surrogates_roundtrip_losslessly_and_detect_change():
    environment = ProcessEnvironment(
        {"NATIVE_VALUE": "path-\udcff"}, rule=NameRule.EXACT
    )
    record = CargoEnvironmentInputs.capture(
        environment, ["NATIVE_VALUE"], local_unit_count=1
    )
    assert record.variables == (
        ("NATIVE_VALUE", hashlib.sha256(b"path-\xed\xb3\xbf").hexdigest()),
    )
    reloaded = CargoEnvironmentInputs.from_dict(
        json.loads(json.dumps(record.to_dict()))
    )
    reloaded.verify(environment)
    with pytest.raises(ValueError, match="NATIVE_VALUE"):
        reloaded.verify(environment.updated({"NATIVE_VALUE": "path-\udcfe"}))


@pytest.mark.parametrize(
    "mutation",
    [
        "schema",
        "extra",
        "rule",
        "zero",
        "boolean",
        "negative",
        "duplicate",
        "order",
        "digest",
        "plaintext",
        "row_extra",
        "noncanonical",
        "empty_name",
    ],
)
def test_malformed_records_never_become_admission(mutation):
    payload = CargoEnvironmentInputs.capture(
        ProcessEnvironment({}, rule=NameRule.WINDOWS), ["A", "B"], local_unit_count=1
    ).to_dict()
    payload = copy.deepcopy(payload)
    if mutation == "schema":
        payload["schema"] = "legacy"
    elif mutation == "extra":
        payload["trusted"] = True
    elif mutation == "rule":
        payload["name_rule"] = "host"
    elif mutation in {"zero", "boolean", "negative"}:
        payload["local_unit_count"] = {"zero": 0, "boolean": True, "negative": -1}[
            mutation
        ]
    elif mutation == "duplicate":
        payload["variables"].append(payload["variables"][0])
    elif mutation == "order":
        payload["variables"].reverse()
    elif mutation == "digest":
        payload["variables"][0]["sha256"] = "A" * 64
    elif mutation == "plaintext":
        payload["variables"][0]["sha256"] = "secret"
    elif mutation == "row_extra":
        payload["variables"][0]["value"] = "secret"
    elif mutation == "noncanonical":
        payload["variables"][0]["name"] = "a"
    else:
        payload["variables"][0]["name"] = ""
    with pytest.raises(ValueError):
        CargoEnvironmentInputs.from_dict(payload)
