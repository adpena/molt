#!/usr/bin/env python3
"""Generate closed runtime name identities and their isolate-owned storage (HF-653)."""

from __future__ import annotations

import json
import re
import sys
import tomllib
from pathlib import Path

_REPO_ROOT = Path(__file__).resolve().parents[1]
if str(_REPO_ROOT) not in sys.path:
    sys.path.insert(0, str(_REPO_ROOT))

from tools.import_file import bind_repository_imports  # noqa: E402

bind_repository_imports(__file__)

ROOT = _REPO_ROOT

from molt.rust_source_scan import (  # noqa: E402
    mask_rust_comments_and_strings,
    mask_rust_test_items,
    rust_test_only_source_files,
    rust_source_tokens,
)
from tools.generator_io import generator_main, rustfmt_source  # noqa: E402

TABLE = ROOT / "runtime/molt-ir/src/tir/runtime_names.toml"
OUTPUT = ROOT / "runtime/molt-runtime/src/state/names_generated.rs"


def load_names(path: Path = TABLE) -> dict[str, str]:
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    if set(data) != {"names"} or not isinstance(data["names"], dict):
        raise ValueError("runtime_names.toml must contain only [names]")
    names = data["names"]
    seen = set()
    for identity, spelling in names.items():
        if not re.fullmatch(r"[A-Z][A-Za-z0-9]*", identity):
            raise ValueError(f"invalid NameId: {identity}")
        if (
            not isinstance(spelling, str)
            or not spelling.isascii()
            or not all(part.isidentifier() for part in spelling.split("."))
        ):
            raise ValueError(f"invalid runtime spelling: {identity}")
        if spelling in seen:
            raise ValueError(f"duplicate runtime spelling: {spelling}")
        seen.add(spelling)
    if not names:
        raise ValueError("runtime name registry is empty")
    return dict(sorted(names.items()))


def reject_spelled_consumers(source: str, path: Path) -> None:
    """Reject literal names at migrated APIs; dynamic input stays at its boundary."""
    positions = {
        "dynamic_attr_name_bits": 1,
        "attr_name_bits_from_bytes": 1,
        "lookup_special_method": 2,
        "class_namespace_lookup_mro_pure": 2,
        "has_special_method": 2,
        "invoke_special_method": 2,
        "lookup_async_special_method": 2,
        "pickle_attr_optional": 2,
        "pickle_attr_required": 3,
        "set_class_method": 2,
    }
    tokens = rust_source_tokens(source)
    for index, token in enumerate(tokens[:-1]):
        if tokens[index + 1].text != "(" or (index and tokens[index - 1].text == "fn"):
            continue
        position = positions.get(token.text)
        if (
            token.text.endswith("_method_bits")
            or token.text == "exception_method_bits_for_owner"
        ):
            position = -1
        if token.text in {"declared", "fixed", "with_signature"}:
            prefix = source[max(0, token.start - 40) : token.start]
            if re.search(r"NativeCallableSpec\s*::\s*$", prefix):
                position = 2
            elif re.search(r"RuntimeClassMethodSpec\s*::\s*$", prefix):
                position = 0
        if position is None:
            continue
        arguments = []
        argument = []
        depth = 0
        for item in tokens[index + 2 :]:
            if item.text == ")" and depth == 0:
                if argument:
                    arguments.append(argument)
                break
            if item.text in {"(", "[", "{"}:
                depth += 1
            elif item.text in {")", "]", "}"}:
                depth -= 1
            if item.text == "," and depth == 0:
                arguments.append(argument)
                argument = []
            else:
                argument.append(item.text)
        if -len(arguments) <= position < len(arguments):
            selected = arguments[position]
            if len(selected) == 1 and re.fullmatch(
                r'b?"[A-Za-z_][A-Za-z_0-9.]*"', selected[0]
            ):
                raise ValueError(
                    f"byte-spelled runtime name lookup in {path}: {token.text}"
                )

    for index, token in enumerate(tokens[:-2]):
        if (
            token.text not in {"native_method_table", "suspension_methods"}
            or tokens[index + 1].text != "!"
        ):
            continue
        depth = 0
        for cursor in range(index + 2, len(tokens)):
            item = tokens[cursor]
            if item.text in {"(", "[", "{"}:
                depth += 1
            elif item.text in {")", "]", "}"}:
                depth -= 1
                if depth == 0:
                    break
            if depth == 2 and re.fullmatch(r'"[A-Za-z_][A-Za-z_0-9]*"', item.text):
                following = [part.text for part in tokens[cursor + 1 : cursor + 3]]
                if following == ["=", ">"] or following[:1] == ["if"]:
                    raise ValueError(
                        f"byte-spelled runtime name lookup in {path}: native table"
                    )


def census(names: dict[str, str], root: Path = ROOT) -> None:
    """Only production typed references are consumers; generated rows prove nothing."""
    paths = sorted((root / "runtime/molt-runtime/src").rglob("*.rs"))
    test_files = rust_test_only_source_files(paths)
    consumed = set()
    for path in paths:
        if path == root / OUTPUT.relative_to(ROOT) or path in test_files:
            continue
        source = mask_rust_test_items(path.read_text(encoding="utf-8"))
        reject_spelled_consumers(source, path)
        code = mask_rust_comments_and_strings(source)
        consumed.update(re.findall(r"\bNameId\s*::\s*([A-Z][A-Za-z0-9]*)\b", code))
        # These entry points accept typed names after the static-name migration.
        spelled = mask_rust_comments_and_strings(source, preserve_literals=True)
        if re.search(
            r"\b(?:runtime_static_name_slot|intern_runtime_static_name|intern_static_name)\s*\(",
            spelled,
        ):
            raise ValueError(f"retired byte-spelled static name lookup in {path}")
    missing = set(names) - consumed
    if missing:
        raise ValueError(
            "registered runtime names without consumers: " + ", ".join(sorted(missing))
        )


def render(names: dict[str, str]) -> str:
    lines = [
        "// @generated by tools/gen_runtime_names.py; do not edit.",
        "use std::sync::atomic::AtomicU64;",
        "use std::sync::Mutex;",
        "#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]",
        "#[repr(u16)]",
        "pub(crate) enum NameId {",
        *(f"    {identity}," for identity in names),
        "}",
        "impl NameId {",
        f"    pub(crate) const ALL: [Self; {len(names)}] = [",
        *(f"        Self::{identity}," for identity in names),
        "    ];",
        "    pub(crate) const fn as_str(self) -> &'static str {",
        "        match self {",
        *(
            f"            Self::{identity} => {json.dumps(spelling)},"
            for identity, spelling in names.items()
        ),
        "        }",
        "    }",
        "    pub(crate) fn from_bytes(name: &[u8]) -> Option<Self> {",
        "        match name {",
        *(
            f"            b{json.dumps(spelling)} => Some(Self::{identity}),"
            for identity, spelling in names.items()
        ),
        "            _ => None,",
        "        }",
        "    }",
        "    #[inline]",
        "    pub(crate) fn slot(self, names: &RuntimeNames) -> &AtomicU64 {",
        "        &names.slots[self as usize]",
        "    }",
        "}",
        "/// Sorted observations of canonical objects retained by the generated slots.",
        "pub(crate) type CanonicalNameIndex = Vec<(u64, NameId)>;",
        "pub(crate) struct RuntimeNames {",
        "    pub(crate) index: Mutex<CanonicalNameIndex>,",
        "    slots: [AtomicU64; NameId::ALL.len()],",
        "}",
        "impl RuntimeNames {",
        "    pub(crate) fn new() -> Self { Self {",
        "        index: Mutex::new(Vec::new()),",
        "        slots: [const { AtomicU64::new(0) }; NameId::ALL.len()],",
        "    } }",
        "    pub(crate) fn slots(&self) -> Vec<&AtomicU64> {",
        "        self.slots.iter().collect()",
        "    }",
        "}",
    ]
    lines.extend(
        [
            "impl std::fmt::Display for NameId {",
            "    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {",
            "        formatter.write_str(self.as_str())",
            "    }",
            "}",
        ]
    )
    return rustfmt_source("\n".join(lines) + "\n", label="runtime names")


def generated_outputs() -> dict[Path, str]:
    names = load_names()
    census(names)
    return {OUTPUT: render(names)}


def main(argv: list[str] | None = None) -> int:
    return generator_main(generated_outputs, argv, description=__doc__)


if __name__ == "__main__":
    raise SystemExit(main())
