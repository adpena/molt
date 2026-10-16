"""Stable identity bits and one acquisition/mutation authority."""

from __future__ import annotations

import hashlib
import json
from pathlib import Path

import pytest

from molt.compiler_analysis import python_identity_catalog_generated as catalog
from molt.compiler_analysis.python_binding_flow import (
    PythonBindingPolicy,
    analyze_python_source_bindings,
    python_canonical_member_writes,
)
from tools import gen_python_identity_catalog as gen


def test_generated_catalog_is_current() -> None:
    for path, text in gen.render_all(gen.load_schema()).items():
        assert path.read_bytes() == text.encode()


def test_existing_identity_and_member_bits_remain_stable() -> None:
    # Pin the complete pre-migration prefixes, including both identity
    # sentinels. Higher-bit additions do not change these independent hashes.
    for enum, width, expected in (
        (
            catalog.PythonIdentity,
            49,
            "2166f7ce86a539acfceb2cd5cad65ad892285a4441ad9762973e30d29524e255",
        ),
        (
            catalog.PythonMember,
            34,
            "77af2bf22160345d0b83d53c3cd74cabde62877d47a95aa0990c0d7b7017b9be",
        ),
    ):
        actual = {item.name: int(item) for item in enum}
        # Exercise append tolerance as data, without registering an unused
        # identity in the production catalog.
        for assignments in (actual, actual | {"future_append": 1 << width}):
            prefix = {
                name: mask for name, mask in assignments.items() if mask < 1 << width
            }
            assert (
                hashlib.sha256(json.dumps(prefix, sort_keys=True).encode()).hexdigest()
                == expected
            )


@pytest.mark.parametrize(
    ("change", "expected"),
    [
        (("BUILTIN_OPEN = 48", "BUILTIN_OPEN = 47"), "duplicate identity bit"),
        (("IMPORTLIB_MODULE = 0", "IMPORTLIB_MODULE = true"), "bit must be"),
        (("schema_version = 1", "schema_version = 1\nunknown = 1"), "top-level keys"),
        (('guard = "BUILTINS_IMPORT"', 'guard = "MISSING"'), "unknown builtin guard"),
        (
            ('owner = "IMPORTLIB_MODULE"', 'owner = "MISSING"'),
            "unknown acquisition owner",
        ),
        (("shape = false", 'shape = "false"'), "shape must be boolean"),
        (
            ('guards = ["MODULE_SPEC_CLASS"]', 'guards = ["MISSING"]'),
            "unknown write guard",
        ),
    ],
)
def test_catalog_rejects_invalid_declarations(tmp_path: Path, change, expected) -> None:
    before, after = change
    source = gen.SOURCE.read_text(encoding="utf-8")
    assert before in source
    path = tmp_path / "invalid.toml"
    path.write_text(source.replace(before, after, 1), encoding="utf-8")
    with pytest.raises(gen.SchemaError, match=expected):
        gen.load_schema(path)


def test_catalog_rejects_ambiguous_shape_identity_and_orphan_guard(
    tmp_path: Path,
) -> None:
    source = gen.SOURCE.read_text(encoding="utf-8")
    ambiguous = tmp_path / "ambiguous.toml"
    ambiguous.write_text(
        source.replace('identity = "BUILTIN_INT"', 'identity = "BUILTIN_BOOL"', 1),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="ambiguous shape identity"):
        gen.load_schema(ambiguous)
    orphan = tmp_path / "orphan.toml"
    orphan.write_text(
        source.replace("[member_bits]", "[member_bits]\nORPHAN = 34", 1),
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="orphan member guards"):
        gen.load_schema(orphan)


def test_catalog_rejects_duplicate_and_contradictory_acquisitions(
    tmp_path: Path,
) -> None:
    source = gen.SOURCE.read_text(encoding="utf-8")
    for guard in ("IMPORTLIB_IMPORT_MODULE", "BUILTINS_IMPORT"):
        path = tmp_path / f"{guard}.toml"
        path.write_text(
            source
            + (
                '\n[[acquisition]]\nowner = "IMPORTLIB_MODULE"\nname = "import_module"\n'
                f'identity = "IMPORTLIB_IMPORT_MODULE"\nguard = "{guard}"\n'
            ),
            encoding="utf-8",
        )
        with pytest.raises(
            gen.SchemaError, match="duplicate or contradictory acquisition"
        ):
            gen.load_schema(path)


@pytest.mark.parametrize(
    ("module", "member", "identity", "guard"),
    [
        (
            "builtins",
            "len",
            catalog.PythonIdentity.BUILTIN_LEN,
            catalog.PythonMember.BUILTINS_LEN,
        ),
        (
            "importlib",
            "import_module",
            catalog.PythonIdentity.IMPORTLIB_IMPORT_MODULE,
            catalog.PythonMember.IMPORTLIB_IMPORT_MODULE,
        ),
        (
            "typing_extensions",
            "TYPE_CHECKING",
            catalog.PythonIdentity.STATIC_FALSE,
            catalog.PythonMember.TYPING_TYPE_CHECKING,
        ),
    ],
)
def test_real_acquisition_and_named_replacement_share_the_guard(
    module, member, identity, guard
) -> None:
    source = (
        f"import {module} as owner\nbefore = owner.{member}\n"
        f"owner.{member} = replacement\nafter = owner.{member}\n"
    )
    index = analyze_python_source_bindings(
        source,
        policy=PythonBindingPolicy(
            foreign_namespace_writes=(), callback_member_writes=0
        ),
    )
    # Source-line facts select actual attribute reads on either side of the
    # assignment. They do not query a rendered table to decide the answer.
    reads = {
        fact.node.lineno: fact
        for fact in index.expressions
        if fact.node.kind == "Attribute"
    }
    assert reads[2].identities & int(identity)
    assert not reads[4].identities & int(identity)
    assert python_canonical_member_writes(module, member) & int(guard)
