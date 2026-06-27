from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
GEN_CODECS = ROOT / "tools" / "gen_codecs.py"


def _load_gen_codecs():
    spec = importlib.util.spec_from_file_location("molt_test_gen_codecs", GEN_CODECS)
    assert spec is not None
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_charmap_codecs_generated_file_is_in_sync() -> None:
    gen = _load_gen_codecs()
    current = gen.OUT_RS.read_text(encoding="utf-8")
    assert current == gen.render()
    aliases_current = gen.OUT_ALIASES_RS.read_text(encoding="utf-8")
    assert aliases_current == gen.render_aliases()


def test_charmap_codec_tables_cover_declared_codecs() -> None:
    gen = _load_gen_codecs()
    rendered = gen.render()
    for codec in gen.CHARMAP_CODECS:
        assert f"EncodingKind::{codec.kind}" in rendered
        assert f"static {codec.rust_name}_DECODE_TABLE: [u16; 128]" in rendered
        assert f"static {codec.rust_name}_ENCODE_HIGH_TABLE: [u8; 128]" in rendered
        assert f"static {codec.rust_name}_ENCODE_EXTENDED_ENTRIES: [u32;" in rendered


def test_charmap_codec_set_is_derived_from_text_registry() -> None:
    gen = _load_gen_codecs()
    registry = gen.REGISTRY.read_text(encoding="utf-8")
    assert tuple(codec.kind for codec in gen.CHARMAP_CODECS) == gen._charmap_kinds(registry)


def test_charmap_codec_tables_are_ascii_compatible() -> None:
    gen = _load_gen_codecs()
    for codec in gen.CHARMAP_CODECS:
        table = gen._raw_decoding_table(codec.module)
        assert all(ord(ch) == idx for idx, ch in enumerate(table[:0x80]))


def test_generated_aliases_cover_supported_cpython_static_aliases() -> None:
    gen = _load_gen_codecs()
    aliases = dict(gen._alias_entries())
    module_to_kind = {
        descriptor.module: descriptor.kind for descriptor in gen.CODEC_DESCRIPTORS
    }
    for alias_name, module_name in gen._static_aliases().items():
        kind = module_to_kind.get(module_name)
        if kind is None:
            continue
        assert aliases[gen._encoding_search_key(alias_name)] == kind


def test_generated_python_aliases_cover_local_encoding_modules() -> None:
    gen = _load_gen_codecs()
    aliases = dict(gen._python_alias_entries())
    available_modules = {
        path.stem
        for path in gen.ENCODINGS.glob("*.py")
        if path.name not in {"__init__.py", "aliases.py"}
    }
    for alias_name, module_name in gen._static_aliases().items():
        if module_name not in available_modules:
            continue
        assert aliases[gen._encoding_search_key(alias_name)] == module_name
    for alias_name in ("base64", "bz2", "hex", "quopri", "uu", "zip", "zlib"):
        assert gen._encoding_search_key(alias_name) in aliases


def test_codec_registry_reexports_generated_aliases() -> None:
    registry = ROOT / "runtime/molt-runtime-text/src/codec_registry.rs"
    text = registry.read_text(encoding="utf-8")
    assert "pub use crate::codec_aliases_generated::ENCODING_ALIASES;" in text
    assert "pub use crate::codec_aliases_generated::PYTHON_ENCODING_ALIASES;" in text
    assert "pub const ENCODING_ALIASES: &[EncodingAlias]" not in text


def test_runtime_no_longer_owns_charmap_tables() -> None:
    stale = ROOT / "runtime/molt-runtime/src/object/ops_encoding/charmap_codecs.rs"
    assert not stale.exists()
