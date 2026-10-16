"""Stable identity bits and one acquisition/mutation authority."""

from __future__ import annotations

import ast
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
from molt.compiler_analysis.python_value_identity import (
    canonical_import_binding_candidates,
    canonical_import_binding_write_mask,
)
from tools import gen_python_identity_catalog as gen


@pytest.mark.parametrize(
    ("name", "invalidates_imports"),
    [(None, True), ("typing", True), ("sys", True), ("os.path", False), ("", False)],
)
def test_import_cache_entry_mutation_preserves_the_cache_object(
    name: str | None, invalidates_imports: bool
) -> None:
    mask = canonical_import_binding_write_mask(name)
    assert bool(mask & int(catalog.PythonMember.IMPORT_HOOKS)) is invalidates_imports
    assert not mask & int(catalog.PythonMember.SYS_MODULES)


@pytest.mark.parametrize(
    ("expression", "expected"),
    [
        ('"typing"', ("typing",)),
        ('"os.path"', ("os.path",)),
        ('f"os.path"', ("os.path",)),
        ('f""', ("",)),
        ('f"{name}.path"', ()),
        ('f"typ{name}ing"', ("typing",)),
        ('f"sys{name}sys"', ()),
        ("left + right", (None,)),
        ("format_key()", (None,)),
    ],
)
def test_import_cache_key_bounds_keep_unknown_expressions_conservative(
    expression: str, expected: tuple[str | None, ...]
) -> None:
    node = ast.parse(expression, mode="eval").body
    assert canonical_import_binding_candidates(node) == expected


def test_disjoint_formatted_key_does_not_prove_callback_freedom() -> None:
    effects: list[str] = []

    class Key:
        def __format__(self, spec: str) -> str:
            effects.append(spec)
            return "typing"

    expression = 'f"{key:provider}.path"'
    assert eval(expression, {"key": Key()}) == "typing.path"
    assert effects == ["provider"]
    assert (
        canonical_import_binding_candidates(ast.parse(expression, mode="eval").body)
        == ()
    )


def test_generated_catalog_is_current() -> None:
    for path, text in gen.render_all(gen.load_schema()).items():
        assert path.read_bytes() == text.encode()


def test_target_property_callable_catalog_has_exact_manifest_producers() -> None:
    schema = gen.load_schema()
    assert schema.target_properties == (
        ("SYS_PLATFORM", "sys_platform"),
        ("OS_NAME", "os_name"),
    )
    assert schema.intrinsic_callables == (
        ("molt_sys_platform", "INTRINSIC_SYS_PLATFORM", "SYS_PLATFORM"),
        ("molt_os_name", "INTRINSIC_OS_NAME", "OS_NAME"),
    )
    assert catalog.INTRINSIC_CALLABLE_IDENTITIES["molt_sys_platform"] is (
        catalog.PythonIdentity.INTRINSIC_SYS_PLATFORM
    )
    assert (
        catalog.INTRINSIC_TARGET_PROPERTIES[
            int(catalog.PythonIdentity.INTRINSIC_OS_NAME)
        ]
        is catalog.PythonTargetProperty.OS_NAME
    )


@pytest.mark.parametrize(
    "before,after,expected",
    [
        (
            'target_property = "SYS_PLATFORM"',
            'target_property = "SYS_PLATFROM"',
            "unknown target property",
        ),
        (
            'name = "molt_sys_platform"',
            'name = "molt_missing_target_property"',
            "no manifest producer",
        ),
        (
            'name = "molt_sys_platform"',
            'name = "molt_locale_setlocale"',
            "must take no arguments",
        ),
        (
            'identity = "INTRINSIC_OS_NAME"',
            'identity = "INTRINSIC_SYS_PLATFORM"',
            "ambiguous intrinsic identity",
        ),
        (
            'identity = "INTRINSIC_OS_NAME"',
            'identity = "BUILTIN_INT"',
            "ambiguous intrinsic identity",
        ),
        (
            'OS_NAME = "os_name"',
            'OS_NAME = "sys_platform"',
            "duplicate target property value",
        ),
        (
            'OS_NAME = "os_name"',
            'OS_NAME = "os_name"\nUNUSED = "unused"',
            "orphan target properties",
        ),
    ],
)
def test_target_property_catalog_rejects_unproven_declarations(
    tmp_path: Path, before: str, after: str, expected: str
) -> None:
    source = gen.SOURCE.read_text(encoding="utf-8")
    assert before in source
    path = tmp_path / "invalid.toml"
    path.write_text(source.replace(before, after, 1), encoding="utf-8")
    with pytest.raises(gen.SchemaError, match=expected):
        gen.load_schema(path)


def test_target_property_catalog_rejects_duplicate_intrinsic_row(
    tmp_path: Path,
) -> None:
    path = tmp_path / "duplicate.toml"
    path.write_text(
        gen.SOURCE.read_text(encoding="utf-8")
        + '\n[[intrinsic_callable]]\nname = "molt_sys_platform"\n'
        'identity = "INTRINSIC_SYS_PLATFORM"\ntarget_property = "SYS_PLATFORM"\n',
        encoding="utf-8",
    )
    with pytest.raises(gen.SchemaError, match="duplicate intrinsic callable"):
        gen.load_schema(path)


def test_target_property_catalog_detects_removed_manifest_producer(
    tmp_path: Path,
) -> None:
    source = gen.INTRINSIC_MANIFEST.read_text(encoding="utf-8")
    declaration = "def molt_sys_platform() -> str: ..."
    assert declaration in source
    manifest = tmp_path / "manifest.pyi"
    manifest.write_text(source.replace(declaration, "", 1), encoding="utf-8")
    with pytest.raises(gen.SchemaError, match="no manifest producer"):
        gen.load_schema(intrinsic_manifest=manifest)


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
