"""Closed Rust wire vocabulary and exhaustive fact rendering."""

from __future__ import annotations

from .registration import backend_private_kinds, registered_simpleir_kinds


def variant(kind: str) -> str:
    # A trailing underscore is meaningful: `mod` and `mod_` are distinct kinds.
    return "".join(part[:1].upper() + part[1:] for part in kind.split("_")) + (
        "_" if kind.endswith("_") else ""
    )


def kind_ref(kind: str) -> str:
    return f"OpKind::{variant(kind)}"


def arms(data: dict, values: dict[str, str], default: str) -> str:
    """Group equal decisions, explicitly covering the complete closed domain."""
    domain = registered_simpleir_kinds(data)
    unknown = values.keys() - domain
    if unknown:
        raise ValueError(f"unregistered wire facts: {sorted(unknown)}")
    groups: dict[str, list[str]] = {}
    for kind in sorted(domain):
        groups.setdefault(values.get(kind, default), []).append(kind_ref(kind))
    return "".join(
        f"        {' | '.join(kinds)} => {value},\n" for value, kinds in groups.items()
    )


def fact(
    data: dict,
    name: str,
    result: str,
    values: dict[str, str],
    default: str,
    *,
    extra: str = "",
    doc: str = "",
) -> str:
    method = name.startswith("simpleir_kind_")
    name = name.removeprefix("simpleir_kind_") if method else name
    receiver = "self" if method else "kind: OpKind"
    subject = "self" if method else "kind"
    body = (
        (f"/// {doc}\n" if doc else "")
        + f"#[inline]\npub const fn {name}({receiver}{extra}) -> {result} {{\n"
        + f"    match {subject} {{\n"
        + arms(data, values, default)
        + "    }\n}\n\n"
    )
    return f"impl OpKind {{\n{body}}}\n\n" if method else body


def boolean(data: dict, name: str, members: list[str] | set[str], doc: str = "") -> str:
    return fact(data, name, "bool", dict.fromkeys(members, "true"), "false", doc=doc)


def render_enum(data: dict) -> str:
    kinds = sorted(registered_simpleir_kinds(data))
    variants = [variant(kind) for kind in kinds]
    if len(set(variants)) != len(variants):
        raise ValueError("wire-kind Rust variants collide")
    out = [
        "/// The complete SimpleIR wire vocabulary. Parsed only at transport boundaries.\n",
        "#[repr(u16)]\n",
        "#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize)]\n",
        "pub enum OpKind {\n",
    ]
    for kind in kinds:
        out.append(f'    #[serde(rename = "{kind}")]\n    {variant(kind)},\n')
    out.append("}\n\nimpl OpKind {\n")
    out.append("    pub const ALL: &'static [Self] = &[\n")
    out.extend(f"        Self::{variant(kind)},\n" for kind in kinds)
    out.append(
        "    ];\n\n    pub const fn as_str(self) -> &'static str {\n        match self {\n"
    )
    out.extend(f'            Self::{variant(kind)} => "{kind}",\n' for kind in kinds)
    out.append("        }\n    }\n}\n\n")
    out.append(
        "impl std::fmt::Display for OpKind {\n"
        "    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
        "        f.write_str(self.as_str())\n    }\n}\n\n"
        "/// Decode a wire spelling without allocating. Internal producers use variants.\n"
        "impl std::str::FromStr for OpKind {\n"
        "    type Err = String;\n"
        "    fn from_str(input: &str) -> Result<Self, Self::Err> {\n"
        "        match input {\n"
    )
    out.extend(
        f'            "{kind}" => Ok(Self::{variant(kind)}),\n' for kind in kinds
    )
    out.append(
        '            _ => Err(format!("unknown SimpleIR op kind {input:?}")),\n        }\n    }\n}\n\n'
    )
    out.append(
        "// Deserialize wire names only: serde's enum visitor also accepts numeric\n"
        "// variant indices in MessagePack, which are not stable wire identities.\n"
        "impl<'de> serde::Deserialize<'de> for OpKind {\n"
        "    fn deserialize<D: serde::Deserializer<'de>>(decoder: D) -> Result<Self, D::Error> {\n"
        "        struct WireKindVisitor;\n"
        "        impl<'de> serde::de::Visitor<'de> for WireKindVisitor {\n"
        "            type Value = OpKind;\n"
        "            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {\n"
        '                f.write_str("a SimpleIR wire kind spelling")\n'
        "            }\n"
        "            fn visit_str<E: serde::de::Error>(self, input: &str) -> Result<Self::Value, E> {\n"
        "                input.parse().map_err(E::custom)\n"
        "            }\n"
        "        }\n"
        "        decoder.deserialize_str(WireKindVisitor)\n"
        "    }\n"
        "}\n\n"
    )
    private = backend_private_kinds(data)
    out.append(
        fact(
            data,
            "simpleir_backend_private_owner",
            "Option<&'static str>",
            {kind: f'Some("{owner}")' for kind, owner in private.items()},
            "None",
        )
    )
    out.append("pub const SIMPLEIR_BACKEND_PRIVATE_KINDS: &[(OpKind, &str)] = &[\n")
    out.extend(
        f'    ({kind_ref(kind)}, "{owner}"),\n'
        for kind, owner in sorted(private.items())
    )
    out.append("];\n\n")
    out.append(boolean(data, "simpleir_kind_is_registered", set(kinds)))
    return "".join(out)
