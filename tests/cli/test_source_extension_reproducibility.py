from __future__ import annotations

import json
from pathlib import Path

import pytest

from molt.cli.source_extension_reproducibility import (
    _canonical_extension_manifest_for_wheel,
    _canonicalize_locations,
    _canonicalize_meson_metadata,
    _source_extension_deterministic_path_args,
)
from molt.cli.source_extension_object_closure_schema import (
    SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
    SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
)
from molt.cli.source_extension_object_closure import (
    finalize_source_extension_object_closure,
)


def _flag_replacements(arguments: list[str]) -> list[str]:
    return [
        argument.split("=", 2)[2]
        for argument in arguments
        if argument.startswith("-ffile-prefix-map=")
    ]


def test_path_map_order_uses_semantic_authority_not_host_path_length(
    tmp_path: Path,
) -> None:
    short_source = tmp_path / "s"
    long_build = tmp_path / "a-build-directory-with-an-arbitrary-long-name"
    long_source = tmp_path / "a-source-directory-with-an-arbitrary-long-name"
    short_build = tmp_path / "b"

    first = _source_extension_deterministic_path_args(
        compiler_command=("clang",),
        roots=((short_source, ".molt/source"), (long_build, ".molt/build")),
    )
    second = _source_extension_deterministic_path_args(
        compiler_command=("clang",),
        roots=((long_source, ".molt/source"), (short_build, ".molt/build")),
    )

    assert _flag_replacements(first) == [".molt/source", ".molt/build"]
    assert _flag_replacements(second) == [".molt/source", ".molt/build"]


def test_path_map_refuses_ancestor_declared_before_descendant(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output"
    objects = output / "objects"

    with pytest.raises(ValueError, match=r"not in canonical order.*\.molt/objects"):
        _source_extension_deterministic_path_args(
            compiler_command=("clang",),
            roots=((output, ".molt/output"), (objects, ".molt/objects")),
        )


def test_path_map_order_is_the_declared_order_on_every_host_layout(
    tmp_path: Path,
) -> None:
    # The same roles must record the same path-map arguments whether or not
    # this host keeps the build root inside the checkout: the order is the
    # declared order, never a function of which roots happen to nest.
    repo = tmp_path / "checkout"
    source = tmp_path / "package-source"
    nested_build = repo / "tmp" / "build"
    external_build = tmp_path / "scratch" / "build"
    roles = (".molt/objects", ".molt/build", ".molt/source", ".molt/repo")

    def arguments(build: Path) -> list[str]:
        return _flag_replacements(
            _source_extension_deterministic_path_args(
                compiler_command=("clang",),
                roots=(
                    (build / "objects", ".molt/objects"),
                    (build, ".molt/build"),
                    (source, ".molt/source"),
                    (repo, ".molt/repo"),
                ),
            )
        )

    assert arguments(nested_build) == list(roles)
    assert arguments(external_build) == list(roles)


def test_equal_root_alias_uses_first_declared_semantic_role(tmp_path: Path) -> None:
    shared = tmp_path / "repo-and-source"
    arguments = _source_extension_deterministic_path_args(
        compiler_command=("clang",),
        roots=((shared, ".molt/source"), (shared, ".molt/repo")),
    )

    assert _flag_replacements(arguments) == [".molt/source"]


def test_location_canonicalization_covers_mapping_keys_and_values(
    tmp_path: Path,
) -> None:
    build = tmp_path / "build"
    payload = {str(build / "module.py"): {"path": str(build / "module.py")}}

    canonical = _canonicalize_locations(payload, ((build, "@build"),))

    assert canonical == {"@build/module.py": {"path": "@build/module.py"}}


def test_location_canonicalization_is_path_boundary_aware(tmp_path: Path) -> None:
    build = tmp_path / "build"
    sibling = tmp_path / "build-other"

    canonical = _canonicalize_locations(
        {
            "selected": str(build / "module.py"),
            "sibling": str(sibling / "module.py"),
        },
        ((build, "@build"),),
    )

    assert canonical["selected"] == "@build/module.py"
    # A path outside every root keeps its own spelling: only rooted spans move.
    assert canonical["sibling"] == str(sibling / "module.py")


def test_location_canonicalization_rewrites_quoted_generated_config_path(
    tmp_path: Path,
) -> None:
    interpreter = tmp_path / "ephemeral-env"
    config = f'"path": r"{interpreter}"'

    canonical = _canonicalize_locations(config, ((interpreter, "@python"),))

    assert canonical == '"path": r"@python"'


def test_location_canonicalization_rejects_collapsed_keys(tmp_path: Path) -> None:
    build = tmp_path / "build"
    with pytest.raises(ValueError, match="collapses distinct metadata keys"):
        _canonicalize_locations(
            {
                str(build / "module.py"): 1,
                (build / "module.py").as_posix(): 2,
            },
            ((build, "@build"),),
        )


def test_meson_metadata_identity_ignores_roots_and_transient_dependency_ids(
    tmp_path: Path,
) -> None:
    first_root = tmp_path / "v4"
    second_root = tmp_path / "v5-repro-with-a-different-length"
    first = [
        {
            "filename": str(first_root / "module.c"),
            "name": "dep123",
            "dependencies": ["none", "dep149274466672618721776634620382072147803"],
        },
        {"dependencies": ["dep213365065413222590675692399346628757385"]},
    ]
    second = [
        {
            "filename": str(second_root / "module.c"),
            "name": "dep123",
            "dependencies": ["none", "dep275308643866700071502226362491441940796"],
        },
        {"dependencies": ["dep201663976336061095125455158485315758473"]},
    ]

    first_canonical = _canonicalize_meson_metadata(first, ((first_root, "@build"),))
    second_canonical = _canonicalize_meson_metadata(second, ((second_root, "@build"),))
    assert first_canonical == second_canonical
    assert first_canonical[0]["name"] == "dep123"

    changed_equivalence = [
        {
            "filename": str(second_root / "module.c"),
            "name": "dep123",
            "dependencies": ["none", "dep1"],
        },
        {"dependencies": ["dep1"]},
    ]
    assert first_canonical != _canonicalize_meson_metadata(
        changed_equivalence,
        ((second_root, "@build"),),
    )


def test_wheel_manifest_core_is_invariant_to_all_operational_roots(
    tmp_path: Path,
) -> None:
    def materialize(label: str, dependency_id: str) -> dict[str, object]:
        root = tmp_path / label
        source = root / "source"
        build = root / "a-build-root-with-variable-spelling"
        output = root / "transaction" / "output"
        plan = build / "meson-info/intro-targets.json"
        commands = build / "compile_commands.json"
        plan.parent.mkdir(parents=True)
        plan.write_text(
            json.dumps(
                [
                    {
                        "filename": str(build / "module.wasm"),
                        "dependencies": [dependency_id],
                    }
                ]
            ),
            encoding="utf-8",
        )
        commands.write_text(
            json.dumps(
                [
                    {
                        "directory": str(build),
                        "file": str(source / "module.c"),
                    }
                ]
            ),
            encoding="utf-8",
        )
        manifest = {
            "module": "pkg.native",
            "init_symbol": "PyInit_native",
            "target_triple": "wasm32-wasip1",
            "artifact_kind": "wasm_relocatable_object",
            "extension": "pkg/native.molt.wasm",
            "extension_sha256": "a" * 64,
            "wheel": "pkg-1.0-py3-molt_abi1-wasm32_wasip1.whl",
            "source_plan": {
                "kind": "meson-intro-targets",
                "plan": str(plan),
                "plan_sha256": "stale",
                "compile_commands": str(commands),
                "compile_commands_sha256": "stale",
                "source_root": str(source),
                "build_root": str(build),
                "digest": "stale",
            },
            "object_closure": {
                "schema_version": SOURCE_EXTENSION_OBJECT_CLOSURE_SCHEMA_VERSION,
                "root_symbol": "PyInit_native",
                "init_symbol_owner": "module.o",
                "defined_symbols": ["PyInit_native"],
                "undefined_symbols": [],
                "runtime_symbols": [],
                "required_c_api_symbols": [],
                "required_capsules": [],
                "project_generated_c_api_symbols": [],
                "wasm_imports": [],
                "objects": [
                    {
                        "source": str(source / "module.c"),
                        "object": "module.o",
                        "language": "c",
                        "source_sha256": "b" * 64,
                        "object_sha256": "c" * 64,
                        "defined_symbols": ["PyInit_native"],
                        "undefined_symbols": [],
                        "compile_command": [
                            "clang",
                            "-x",
                            "c",
                            "-c",
                            str(source / "module.c"),
                            "-o",
                            str(output / "module.o"),
                        ],
                        "symbol_authority": SOURCE_EXTENSION_WASM_SYMBOL_AUTHORITY,
                        "dependencies": [],
                        "required_c_api_symbols": [],
                        "required_capsules": [],
                        "project_generated_c_api_symbols": [],
                    }
                ],
            },
            "build": {
                "source_plan_digest": "stale",
            },
        }
        finalize_source_extension_object_closure(manifest)
        return _canonical_extension_manifest_for_wheel(
            manifest,
            location_roots=(
                (source, "@source"),
                (build, "@build"),
                (output, "@output"),
                (root / "transaction", "@transaction"),
            ),
            meson_plan_path=plan,
            compile_commands_path=commands,
        )

    first = materialize("v4", "dep149274466672618721776634620382072147803")
    second = materialize(
        "v5-repro-with-a-different-length",
        "dep275308643866700071502226362491441940796",
    )

    assert first == second
    assert str(tmp_path) not in json.dumps(first)


def test_location_roots_canonicalize_every_spelling_of_the_same_directory(
    tmp_path: Path,
) -> None:
    """A version-alias junction and its real directory are one producer root."""
    import os
    import sys

    from molt.cli.source_extension_reproducibility import (
        _canonicalize_location_string,
    )

    real = tmp_path / "cpython-3.12.13"
    real.mkdir()
    alias = tmp_path / "cpython-3.12"
    if sys.platform == "win32":
        from tests.process_guard_common import run_guarded_test_process

        created = run_guarded_test_process(
            ["cmd.exe", "/d", "/c", "mklink", "/J", str(alias), str(real)],
            capture_output=True,
            text=True,
            check=False,
        )
        if created.returncode != 0:
            pytest.skip(f"junction creation is unavailable: {created.stderr}")
    else:
        os.symlink(real, alias, target_is_directory=True)
    assert alias.resolve() == real.resolve()
    roots = [(alias, "@python-base")]

    assert (
        _canonicalize_location_string(f"-I{real.as_posix()}/Include", roots)
        == "-I@python-base/Include"
    )
    assert (
        _canonicalize_location_string(f"-I{alias.as_posix()}/Include", roots)
        == "-I@python-base/Include"
    )


def test_virtual_posix_root_canonicalizes_the_install_prefix() -> None:
    from pathlib import PurePosixPath

    from molt.cli.source_extension_reproducibility import (
        _canonicalize_location_string,
        _residual_producer_paths,
    )

    roots = [(PurePosixPath("/molt-install-prefix"), "@install-prefix")]
    canonical = _canonicalize_location_string(
        "/molt-install-prefix/Lib/site-packages/numpy/version.py", roots
    )
    assert canonical == "@install-prefix/Lib/site-packages/numpy/version.py"
    assert _residual_producer_paths([canonical]) == []


def test_location_canonicalization_rewrites_only_path_spans(tmp_path: Path) -> None:
    # Installed Python sources travel through location canonicalization: an
    # escape sequence is not a path separator, and a path literal keeps the
    # escapes that follow it (the old whole-text replacement turned every
    # sealed "\\n" into "/n").
    backslash = chr(92)
    build = tmp_path / "build"
    raw = str(build)
    escaped = raw.replace(backslash, backslash * 2)
    source = (
        "greeting = 'hello" + backslash + "n'\n"
        "raw_path = r'" + raw + backslash + "sub" + backslash + "f.c'\n"
        "literal = '" + escaped + backslash * 2 + "sub" + backslash + "n'\n"
        "json = '\"" + escaped + backslash * 2 + "sub" + backslash * 2 + "\"'\n"
    )

    canonical = _canonicalize_locations(source, ((build, "@build"),))

    assert canonical == (
        "greeting = 'hello" + backslash + "n'\n"
        "raw_path = r'@build/sub/f.c'\n"
        "literal = '@build/sub" + backslash + "n'\n"
        "json = '\"@build/sub/\"'\n"
    )
