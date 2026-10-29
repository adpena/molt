from __future__ import annotations

from dataclasses import FrozenInstanceError
from types import SimpleNamespace

import pytest

from molt import process_environment
from molt.process_environment import (
    NameRule,
    ProcessEnvironment,
    ProcessEnvironmentBuilder,
)


@pytest.mark.parametrize(
    "rule,expected",
    [
        (NameRule.EXACT, ["Path", "Straße", "SS", "ß", "ﬃ", "ı", "é"]),
        (NameRule.WINDOWS, ["PATH", "STRAßE", "SS", "ß", "ﬃ", "I", "É"]),
    ],
)
def test_name_rule_uses_simple_uppercase_without_expansion(rule, expected):
    assert [
        rule.key(name) for name in ["Path", "Straße", "SS", "ß", "ﬃ", "ı", "é"]
    ] == expected
    env = ProcessEnvironment({"ß": "sharp", "SS": "double"}, rule=rule)
    assert len(env) == 2
    assert env["ß"] == "sharp"
    assert env["SS"] == "double"


@pytest.mark.parametrize("rule", list(NameRule))
def test_host_rule_and_explicit_rule_copy_custody(monkeypatch, rule):
    monkeypatch.setattr(
        process_environment,
        "os",
        SimpleNamespace(name="nt" if rule is NameRule.WINDOWS else "posix"),
    )
    assert NameRule.host() is rule
    assert ProcessEnvironment({}).rule is rule
    assert ProcessEnvironmentBuilder({}).rule is rule
    other = NameRule.EXACT if rule is NameRule.WINDOWS else NameRule.WINDOWS
    source = ProcessEnvironment({"Path": "original"}, rule=other)
    builder = ProcessEnvironmentBuilder(source)
    assert builder.rule is other
    assert ProcessEnvironment(builder).rule is other
    assert ProcessEnvironment(source).updated({}).without([]).rule is other
    assert ProcessEnvironment(source, rule=rule).rule is rule
    assert ProcessEnvironmentBuilder(builder, rule=rule).rule is rule


@pytest.mark.parametrize(
    "rule,expected",
    [
        (NameRule.EXACT, {"Path": "first", "PATH": "last"}),
        (NameRule.WINDOWS, {"Path": "last"}),
    ],
)
def test_first_spelling_survives_and_posix_keeps_distinct_names(rule, expected):
    env = ProcessEnvironment({"Path": "first", "PATH": "last"}, rule=rule)
    assert dict(env) == expected
    assert list(env) == list(expected)
    assert len(env) == len(expected)


@pytest.mark.parametrize("rule", list(NameRule))
def test_lookup_and_membership_follow_the_rule(rule):
    env = ProcessEnvironment({"Path": "tools"}, rule=rule)
    assert env["Path"] == "tools" and "Path" in env
    if rule is NameRule.WINDOWS:
        assert env["pAtH"] == "tools" and "pAtH" in env
    else:
        assert "pAtH" not in env
        with pytest.raises(KeyError):
            env["pAtH"]
    assert env.get("missing") is None and "missing" not in env


@pytest.mark.parametrize("rule", list(NameRule))
def test_updated_and_without_preserve_the_source_and_rule(rule):
    source = ProcessEnvironment({"Path": "old", "KEEP": "value"}, rule=rule)
    updated = source.updated({"PATH": "new", "ADDED": "extra"})
    expected = (
        {"Path": "new", "KEEP": "value", "ADDED": "extra"}
        if rule is NameRule.WINDOWS
        else {"Path": "old", "KEEP": "value", "PATH": "new", "ADDED": "extra"}
    )
    assert dict(updated) == expected
    removed = updated.without(["PATH", "missing"])
    assert dict(removed) == (
        {"KEEP": "value", "ADDED": "extra"}
        if rule is NameRule.WINDOWS
        else {"Path": "old", "KEEP": "value", "ADDED": "extra"}
    )
    assert dict(source) == {"Path": "old", "KEEP": "value"}
    assert dict(updated) == expected
    assert removed.rule is updated.rule is source.rule is rule


@pytest.mark.parametrize("rule", list(NameRule))
def test_builder_mutation_keeps_spelling_and_snapshots_do_not_alias(rule):
    values = {"Path": "old", "KEEP": "value"}
    builder = ProcessEnvironmentBuilder(values, rule=rule)
    selector = "PATH" if rule is NameRule.WINDOWS else "Path"
    builder[selector] = "new"
    assert dict(builder) == {"Path": "new", "KEEP": "value"}
    frozen = ProcessEnvironment(builder)
    copied = ProcessEnvironment(frozen)
    del builder[selector]
    builder[selector] = "later"
    values["KEEP"] = "outside"
    assert dict(frozen) == {"Path": "new", "KEEP": "value"}
    assert copied == frozen and copied.rule is rule
    assert dict(copied) == {"Path": "new", "KEEP": "value"}
    assert dict(builder) == {"KEEP": "value", selector: "later"}
    with pytest.raises(KeyError):
        del builder["missing"]


@pytest.mark.parametrize("rule", list(NameRule))
def test_snapshot_rejects_mutation(rule):
    values = {"Path": "original"}
    env = ProcessEnvironment(values, rule=rule)
    values["Path"] = "outside"
    with pytest.raises(TypeError):
        env["Path"] = "changed"
    with pytest.raises(FrozenInstanceError):
        env.rule = NameRule.EXACT
    assert env["Path"] == "original"


@pytest.mark.parametrize("rule", list(NameRule))
def test_equality_compares_rules_and_name_identity(rule):
    env = ProcessEnvironment({"Path": "tools", "KEEP": "value"}, rule=rule)
    same = ProcessEnvironment({"KEEP": "value", "Path": "tools"}, rule=rule)
    alias = ProcessEnvironment({"PATH": "tools", "KEEP": "value"}, rule=rule)
    other_rule = NameRule.EXACT if rule is NameRule.WINDOWS else NameRule.WINDOWS
    assert env == same and same == env
    assert (env == alias) is (rule is NameRule.WINDOWS)
    assert (alias == env) is (rule is NameRule.WINDOWS)
    assert env != ProcessEnvironment(dict(env), rule=other_rule)
    assert ProcessEnvironment({"UPPER": "value"}, rule=rule) != ProcessEnvironment(
        {"UPPER": "value"}, rule=other_rule
    )
    assert env != env.updated({"KEEP": "changed"})
    assert env != env.without(["KEEP"])
    assert env.without(["KEEP"]) != env
    assert env != dict(env) and dict(env) != env


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_hash_agrees_with_equality_for_values_that_are_not_utf8(rule):
    # os.environ holds a POSIX byte that is not UTF-8 as a surrogate escape,
    # and a Windows lone UTF-16 surrogate as itself; neither encodes as UTF-8.
    escaped = "ab" + b"\xff".decode("utf-8", "surrogateescape") + "cd"
    lone = "ab\ud800cd"
    with pytest.raises(UnicodeEncodeError):
        escaped.encode("utf-8")
    env = ProcessEnvironment({"Path": escaped, "KEEP": lone}, rule=rule)
    same = ProcessEnvironment({"KEEP": lone, "Path": escaped}, rule=rule)
    alias = ProcessEnvironment({"PATH": escaped, "KEEP": lone}, rule=rule)
    assert hash(env) == hash(same) and {env: "cached"}[same] == "cached"
    if rule is NameRule.WINDOWS:
        assert env == alias and hash(env) == hash(alias)
    assert env.updated({"Path": "ab?cd"}) not in {env}


@pytest.mark.parametrize(
    "rule,ordered_names",
    [(NameRule.EXACT, ["Z", "a", "ß"]), (NameRule.WINDOWS, ["a", "Z", "ß"])],
)
def test_child_block_orders_by_rule_and_returns_an_independent_dict(
    rule, ordered_names
):
    env = ProcessEnvironment({"ß": "sharp", "a": "one", "Z": "last"}, rule=rule)
    child = env.child_block()
    assert list(child) == ordered_names
    assert child == {"ß": "sharp", "a": "one", "Z": "last"}
    child["a"] = "changed"
    assert env["a"] == "one"
