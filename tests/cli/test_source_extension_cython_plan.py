from __future__ import annotations

import json
from pathlib import Path

from molt.cli import source_extensions


def _write_missing_generated_plan(
    tmp_path: Path,
    *,
    pyx_relpaths: tuple[str, ...],
) -> tuple[Path, Path, Path, Path]:
    source_root = tmp_path / "source"
    build_root = tmp_path / "build"
    plan_path = build_root / "meson-info" / "intro-targets.json"
    compile_commands_path = build_root / "compile_commands.json"
    generated_c = build_root / "generated" / "probe.c"
    plan_path.parent.mkdir(parents=True)
    for relpath in pyx_relpaths:
        pyx = source_root / relpath
        pyx.parent.mkdir(parents=True, exist_ok=True)
        pyx.write_text("def probe():\n    return 1\n", encoding="utf-8")
    plan_path.write_text(
        json.dumps(
            [
                {
                    "id": "pkg.probe",
                    "name": "probe",
                    "type": "shared module",
                    "filename": str(build_root / "pkg" / "probe.so"),
                    "target_sources": [
                        {
                            "language": "c",
                            "sources": list(pyx_relpaths),
                            "generated_sources": [str(generated_c)],
                        }
                    ],
                    "linker_parameters": [],
                }
            ]
        ),
        encoding="utf-8",
    )
    compile_commands_path.write_text(
        json.dumps(
            [
                {
                    "directory": str(build_root),
                    "file": str(generated_c),
                    "arguments": [
                        "clang",
                        "-DKEEP_GENERATED_UNIT=1",
                        "-c",
                        str(generated_c),
                        "-o",
                        str(build_root / "pkg" / "probe.so.p" / "probe.o"),
                    ],
                }
            ]
        ),
        encoding="utf-8",
    )
    return source_root, build_root, plan_path, generated_c


def _load_plan(
    *,
    source_root: Path,
    build_root: Path,
    plan_path: Path,
) -> tuple[source_extensions._SourceExtensionBuildPlan | None, list[str]]:
    return source_extensions._load_meson_intro_targets_source_extension_plan(
        plan_path=plan_path,
        project_root=source_root,
        module_name="pkg.probe",
        selector="probe",
        source_root=source_root,
        build_root=build_root,
        compile_commands=build_root / "compile_commands.json",
    )


def test_missing_generated_c_keeps_real_compile_unit_for_unique_pyx(
    tmp_path: Path,
) -> None:
    source_root, build_root, plan_path, generated_c = _write_missing_generated_plan(
        tmp_path, pyx_relpaths=("pkg/probe.pyx",)
    )

    plan, errors = _load_plan(
        source_root=source_root,
        build_root=build_root,
        plan_path=plan_path,
    )

    assert errors == []
    assert plan is not None
    assert not generated_c.exists(), "plan loading must not create placeholder C"
    assert plan.generated_sources == (generated_c.resolve(),)
    assert len(plan.compile_units) == 1
    unit = plan.compile_units[0]
    assert unit.source_path == generated_c.resolve()
    assert unit.generated is True
    assert unit.compiler == ("clang",)
    assert unit.compile_args == ("-DKEEP_GENERATED_UNIT=1",)


def test_missing_generated_c_without_same_stem_pyx_fails_closed(
    tmp_path: Path,
) -> None:
    source_root, build_root, plan_path, generated_c = _write_missing_generated_plan(
        tmp_path, pyx_relpaths=("pkg/other.pyx",)
    )

    plan, errors = _load_plan(
        source_root=source_root,
        build_root=build_root,
        plan_path=plan_path,
    )

    assert plan is None
    assert any(
        "has no unique target-local or Ninja-proven .pyx input" in error
        and str(generated_c.resolve()) in error
        for error in errors
    )


def test_missing_generated_c_with_duplicate_same_stem_pyx_fails_closed(
    tmp_path: Path,
) -> None:
    source_root, build_root, plan_path, generated_c = _write_missing_generated_plan(
        tmp_path,
        pyx_relpaths=("pkg/left/probe.pyx", "pkg/right/probe.pyx"),
    )

    plan, errors = _load_plan(
        source_root=source_root,
        build_root=build_root,
        plan_path=plan_path,
    )

    assert plan is None
    assert any(
        "ambiguous same-stem Cython inputs" in error
        and str(generated_c.resolve()) in error
        for error in errors
    )
