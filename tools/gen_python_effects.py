#!/usr/bin/env python3
"""Generate the cross-language Python effect lattice from one TOML authority.

The generated hot-path representation is a raw integer bitmask in both Python
and Rust. Join is bitwise OR, zero is bottom, and the mask containing every
declared bit is top/unknown. Capability projections fail closed for masks that
contain bits unknown to the current schema.

Usage::

    python tools/gen_python_effects.py
    python tools/gen_python_effects.py --check
"""

from __future__ import annotations

import argparse
import re
import shutil
import subprocess
import sys
import tomllib
from dataclasses import dataclass
from pathlib import Path
from typing import cast

from generator_io import generated_file_matches, write_generated_text

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "runtime" / "python_effects.toml"
OUT_PYTHON = ROOT / "src" / "molt" / "compiler_analysis" / "python_effects_generated.py"
OUT_RUST = ROOT / "runtime" / "molt-ir" / "src" / "python_effects_generated.rs"
OUTPUTS = (OUT_PYTHON, OUT_RUST)

_IDENTIFIER = re.compile(r"^[a-z][a-z0-9_]*$")
_MASK_WIDTHS = {8, 16, 32, 64}


class SchemaError(ValueError):
    """The checked-in effect authority is internally inconsistent."""


@dataclass(frozen=True)
class Effect:
    name: str
    bit: int
    description: str

    @property
    def mask(self) -> int:
        return 1 << self.bit


@dataclass(frozen=True)
class Capability:
    name: str
    description: str
    forbidden: tuple[str, ...]
    forbidden_mask: int


@dataclass(frozen=True)
class Schema:
    mask_bits: int
    effects: tuple[Effect, ...]
    capabilities: tuple[Capability, ...]

    @property
    def all_effects(self) -> int:
        return (1 << len(self.effects)) - 1


def _table_rows(value: object, context: str) -> list[dict[str, object]]:
    if not isinstance(value, list) or not value:
        raise SchemaError(f"{context} must be a non-empty array of tables")
    if not all(isinstance(row, dict) for row in value):
        raise SchemaError(f"{context} rows must be tables")
    return cast(list[dict[str, object]], value)


def _identifier(value: object, context: str) -> str:
    if not isinstance(value, str) or _IDENTIFIER.fullmatch(value) is None:
        raise SchemaError(f"{context} must be a snake_case identifier")
    return value


def _description(value: object, context: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise SchemaError(f"{context} must be a non-empty description")
    return value


def load_schema(path: Path = SOURCE) -> Schema:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if set(data) != {"schema_version", "mask_bits", "effect", "capability"}:
        raise SchemaError(
            "top-level keys must be schema_version, mask_bits, effect, capability"
        )
    if data["schema_version"] != 1:
        raise SchemaError("python-effect authority schema_version must be 1")
    mask_bits = data["mask_bits"]
    if not isinstance(mask_bits, int) or isinstance(mask_bits, bool):
        raise SchemaError("mask_bits must be an integer")
    if mask_bits not in _MASK_WIDTHS:
        raise SchemaError(f"mask_bits must be one of {sorted(_MASK_WIDTHS)}")

    effect_rows = _table_rows(data["effect"], "effect")
    effects: list[Effect] = []
    effect_names: set[str] = set()
    for index, row in enumerate(effect_rows):
        if set(row) != {"name", "bit", "description"}:
            raise SchemaError(
                f"effect[{index}] keys must be exactly name, bit, description"
            )
        name = _identifier(row["name"], f"effect[{index}].name")
        bit = row["bit"]
        if not isinstance(bit, int) or isinstance(bit, bool) or bit < 0:
            raise SchemaError(f"effect {name!r} bit must be a non-negative integer")
        if name in effect_names:
            raise SchemaError(f"duplicate effect name {name!r}")
        effect_names.add(name)
        effects.append(
            Effect(name, bit, _description(row["description"], f"effect {name}"))
        )
    actual_bits = [effect.bit for effect in effects]
    expected_bits = list(range(len(effects)))
    if actual_bits != expected_bits:
        raise SchemaError(
            "effect bits must be dense, source-ordered, and zero-based: "
            f"expected {expected_bits!r}, got {actual_bits!r}"
        )
    if len(effects) > mask_bits:
        raise SchemaError(
            f"{len(effects)} effect bits exceed the declared {mask_bits}-bit mask"
        )

    capabilities: list[Capability] = []
    capability_names: set[str] = set()
    for index, row in enumerate(_table_rows(data["capability"], "capability")):
        if set(row) != {"name", "description", "forbidden"}:
            raise SchemaError(
                f"capability[{index}] keys must be exactly name, description, forbidden"
            )
        name = _identifier(row["name"], f"capability[{index}].name")
        if name in capability_names:
            raise SchemaError(f"duplicate capability name {name!r}")
        capability_names.add(name)
        raw_forbidden = row["forbidden"]
        if not isinstance(raw_forbidden, list) or not raw_forbidden:
            raise SchemaError(f"capability {name!r} forbidden must be non-empty")
        if not all(isinstance(item, str) for item in raw_forbidden):
            raise SchemaError(f"capability {name!r} forbidden entries must be strings")
        forbidden = tuple(cast(str, item) for item in raw_forbidden)
        if len(forbidden) != len(set(forbidden)):
            raise SchemaError(f"capability {name!r} forbidden effects must be unique")
        unknown = set(forbidden) - effect_names
        if unknown:
            raise SchemaError(
                f"capability {name!r} references unknown effects {sorted(unknown)!r}"
            )
        forbidden_set = set(forbidden)
        ordered_forbidden = tuple(
            effect.name for effect in effects if effect.name in forbidden_set
        )
        if forbidden != ordered_forbidden:
            raise SchemaError(
                f"capability {name!r} forbidden effects must follow effect bit order"
            )
        forbidden_mask = sum(
            effect.mask for effect in effects if effect.name in forbidden_set
        )
        capabilities.append(
            Capability(
                name=name,
                description=_description(row["description"], f"capability {name}"),
                forbidden=forbidden,
                forbidden_mask=forbidden_mask,
            )
        )

    schema = Schema(mask_bits, tuple(effects), tuple(capabilities))
    masks_to_names: dict[int, list[str]] = {}
    for capability in schema.capabilities:
        masks_to_names.setdefault(capability.forbidden_mask, []).append(capability.name)
    duplicate_projections = [
        names for names in masks_to_names.values() if len(names) > 1
    ]
    if duplicate_projections:
        raise SchemaError(
            f"capabilities must not duplicate a projection: {duplicate_projections!r}"
        )
    transparent = next(
        (
            capability
            for capability in schema.capabilities
            if capability.name == "referentially_transparent"
        ),
        None,
    )
    if transparent is None or transparent.forbidden_mask != schema.all_effects:
        raise SchemaError("referentially_transparent must forbid every declared effect")
    return schema


def _constant(name: str) -> str:
    return name.upper()


def _python_string(value: str) -> str:
    return repr(value)


def _rust_string(value: str) -> str:
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'


def render_python(schema: Schema) -> str:
    lines = [
        "# @generated by tools/gen_python_effects.py from\n",
        "# runtime/python_effects.toml. DO NOT EDIT.\n",
        '"""Raw-mask Python effect lattice shared by compiler analyses."""\n\n',
        "from __future__ import annotations\n\n",
        "from typing import Final, TypeAlias\n\n",
        "EffectMask: TypeAlias = int\n",
        f"EFFECT_MASK_BITS: Final = {schema.mask_bits}\n",
        "NO_EFFECTS: Final[EffectMask] = 0\n",
    ]
    for effect in schema.effects:
        lines.append(
            f"{_constant(effect.name)}: Final[EffectMask] = 1 << {effect.bit}\n"
        )
    lines.extend(
        [
            f"ALL_EFFECTS: Final[EffectMask] = 0x{schema.all_effects:X}\n",
            "UNKNOWN_EFFECTS: Final[EffectMask] = ALL_EFFECTS\n\n",
            "EFFECT_ROWS: Final[tuple[tuple[str, EffectMask, str], ...]] = (\n",
        ]
    )
    for effect in schema.effects:
        lines.append(
            "    ("
            f"{_python_string(effect.name)}, {_constant(effect.name)}, "
            f"{_python_string(effect.description)}),\n"
        )
    lines.append(")\n\n")
    for capability in schema.capabilities:
        lines.append(
            f"{_constant(capability.name)}_FORBIDDEN_EFFECTS: Final[EffectMask] "
            f"= 0x{capability.forbidden_mask:X}\n"
        )
    lines.append(
        "\nCAPABILITY_ROWS: Final[tuple[tuple[str, EffectMask, str], ...]] = (\n"
    )
    for capability in schema.capabilities:
        lines.append(
            "    ("
            f"{_python_string(capability.name)}, "
            f"{_constant(capability.name)}_FORBIDDEN_EFFECTS, "
            f"{_python_string(capability.description)}),\n"
        )
    lines.extend(
        [
            ")\n\n",
            "def join_effects(left: EffectMask, right: EffectMask) -> EffectMask:\n",
            '    """Least upper bound. Internal hot paths intentionally use one OR."""\n',
            "    return left | right\n\n\n",
            "def effect_mask_is_known(mask: EffectMask) -> bool:\n",
            '    """Return whether *mask* contains only bits in this schema version."""\n',
            "    return mask >= 0 and mask & ~ALL_EFFECTS == 0\n\n\n",
            "def canonicalize_effects(mask: EffectMask) -> EffectMask:\n",
            '    """Fail closed at untrusted/versioned boundaries: unknown bits become top."""\n',
            "    return mask if effect_mask_is_known(mask) else UNKNOWN_EFFECTS\n\n\n",
            "def effect_mask_has_any(mask: EffectMask, projection: EffectMask) -> bool:\n",
            "    return bool(mask & projection)\n\n\n",
            "def effect_mask_has_all(mask: EffectMask, projection: EffectMask) -> bool:\n",
            "    return mask & projection == projection\n\n\n",
            "def effect_mask_satisfies_capability(\n",
            "    mask: EffectMask, forbidden_effects: EffectMask\n",
            ") -> bool:\n",
            '    """Capabilities fail closed for masks from a newer/invalid schema."""\n',
            "    return effect_mask_is_known(mask) and mask & forbidden_effects == 0\n",
        ]
    )
    return "".join(lines)


def render_rust(schema: Schema) -> str:
    rust_type = f"u{schema.mask_bits}"
    lines = [
        "// @generated by tools/gen_python_effects.py from\n",
        "// runtime/python_effects.toml. DO NOT EDIT.\n\n",
        "//! Raw-mask Python effect lattice shared by compiler analyses.\n\n",
        f"pub type PythonEffectMask = {rust_type};\n",
        f"pub const PYTHON_EFFECT_MASK_BITS: u32 = {schema.mask_bits};\n",
        "pub const NO_PYTHON_EFFECTS: PythonEffectMask = 0;\n",
    ]
    for effect in schema.effects:
        lines.append(
            f"pub const PYTHON_EFFECT_{_constant(effect.name)}: PythonEffectMask = "
            f"1 << {effect.bit};\n"
        )
    lines.extend(
        [
            f"pub const ALL_PYTHON_EFFECTS: PythonEffectMask = 0x{schema.all_effects:X};\n",
            "pub const UNKNOWN_PYTHON_EFFECTS: PythonEffectMask = ALL_PYTHON_EFFECTS;\n\n",
            f"pub const PYTHON_EFFECT_ROWS: [(&str, PythonEffectMask, &str); {len(schema.effects)}] = [\n",
        ]
    )
    for effect in schema.effects:
        lines.append(
            "    ("
            f"{_rust_string(effect.name)}, "
            f"PYTHON_EFFECT_{_constant(effect.name)}, "
            f"{_rust_string(effect.description)}),\n"
        )
    lines.append("];\n\n")
    for capability in schema.capabilities:
        lines.append(
            f"pub const PYTHON_CAPABILITY_{_constant(capability.name)}_FORBIDDEN_EFFECTS: "
            f"PythonEffectMask = 0x{capability.forbidden_mask:X};\n"
        )
    lines.extend(
        [
            "\n",
            f"pub const PYTHON_CAPABILITY_ROWS: [(&str, PythonEffectMask, &str); {len(schema.capabilities)}] = [\n",
        ]
    )
    for capability in schema.capabilities:
        lines.append(
            "    ("
            f"{_rust_string(capability.name)}, "
            f"PYTHON_CAPABILITY_{_constant(capability.name)}_FORBIDDEN_EFFECTS, "
            f"{_rust_string(capability.description)}),\n"
        )
    lines.extend(
        [
            "];\n\n",
            "#[inline(always)]\n",
            "pub const fn join_python_effects(\n",
            "    left: PythonEffectMask,\n",
            "    right: PythonEffectMask,\n",
            ") -> PythonEffectMask {\n",
            "    left | right\n",
            "}\n\n",
            "#[inline(always)]\n",
            "pub const fn python_effect_mask_is_known(mask: PythonEffectMask) -> bool {\n",
            "    mask & !ALL_PYTHON_EFFECTS == 0\n",
            "}\n\n",
            "#[inline(always)]\n",
            "pub const fn canonicalize_python_effects(mask: PythonEffectMask) -> PythonEffectMask {\n",
            "    if python_effect_mask_is_known(mask) {\n",
            "        mask\n",
            "    } else {\n",
            "        UNKNOWN_PYTHON_EFFECTS\n",
            "    }\n",
            "}\n\n",
            "#[inline(always)]\n",
            "pub const fn python_effect_mask_has_any(\n",
            "    mask: PythonEffectMask,\n",
            "    projection: PythonEffectMask,\n",
            ") -> bool {\n",
            "    mask & projection != 0\n",
            "}\n\n",
            "#[inline(always)]\n",
            "pub const fn python_effect_mask_has_all(\n",
            "    mask: PythonEffectMask,\n",
            "    projection: PythonEffectMask,\n",
            ") -> bool {\n",
            "    mask & projection == projection\n",
            "}\n\n",
            "#[inline(always)]\n",
            "pub const fn python_effect_mask_satisfies_capability(\n",
            "    mask: PythonEffectMask,\n",
            "    forbidden_effects: PythonEffectMask,\n",
            ") -> bool {\n",
            "    python_effect_mask_is_known(mask) && mask & forbidden_effects == 0\n",
            "}\n\n",
            "#[cfg(test)]\n",
            "mod tests {\n",
            "    use super::*;\n\n",
            "    #[test]\n",
            "    fn effect_lattice_obeys_join_laws() {\n",
            "        assert_eq!(UNKNOWN_PYTHON_EFFECTS, ALL_PYTHON_EFFECTS);\n",
            "        for (_, left, _) in PYTHON_EFFECT_ROWS {\n",
            "            assert_eq!(join_python_effects(NO_PYTHON_EFFECTS, left), left);\n",
            "            assert_eq!(join_python_effects(left, left), left);\n",
            "            assert_eq!(join_python_effects(left, UNKNOWN_PYTHON_EFFECTS), UNKNOWN_PYTHON_EFFECTS);\n",
            "            for (_, right, _) in PYTHON_EFFECT_ROWS {\n",
            "                assert_eq!(join_python_effects(left, right), join_python_effects(right, left));\n",
            "                assert!(python_effect_mask_has_all(join_python_effects(left, right), left));\n",
            "                assert!(python_effect_mask_has_all(join_python_effects(left, right), right));\n",
            "            }\n",
            "        }\n",
            "    }\n\n",
            "    #[test]\n",
            "    fn capability_projection_is_monotone_and_unknown_fails_closed() {\n",
            "        for (_, forbidden, _) in PYTHON_CAPABILITY_ROWS {\n",
            "            assert!(python_effect_mask_satisfies_capability(NO_PYTHON_EFFECTS, forbidden));\n",
            "            assert!(!python_effect_mask_satisfies_capability(UNKNOWN_PYTHON_EFFECTS, forbidden));\n",
            "            for (_, effect, _) in PYTHON_EFFECT_ROWS {\n",
            "                let expected = effect & forbidden == 0;\n",
            "                assert_eq!(python_effect_mask_satisfies_capability(effect, forbidden), expected);\n",
            "            }\n",
            "        }\n",
            "        let future_bit = 1 << PYTHON_EFFECT_MASK_BITS.saturating_sub(1);\n",
            "        assert!(!python_effect_mask_is_known(future_bit));\n",
            "        assert_eq!(canonicalize_python_effects(future_bit), UNKNOWN_PYTHON_EFFECTS);\n",
            "    }\n",
            "}\n",
        ]
    )
    return "".join(lines)


def _format_python(source: str) -> str:
    completed = subprocess.run(
        [
            sys.executable,
            "-m",
            "ruff",
            "format",
            "-",
            "--stdin-filename",
            str(OUT_PYTHON),
        ],
        cwd=ROOT,
        input=source,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"ruff format failed:\n{completed.stderr}")
    return completed.stdout


def _format_rust(source: str) -> str:
    rustfmt = shutil.which("rustfmt")
    if rustfmt is None:
        raise RuntimeError("rustfmt is required to generate Python-effect Rust")
    completed = subprocess.run(
        [rustfmt, "--edition", "2024", "--emit", "stdout"],
        cwd=ROOT,
        input=source,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        raise RuntimeError(f"rustfmt failed:\n{completed.stderr}")
    return completed.stdout


def render_all(schema: Schema) -> dict[Path, str]:
    return {
        OUT_PYTHON: _format_python(render_python(schema)),
        OUT_RUST: _format_rust(render_rust(schema)),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check", action="store_true", help="fail if generated outputs are stale"
    )
    args = parser.parse_args(argv)
    stale = False
    for path, source in render_all(load_schema()).items():
        if args.check:
            if not generated_file_matches(path, source):
                print(
                    f"STALE generated file: {path.relative_to(ROOT)}", file=sys.stderr
                )
                stale = True
        else:
            write_generated_text(path, source)
    return int(stale)


if __name__ == "__main__":
    raise SystemExit(main())
