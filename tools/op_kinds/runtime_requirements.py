"""Shared projections of callable requirements from the semantic role registry."""

from __future__ import annotations


def _inherit_kind_alias_facts(data: dict, facts: dict, merge) -> dict:
    """Aliases inherit canonical semantic facts without rewriting field roles.

    A spelling may add stricter facts (for example raw-integer boxing), but it
    cannot erase the canonical operation's semantic requirements. A shared
    mapper opcode alone is not an alias: GPU and ordinary calls remain distinct.
    """
    result = dict(facts)
    for row in data.get("kind", ()):
        canonical = row["canonical"]
        if canonical not in facts:
            continue
        for alias in row.get("aliases", ()):
            result[alias] = merge(canonical, alias, facts[canonical], result.get(alias))
    return result


def runtime_symbol_requirement_masks(data: dict) -> dict[str, int]:
    masks: dict[str, int] = {}
    for role in data["simpleir_runtime_requirement_roles"]:
        for symbol in role.get("runtime_symbols", []):
            masks[symbol] = masks.get(symbol, 0) | (1 << role["bit"])
    return masks


def runtime_callable_attribute_requirement_masks(data: dict) -> dict[str, int]:
    symbols = runtime_symbol_requirement_masks(data)
    attributes: dict[str, int] = {}
    protected = 0
    for row in data.get("simpleir_runtime_qualified_callable", []):
        bits = symbols[row["symbol"]]
        attr = row["qualified"].rsplit(".", 1)[1]
        attributes[attr] = attributes.get(attr, 0) | bits
        protected |= bits
    gateways = set(data.get("simpleir_runtime_protected_attribute_gateways", []))
    gateways.update(
        qualified.rsplit(".", 1)[1]
        for qualified in data.get("simpleir_runtime_protected_gateway_callables", [])
    )
    for attr in gateways:
        attributes[attr] = attributes.get(attr, 0) | protected
    return attributes


def registered_runtime_kinds(data: dict) -> set[str]:
    """Derive every wire spelling covered by runtime-semantic admission."""

    registered: set[str] = set()
    for row in data.get("kind", []):
        registered.add(row["canonical"])
        registered.update(row.get("aliases", []))
    # Frontend optimizer effect tokens belong to the pre-serialization IR.
    # They never create executable wire spellings or grant runtime admission.
    registered.update(row["kind"] for row in data.get("simpleir_control_kind", []))
    for row in data.get("simpleir_runtime_requirement_roles", []):
        registered.update(data.get(row["table"], []))
    registered.update(data.get("simpleir_runtime_neutral_semantics_kinds", []))
    for table, _ in INTEGER_SEMANTIC_ROLES:
        registered.update(data.get(table, ()))
    return registered


def runtime_kind_requirement_masks(data: dict) -> dict[str, int]:
    """Minimum execution requirements; callable metadata only adds requirements."""
    masks = dict.fromkeys(registered_runtime_kinds(data), 0)
    for role in data["simpleir_runtime_requirement_roles"]:
        for kind in data.get(role["table"], ()):
            masks[kind] |= 1 << role["bit"]
    return _inherit_kind_alias_facts(
        data, masks, lambda canonical, alias, inherited, own: inherited | (own or 0)
    )


def target_runtime_requirement_masks(data: dict) -> dict[str, int]:
    roles = {
        row["constant"]: 1 << row["bit"]
        for row in data["simpleir_runtime_requirement_roles"]
    }
    return {
        row["target"]: sum(roles[name] for name in row["supported"])
        for row in data["simpleir_target_runtime_profiles"]
    }


INTEGER_SEMANTIC_ROLES = (
    ("simpleir_dynamic_add_semantics_kinds", "DynamicAdd"),
    ("simpleir_dynamic_numeric_semantics_kinds", "DynamicNumeric"),
    ("simpleir_dynamic_true_div_semantics_kinds", "DynamicTrueDiv"),
    ("simpleir_dynamic_divmod_semantics_kinds", "DynamicDivmod"),
    ("simpleir_dynamic_power_semantics_kinds", "DynamicPower"),
    ("simpleir_dynamic_unary_numeric_semantics_kinds", "DynamicUnaryNumeric"),
    ("simpleir_integer_only_semantics_kinds", "IntegerOnly"),
    ("simpleir_integer_literal_semantics_kinds", "IntegerLiteral"),
    ("simpleir_integer_producer_semantics_kinds", "IntegerProducer"),
)


def integer_semantics_by_kind(data: dict) -> dict[str, str]:
    roles = {
        kind: role
        for table, role in INTEGER_SEMANTIC_ROLES
        for kind in data.get(table, ())
    }

    def merge(canonical, alias, inherited, own):
        if own is not None and own != inherited:
            raise ValueError(
                f"numeric role for {alias!r} differs from canonical {canonical!r}"
            )
        return inherited

    return _inherit_kind_alias_facts(data, roles, merge)
