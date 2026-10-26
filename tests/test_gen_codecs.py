from __future__ import annotations

import ast
import hashlib
import json
import runpy
import sys
import tomllib
import types
from pathlib import Path

import pytest

from tools import gen_codecs

ROOT = Path(__file__).resolve().parents[1]


def test_charmap_codecs_generated_file_is_in_sync() -> None:
    gen = gen_codecs
    current = gen.OUT_RS.read_text(encoding="utf-8")
    assert current == gen.render()
    aliases_current = gen.OUT_ALIASES_RS.read_text(encoding="utf-8")
    assert aliases_current == gen.render_aliases()


def test_charmap_codec_tables_cover_declared_codecs() -> None:
    gen = gen_codecs
    rendered = gen.render()
    for codec in gen.CHARMAP_CODECS:
        assert f"EncodingKind::{codec.kind}" in rendered
        assert f"static {codec.rust_name}_DECODE_TABLE: [u16; 128]" in rendered
        assert f"static {codec.rust_name}_ENCODE_HIGH_TABLE: [u8; 128]" in rendered
        assert f"static {codec.rust_name}_ENCODE_EXTENDED_ENTRIES: [u32;" in rendered


def test_charmap_codec_set_is_derived_from_text_registry() -> None:
    gen = gen_codecs
    registry = gen.REGISTRY.read_text(encoding="utf-8")
    assert tuple(codec.kind for codec in gen.CHARMAP_CODECS) == gen._charmap_kinds(
        registry
    )


def test_charmap_codec_tables_are_ascii_compatible() -> None:
    gen = gen_codecs
    for codec in gen.CHARMAP_CODECS:
        table = gen._raw_decoding_table(codec.module)
        assert all(ord(ch) == idx for idx, ch in enumerate(table[:0x80]))


def test_generated_aliases_cover_supported_reference_aliases() -> None:
    gen = gen_codecs
    aliases = dict(gen._alias_entries())
    module_to_kind = {
        descriptor.module: descriptor.kind for descriptor in gen.CODEC_DESCRIPTORS
    }
    for alias_name, module_name in gen._load_aliases().items():
        kind = module_to_kind.get(module_name)
        if kind is None:
            continue
        assert aliases[gen._encoding_search_key(alias_name)] == kind


def test_generated_python_aliases_cover_local_encoding_modules() -> None:
    gen = gen_codecs
    aliases = dict(gen._python_alias_entries())
    available_modules = {
        path.stem
        for path in gen.ENCODINGS.glob("*.py")
        if path.name not in {"__init__.py", "aliases.py"}
    }
    for alias_name, module_name in gen._load_aliases().items():
        if module_name not in available_modules:
            continue
        assert aliases[gen._encoding_search_key(alias_name)] == module_name
    for alias_name in ("base64", "bz2", "hex", "quopri", "uu", "zip", "zlib"):
        assert gen._encoding_search_key(alias_name) in aliases


def test_codec_registry_reexports_generated_aliases() -> None:
    registry = ROOT / "runtime/molt-stdlib-text/src/codec_registry.rs"
    text = registry.read_text(encoding="utf-8")
    assert "pub use crate::codec_aliases_generated::ENCODING_ALIASES;" in text
    assert "pub use crate::codec_aliases_generated::PYTHON_ENCODING_ALIASES;" in text
    assert "pub const ENCODING_ALIASES: &[EncodingAlias]" not in text


def test_runtime_no_longer_owns_charmap_tables() -> None:
    stale = ROOT / "runtime/molt-runtime/src/object/ops_encoding/charmap_codecs.rs"
    assert not stale.exists()

    consumer = (ROOT / "runtime/molt-runtime/src/object/ops_encoding.rs").read_text(
        encoding="utf-8"
    )
    assert "molt_stdlib_text::charmap_codecs_generated" in consumer
    assert "mod charmap_codecs" not in consumer
    assert "crate::object::ops_encoding::charmap_codecs" not in consumer


def test_reference_aliases_preserve_the_complete_migrated_table() -> None:
    # Independent pre-migration oracle: all reference rows, including aliases
    # filtered out of the current runtime's supported codec/module subsets.
    aliases = tomllib.loads(gen_codecs.ALIAS_DATA.read_text(encoding="utf-8"))[
        "aliases"
    ]
    canonical = json.dumps(aliases, sort_keys=True, separators=(",", ":")).encode()
    assert len(aliases) == 326
    assert (
        hashlib.sha256(canonical).hexdigest()
        == "859b741b06f6dbcb646b31d1ccd5307411ea3b32c716ea1414fde69d53c00e1b"
    )


def test_generated_alias_table_bytes_preserve_the_migrated_projection() -> None:
    # Only the generated source-provenance comment changes in this migration.
    generated = gen_codecs.OUT_ALIASES_RS.read_bytes()
    header, body = generated.split(b"\n", 1)
    assert b"config/codec_aliases.toml" in header
    assert (
        hashlib.sha256(body).hexdigest()
        == "9ba8fa9f66c5955cd88cd69d4e4fb8e0689661159f746247f986f22678893cff"
    )


@pytest.mark.parametrize(
    "source",
    [
        "",
        'aliases = "wrong shape"\n',
        "[aliases]\n",
        '[extra]\nkey = "utf_8"\n[aliases]\nu8 = "utf_8"\n',
        "[aliases]\nu8 = 8\n",
        '[aliases]\nu8 = ""\n',
        '[aliases]\n"" = "utf_8"\n',
        '[aliases]\nu8 = "utf_8"\nu8 = "ascii"\n',
    ],
)
def test_alias_data_refuses_invalid_documents(tmp_path, monkeypatch, source) -> None:
    path = tmp_path / "aliases.toml"
    path.write_text(source, encoding="utf-8")
    monkeypatch.setattr(gen_codecs, "ALIAS_DATA", path)
    with pytest.raises(ValueError):
        gen_codecs._load_aliases()


def test_both_alias_projections_read_the_declarative_input(
    tmp_path, monkeypatch
) -> None:
    path = tmp_path / "aliases.toml"
    path.write_text('[aliases]\nprivate_probe = "utf_8"\n', encoding="utf-8")
    monkeypatch.setattr(gen_codecs, "ALIAS_DATA", path)
    assert dict(gen_codecs._alias_entries())["private_probe"] == "Utf8"
    assert dict(gen_codecs._python_alias_entries()) == {"private_probe": "utf_8"}


def test_runtime_aliases_constructs_only_the_intrinsic_mapping(monkeypatch) -> None:
    runtime_source = ROOT / "src/molt/stdlib/encodings/aliases.py"
    # A copied reference dict is executable work even if its name is private.
    tree = ast.parse(runtime_source.read_text(encoding="utf-8"))
    assert not any(isinstance(node, ast.Dict) for node in ast.walk(tree))
    calls: list[str] = []
    supplied = {"independent_probe": "private_codec"}

    def resolve(name):
        calls.append(name)
        assert name == "molt_encodings_aliases_map"
        return lambda: supplied

    intrinsics = types.ModuleType("_intrinsics")
    intrinsics.require_intrinsic = resolve
    monkeypatch.setitem(sys.modules, "_intrinsics", intrinsics)
    namespace = runpy.run_path(str(runtime_source))
    assert calls == ["molt_encodings_aliases_map"]
    assert namespace["aliases"] == supplied
    assert namespace["aliases"] is not supplied
    assert "_STATIC_ALIASES" not in namespace
