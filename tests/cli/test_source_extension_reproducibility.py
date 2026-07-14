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


def test_path_map_order_places_descendant_before_declared_ancestor(
    tmp_path: Path,
) -> None:
    output = tmp_path / "output"
    objects = output / "objects"

    arguments = _source_extension_deterministic_path_args(
        compiler_command=("clang",),
        roots=((output, ".molt/output"), (objects, ".molt/objects")),
    )

    assert _flag_replacements(arguments) == [".molt/objects", ".molt/output"]


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
    assert canonical["sibling"] == (sibling / "module.py").as_posix()


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
    second_canonical = _canonicalize_meson_metadata(
        second, ((second_root, "@build"),)
    )
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
                "closure_sha256": "stale",
                "objects": [
                    {
                        "source": str(source / "module.c"),
                        "object": str(output / "module.o"),
                        "compile_command": [
                            "clang",
                            "-c",
                            str(source / "module.c"),
                            "-o",
                            str(output / "module.o"),
                        ],
                    }
                ],
            },
            "build": {
                "source_plan_digest": "stale",
                "object_closure_sha256": "stale",
            },
        }
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
