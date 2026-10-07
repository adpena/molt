#!/usr/bin/env python3
"""Render the ``MOLT_*`` environment registry projections.

Authority: ``src/molt/environment_registry.toml``. Outputs:

* ``src/molt/_environment_registry.py`` — the shipped projection the CLI
  loads on every start (JSON payload; no TOML parse at run time).
* ``docs/environment-variables.generated.md`` — the reference page.

Usage:
    python3 tools/gen_environment_registry.py --write  # rewrite stale outputs
    python3 tools/gen_environment_registry.py --check  # exit 1 when stale

The generator is the only TOML parser; ``molt.environment_registry`` loads
the projection and ``tools/check_environment_registry.py`` proves the
projection is current before it scans the source.
"""

from __future__ import annotations

import json
from pathlib import Path
import re
import sys
import tomllib
from typing import Any

ROOT = Path(__file__).resolve().parents[1]
for _import_root in (ROOT / "tools", ROOT / "src"):
    if str(_import_root) not in sys.path:
        sys.path.insert(0, str(_import_root))

from generator_io import generator_main, stale_outputs  # noqa: E402
from molt.environment_registry import (  # noqa: E402
    AUDIENCES,
    KINDS,
    EnvironmentRegistry,
    EnvironmentRegistryError,
    registry_from_payload,
)

SOURCE_TOML = ROOT / "src" / "molt" / "environment_registry.toml"
OUT_PY = ROOT / "src" / "molt" / "_environment_registry.py"
OUT_DOC = ROOT / "docs" / "environment-variables.generated.md"
SCHEMA = "molt.environment-registry.v1"

_NAME = re.compile(r"^MOLT_[A-Z0-9_]+$")
_STEM = re.compile(r"^MOLT_[A-Z0-9_]+$")
_SUFFIX = re.compile(r"^_[A-Z0-9_]+$")
_PREFIX = re.compile(r"^MOLT_[A-Z0-9_]+_$")
_DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
_BOOL_DEFAULTS = frozenset({"", "0", "1"})

_AUDIENCE_TITLES = {
    "user": "User variables",
    "developer": "Developer variables",
    "ci": "CI variables",
    "internal": "Internal variables",
}
_AUDIENCE_INTROS = {
    "user": (
        "Set these when you use the `molt` CLI on your own project. They are "
        "read by the shipped compiler, runtime, or CLI."
    ),
    "developer": (
        "Set these when you work on Molt itself: the dev wrappers, proof "
        "queue, benchmarks, and guards under `tools/`."
    ),
    "ci": (
        "Set by the GitHub workflows (or as repository variables). They "
        "configure CI custody roots, pinned tool releases, and trust policy."
    ),
    "internal": (
        "Set by one Molt process for a child it launches. Do not set these "
        "by hand; the parent owns the value."
    ),
}


class RegistryFormatError(ValueError):
    """The TOML authority violates the registry contract."""


def _fail(message: str) -> None:
    raise RegistryFormatError(f"{SOURCE_TOML.relative_to(ROOT).as_posix()}: {message}")


def _require_str(row: dict[str, Any], key: str, *, context: str) -> str:
    value = row.get(key)
    if not isinstance(value, str):
        _fail(f"{context}: field {key!r} must be a string")
    return value


def _optional_str(row: dict[str, Any], key: str, *, context: str) -> str:
    value = row.get(key, "")
    if not isinstance(value, str):
        _fail(f"{context}: field {key!r} must be a string")
    return value


def _optional_values(row: dict[str, Any], *, context: str) -> list[str]:
    values = row.get("values", [])
    if not isinstance(values, list) or not all(isinstance(v, str) for v in values):
        _fail(f"{context}: field 'values' must be a list of strings")
    if len(set(values)) != len(values):
        _fail(f"{context}: 'values' repeats an entry")
    return values


def _check_summary(summary: str, *, context: str) -> None:
    if not summary.strip():
        _fail(f"{context}: summary is empty")
    if "\n" in summary:
        _fail(f"{context}: summary must be one line")
    if not summary.endswith("."):
        _fail(f"{context}: summary must end with a period")


def _check_owner(owner: str, *, context: str) -> None:
    if not owner or owner.startswith("/") or "\\" in owner:
        _fail(f"{context}: owner must be a repo-relative POSIX path, got {owner!r}")


def _check_kind_fields(
    row: dict[str, Any], *, context: str
) -> tuple[str, str, str, list[str]]:
    audience = _require_str(row, "audience", context=context)
    if audience not in AUDIENCES:
        _fail(f"{context}: audience must be one of {AUDIENCES}, got {audience!r}")
    kind = _require_str(row, "kind", context=context)
    if kind not in KINDS:
        _fail(f"{context}: kind must be one of {KINDS}, got {kind!r}")
    default = _optional_str(row, "default", context=context)
    values = _optional_values(row, context=context)
    if kind == "enum":
        if not values:
            _fail(f"{context}: an enum needs a non-empty 'values' list")
        if default and default not in values:
            _fail(f"{context}: default {default!r} is not one of {values}")
    elif values:
        _fail(f"{context}: 'values' is only valid for kind = \"enum\"")
    if kind == "bool" and default not in _BOOL_DEFAULTS:
        _fail(f'{context}: a bool default must be "", "0" or "1", got {default!r}')
    if kind == "int" and default:
        try:
            int(default)
        except ValueError:
            _fail(
                f"{context}: an int default must parse as an integer, got {default!r}"
            )
    if kind == "float" and default:
        try:
            float(default)
        except ValueError:
            _fail(f"{context}: a float default must parse as a number, got {default!r}")
    return audience, kind, default, values


def _check_sorted(names: list[str], *, context: str) -> None:
    for previous, current in zip(names, names[1:]):
        if current <= previous:
            _fail(
                f"{context}: rows must be sorted and unique; {current!r} follows {previous!r}"
            )


def _rows(data: dict[str, Any], key: str) -> list[dict[str, Any]]:
    rows = data.get(key, [])
    if not isinstance(rows, list) or not all(isinstance(r, dict) for r in rows):
        _fail(f"[[{key}]] must be an array of tables")
    return rows


def load_source(path: Path = SOURCE_TOML) -> dict[str, Any]:
    try:
        return tomllib.loads(path.read_text(encoding="utf-8"))
    except tomllib.TOMLDecodeError as exc:
        raise RegistryFormatError(
            f"{path.relative_to(ROOT).as_posix()}: {exc}"
        ) from exc


def build_payload(data: dict[str, Any]) -> dict[str, Any]:
    """Validate the TOML authority and return the JSON-ready payload."""

    if data.get("schema") != SCHEMA:
        _fail(f"schema must be {SCHEMA!r}, got {data.get('schema')!r}")

    variables: list[dict[str, Any]] = []
    for row in _rows(data, "variable"):
        name = _require_str(row, "name", context="[[variable]]")
        context = f"[[variable]] {name}"
        if not _NAME.match(name):
            _fail(f"{context}: name must match MOLT_[A-Z0-9_]+")
        audience, kind, default, values = _check_kind_fields(row, context=context)
        owner = _require_str(row, "owner", context=context)
        _check_owner(owner, context=context)
        summary = _require_str(row, "summary", context=context)
        _check_summary(summary, context=context)
        extra = set(row) - {
            "name",
            "audience",
            "kind",
            "default",
            "values",
            "owner",
            "summary",
        }
        if extra:
            _fail(f"{context}: unknown fields {sorted(extra)}")
        variables.append(
            {
                "name": name,
                "audience": audience,
                "kind": kind,
                "default": default,
                "values": values,
                "owner": owner,
                "summary": summary,
            }
        )
    _check_sorted([v["name"] for v in variables], context="[[variable]]")
    variable_names = {v["name"] for v in variables}

    stems: list[dict[str, Any]] = []
    for row in _rows(data, "stem"):
        name = _require_str(row, "name", context="[[stem]]")
        context = f"[[stem]] {name}"
        if not _STEM.match(name):
            _fail(f"{context}: name must match MOLT_[A-Z0-9_]+")
        owner = _require_str(row, "owner", context=context)
        _check_owner(owner, context=context)
        summary = _require_str(row, "summary", context=context)
        _check_summary(summary, context=context)
        extra = set(row) - {"name", "owner", "summary"}
        if extra:
            _fail(f"{context}: unknown fields {sorted(extra)}")
        stems.append({"name": name, "owner": owner, "summary": summary})
    _check_sorted([s["name"] for s in stems], context="[[stem]]")
    stem_names = {s["name"] for s in stems}

    families: list[dict[str, Any]] = []
    for row in _rows(data, "family"):
        suffix = _require_str(row, "suffix", context="[[family]]")
        context = f"[[family]] {suffix}"
        if not _SUFFIX.match(suffix):
            _fail(f"{context}: suffix must match _[A-Z0-9_]+")
        root_fallback = _optional_str(row, "root_fallback", context=context)
        if root_fallback:
            if not _NAME.match(root_fallback):
                _fail(f"{context}: root_fallback must match MOLT_[A-Z0-9_]+")
            if root_fallback in variable_names:
                _fail(
                    f"{context}: root_fallback {root_fallback} is already a "
                    "[[variable]]; a family root is registered through its family"
                )
        audience, kind, default, values = _check_kind_fields(row, context=context)
        owner = _require_str(row, "owner", context=context)
        _check_owner(owner, context=context)
        summary = _require_str(row, "summary", context=context)
        _check_summary(summary, context=context)
        extra = set(row) - {
            "suffix",
            "root_fallback",
            "audience",
            "kind",
            "default",
            "values",
            "owner",
            "summary",
        }
        if extra:
            _fail(f"{context}: unknown fields {sorted(extra)}")
        for stem in stem_names:
            if f"{stem}{suffix}" in variable_names:
                _fail(
                    f"{context}: {stem}{suffix} is also a [[variable]]; pick one owner"
                )
        families.append(
            {
                "suffix": suffix,
                "root_fallback": root_fallback,
                "audience": audience,
                "kind": kind,
                "default": default,
                "values": values,
                "owner": owner,
                "summary": summary,
            }
        )
    _check_sorted([f["suffix"] for f in families], context="[[family]]")
    roots = [f["root_fallback"] for f in families if f["root_fallback"]]
    if len(set(roots)) != len(roots):
        _fail("[[family]]: two families share a root_fallback")

    prefix_families: list[dict[str, Any]] = []
    for row in _rows(data, "prefix_family"):
        prefix = _require_str(row, "prefix", context="[[prefix_family]]")
        context = f"[[prefix_family]] {prefix}"
        if not _PREFIX.match(prefix):
            _fail(f"{context}: prefix must match MOLT_[A-Z0-9_]+_")
        audience, kind, default, values = _check_kind_fields(row, context=context)
        if default or values:
            _fail(f"{context}: a prefix family has no default or values")
        owner = _require_str(row, "owner", context=context)
        _check_owner(owner, context=context)
        summary = _require_str(row, "summary", context=context)
        _check_summary(summary, context=context)
        extra = set(row) - {"prefix", "audience", "kind", "owner", "summary"}
        if extra:
            _fail(f"{context}: unknown fields {sorted(extra)}")
        shadowed = sorted(n for n in variable_names if n.startswith(prefix))
        if shadowed:
            _fail(
                f"{context}: variables {shadowed} start with this prefix; pick one owner"
            )
        prefix_families.append(
            {
                "prefix": prefix,
                "audience": audience,
                "kind": kind,
                "owner": owner,
                "summary": summary,
            }
        )
    _check_sorted([p["prefix"] for p in prefix_families], context="[[prefix_family]]")

    retired: list[dict[str, Any]] = []
    retired_suffixes: list[dict[str, Any]] = []
    for row in _rows(data, "retired"):
        context = "[[retired]]"
        retired_on = row.get("retired")
        retired_text = (
            retired_on.isoformat()
            if hasattr(retired_on, "isoformat")
            else str(retired_on)
        )
        if not _DATE.match(retired_text):
            _fail(
                f"{context}: 'retired' must be a date (YYYY-MM-DD), got {retired_on!r}"
            )
        if "suffix" in row:
            suffix = _require_str(row, "suffix", context=context)
            context = f"[[retired]] {suffix}"
            replacement_suffix = _require_str(
                row, "replacement_suffix", context=context
            )
            if not _SUFFIX.match(suffix) or not _SUFFIX.match(replacement_suffix):
                _fail(f"{context}: suffixes must match _[A-Z0-9_]+")
            if any(f["suffix"] == suffix for f in families):
                _fail(f"{context}: a retired suffix cannot also be a [[family]]")
            if not any(f["suffix"] == replacement_suffix for f in families):
                _fail(
                    f"{context}: replacement_suffix {replacement_suffix} is not a [[family]]"
                )
            extra = set(row) - {"suffix", "replacement_suffix", "retired"}
            if extra:
                _fail(f"{context}: unknown fields {sorted(extra)}")
            retired_suffixes.append(
                {
                    "suffix": suffix,
                    "replacement_suffix": replacement_suffix,
                    "retired": retired_text,
                }
            )
            continue
        name = _require_str(row, "name", context=context)
        context = f"[[retired]] {name}"
        if not _NAME.match(name):
            _fail(f"{context}: name must match MOLT_[A-Z0-9_]+")
        replacement = _optional_str(row, "replacement", context=context)
        note = _optional_str(row, "note", context=context)
        if bool(replacement) == bool(note):
            _fail(
                f"{context}: give exactly one of 'replacement' (a registered name) or 'note'"
            )
        rejected_by = row.get("rejected_by", [])
        if not isinstance(rejected_by, list) or not all(
            isinstance(p, str) for p in rejected_by
        ):
            _fail(f"{context}: 'rejected_by' must be a list of repo-relative paths")
        for path in rejected_by:
            _check_owner(path, context=context)
        extra = set(row) - {"name", "replacement", "retired", "note", "rejected_by"}
        if extra:
            _fail(f"{context}: unknown fields {sorted(extra)}")
        retired.append(
            {
                "name": name,
                "replacement": replacement,
                "retired": retired_text,
                "note": note,
                "rejected_by": list(rejected_by),
            }
        )
    _check_sorted([r["name"] for r in retired], context="[[retired]] names")
    _check_sorted(
        [r["suffix"] for r in retired_suffixes], context="[[retired]] suffixes"
    )

    payload = {
        "schema": SCHEMA,
        "variables": variables,
        "stems": stems,
        "families": families,
        "prefix_families": prefix_families,
        "retired": retired,
        "retired_suffixes": retired_suffixes,
    }
    registry = registry_from_payload(payload)
    for row in retired:
        if registry.is_registered(row["name"]):
            _fail(
                f"[[retired]] {row['name']}: still registered; retire or keep, not both"
            )
        if row["replacement"] and not registry.is_registered(row["replacement"]):
            _fail(
                f"[[retired]] {row['name']}: replacement {row['replacement']} is not registered"
            )
    return payload


def build_registry(data: dict[str, Any] | None = None) -> EnvironmentRegistry:
    try:
        return registry_from_payload(
            build_payload(load_source() if data is None else data)
        )
    except EnvironmentRegistryError as exc:
        raise RegistryFormatError(str(exc)) from exc


# --------------------------------------------------------------------------
# Renderers
# --------------------------------------------------------------------------


def render_python(payload: dict[str, Any]) -> str:
    body = json.dumps(payload, indent=2, sort_keys=True)
    return (
        "# @generated by tools/gen_environment_registry.py from\n"
        "# src/molt/environment_registry.toml. DO NOT EDIT.\n"
        "\n"
        "from __future__ import annotations\n"
        "\n"
        "import json\n"
        "from typing import Any\n"
        "\n"
        f'_REGISTRY_JSON = r"""{body}"""\n'
        "\n"
        "\n"
        "def environment_registry() -> dict[str, Any]:\n"
        '    """Return the registry payload (fresh dict each call)."""\n'
        "\n"
        "    return json.loads(_REGISTRY_JSON)\n"
    )


def _cell(text: str) -> str:
    return text.replace("|", "\\|")


def _code(text: str) -> str:
    return f"`{text}`" if text else ""


def _kind_cell(kind: str, values: list[str]) -> str:
    if kind == "enum":
        return "enum: " + ", ".join(f"`{v}`" for v in values)
    return kind


def _default_cell(default: str) -> str:
    return _code(default) if default else "unset"


def _variable_table(rows: list[dict[str, Any]]) -> list[str]:
    lines = [
        "| Variable | Kind | Default | Read by | Summary |",
        "|---|---|---|---|---|",
    ]
    for row in rows:
        lines.append(
            "| "
            + " | ".join(
                (
                    _code(row["name"]),
                    _cell(_kind_cell(row["kind"], row["values"])),
                    _cell(_default_cell(row["default"])),
                    _code(row["owner"]),
                    _cell(row["summary"]),
                )
            )
            + " |"
        )
    return lines


def render_doc(payload: dict[str, Any]) -> str:
    variables = payload["variables"]
    stems = payload["stems"]
    families = payload["families"]
    prefix_families = payload["prefix_families"]
    retired = payload["retired"]
    retired_suffixes = payload["retired_suffixes"]
    counts = {a: sum(1 for v in variables if v["audience"] == a) for a in AUDIENCES}

    out: list[str] = [
        "# Environment variables",
        "",
        "> Generated by `tools/gen_environment_registry.py` from "
        "`src/molt/environment_registry.toml`; do not edit.",
        "",
        "Every `MOLT_*` name Molt reads is registered here: "
        f"{len(variables)} fixed variables, {len(families)} families over "
        f"{len(stems)} guard scopes, {len(prefix_families)} open prefix "
        f"families, and {len(retired) + len(retired_suffixes)} retired names.",
        "",
        "The `molt` CLI and the developer `RunContext` inspect the process "
        "environment on start. An unknown `MOLT_*` name prints a warning on "
        "stderr with close matches. A retired name is an error that names the "
        "replacement: Molt keeps no aliases.",
        "",
        "Value kinds: `bool` accepts `1`/`true`/`yes`/`on` and `0`/`false`/`no`/"
        "`off`; `path-list` and `list` use the platform path separator or "
        "commas as the reader documents; `enum` lists its values.",
        "",
        "## Contents",
        "",
    ]
    for audience in AUDIENCES:
        anchor = _AUDIENCE_TITLES[audience].lower().replace(" ", "-")
        out.append(f"- [{_AUDIENCE_TITLES[audience]}](#{anchor}) ({counts[audience]})")
    out.append(
        f"- [Guard scopes and families](#guard-scopes-and-families) ({len(families)} families, {len(stems)} scopes)"
    )
    out.append(f"- [Prefix families](#prefix-families) ({len(prefix_families)})")
    out.append(
        f"- [Retired names](#retired-names) ({len(retired) + len(retired_suffixes)})"
    )
    out.append("")

    for audience in AUDIENCES:
        rows = [v for v in variables if v["audience"] == audience]
        out.extend(
            [f"## {_AUDIENCE_TITLES[audience]}", "", _AUDIENCE_INTROS[audience], ""]
        )
        if rows:
            out.extend(_variable_table(rows))
        else:
            out.append("_None._")
        out.append("")

    out.extend(
        [
            "## Guard scopes and families",
            "",
            "Family names are built at run time as `<STEM><SUFFIX>`. The stem "
            "is the guard scope a Molt tool runs its subprocesses under; the "
            "scope-specific name wins over the root fallback when both are set.",
            "",
            "### Families",
            "",
            "| Family | Kind | Default | Root fallback | Read by | Summary |",
            "|---|---|---|---|---|---|",
        ]
    )
    for row in families:
        out.append(
            "| "
            + " | ".join(
                (
                    _code(f"MOLT_<STEM>{row['suffix']}"),
                    _cell(_kind_cell(row["kind"], row["values"])),
                    _cell(_default_cell(row["default"])),
                    _code(row["root_fallback"]) if row["root_fallback"] else "none",
                    _code(row["owner"]),
                    _cell(row["summary"]),
                )
            )
            + " |"
        )
    out.extend(
        [
            "",
            "### Guard scopes (stems)",
            "",
            "| Stem | Used by | Summary |",
            "|---|---|---|",
        ]
    )
    for row in stems:
        out.append(
            f"| {_code(row['name'])} | {_code(row['owner'])} | {_cell(row['summary'])} |"
        )
    out.extend(
        [
            "",
            "## Prefix families",
            "",
            "Names built from a fixed prefix and an open member set.",
            "",
            "| Family | Kind | Audience | Read by | Summary |",
            "|---|---|---|---|---|",
        ]
    )
    for row in prefix_families:
        out.append(
            f"| {_code(row['prefix'] + '<MEMBER>')} | {row['kind']} | {row['audience']} | "
            f"{_code(row['owner'])} | {_cell(row['summary'])} |"
        )
    out.extend(
        [
            "",
            "## Retired names",
            "",
            "Setting a retired name is an error. Set the replacement instead.",
            "",
            "| Retired | Replacement | Since |",
            "|---|---|---|",
        ]
    )
    for row in retired:
        advice = _code(row["replacement"]) if row["replacement"] else _cell(row["note"])
        out.append(f"| {_code(row['name'])} | {advice} | {row['retired']} |")
    for row in retired_suffixes:
        out.append(
            f"| {_code('MOLT_<STEM>' + row['suffix'])} | "
            f"{_code('MOLT_<STEM>' + row['replacement_suffix'])} | {row['retired']} |"
        )
    out.append("")
    return "\n".join(out)


def generated_outputs() -> dict[Path, str]:
    """Each output path mapped to its exact generated text."""
    payload = build_payload(load_source())
    return {OUT_PY: render_python(payload), OUT_DOC: render_doc(payload)}


def projection_is_current() -> bool:
    """True when both outputs match the TOML authority (used by the gate)."""

    return not stale_outputs(generated_outputs())


def main(argv: list[str] | None = None) -> int:
    try:
        return generator_main(generated_outputs, argv, description=__doc__)
    except RegistryFormatError as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
