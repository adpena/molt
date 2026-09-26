from __future__ import annotations

import json
import subprocess
from contextlib import nullcontext
from dataclasses import replace
from pathlib import Path

import pytest

from molt.cli import source_extensions, source_extension_cython


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


def test_cython_generation_shares_only_equivalent_unit_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source_root, build_root, plan_path, generated_c = _write_missing_generated_plan(
        tmp_path, pyx_relpaths=("pkg/probe.pyx",)
    )
    plan, errors = _load_plan(
        source_root=source_root, build_root=build_root, plan_path=plan_path
    )
    assert not errors and plan is not None
    base = plan.compile_units[0]
    # Same C input can be compiled with different SIMD flags without changing
    # generation. Language, ordered include inputs, and producer output cannot.
    include_a, include_b = source_root / "a", source_root / "b"
    units = (
        base,
        replace(base, compile_args=("-DVARIANT=1",)),
        replace(base, include_dirs=(include_a, include_b)),
        replace(base, include_dirs=(include_b, include_a)),
        replace(base, language=source_extension_cython.SourceExtensionLanguage.CPP),
        replace(base, source_path=build_root / "other" / generated_c.name),
    )
    calls = []

    def generate(**kwargs):
        calls.append(kwargs)
        output = kwargs["out_dir"] / kwargs["original_c"].name
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(str(len(calls)), encoding="utf-8")
        return source_extension_cython.CythonRegeneration(
            pyx_path=kwargs["pyx_path"],
            original_c=kwargs["original_c"],
            regenerated_c=output,
            cython_version="test",
            cython_argv=(),
        ), None

    monkeypatch.setattr(
        source_extension_cython, "cython_execution", lambda **kw: nullcontext(None)
    )
    monkeypatch.setattr(
        source_extension_cython, "regenerate_cython_c_standalone", generate
    )
    results, error = source_extension_cython.source_plan_cython_regenerations(
        plan=replace(plan, compile_units=units),
        pyproject={},
        ninja_command=("owned-ninja",),
        abi_tier="cpython-abi",
    )
    assert error is None and results is not None
    assert results[0] is results[1]
    assert len(calls) == 5
    assert [call["include_dirs"] for call in calls[1:3]] == [
        (include_a, include_b),
        (include_b, include_a),
    ]
    distinct = (results[0], *results[2:])
    assert len({result.regenerated_c for result in distinct}) == 5
    assert [
        result.regenerated_c.read_text(encoding="utf-8") for result in distinct
    ] == ["1", "2", "3", "4", "5"]
    sibling, error = source_extension_cython.source_plan_cython_regenerations(
        plan=replace(plan, target_id="sibling-target"),
        pyproject={},
        ninja_command=("owned-ninja",),
        abi_tier="cpython-abi",
    )
    assert error is None and sibling is not None and sibling[0] is not None
    assert sibling[0].regenerated_c != results[0].regenerated_c
    assert results[0].regenerated_c.read_text(encoding="utf-8") == "1"
    other_pyx = source_root / "other/probe.pyx"
    other_pyx.parent.mkdir()
    other_pyx.write_text("def different(): return 2\n", encoding="utf-8")
    ambiguous, error = source_extension_cython.source_plan_cython_regenerations(
        plan=replace(plan, non_compiled_inputs=(*plan.non_compiled_inputs, other_pyx)),
        pyproject={},
        ninja_command=("owned-ninja",),
        abi_tier="cpython-abi",
    )
    assert ambiguous is None and error is not None and "Ambiguous Cython input" in error
    assert len(calls) == 6


@pytest.mark.parametrize("output_exists", [False, True])
@pytest.mark.parametrize("generator", ["cython", "other", "missing-input", "no-owner"])
def test_generated_unit_uses_own_ninja_command_not_file_presence_or_stem(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    output_exists: bool,
    generator: str,
) -> None:
    source_root, build_root, plan_path, generated_c = _write_missing_generated_plan(
        tmp_path, pyx_relpaths=("pkg/probe.pyx",)
    )
    plan, errors = _load_plan(
        source_root=source_root, build_root=build_root, plan_path=plan_path
    )
    assert not errors and plan is not None
    if output_exists:
        generated_c.parent.mkdir(parents=True, exist_ok=True)
        generated_c.write_text("/* upstream shared utility output */", encoding="utf-8")
    (build_root / "build.ninja").write_text("# expanded by the owned Ninja\n")
    pyx = source_root / "pkg/probe.pyx"
    command_input = pyx if generator != "missing-input" else source_root / "missing.pyx"
    command = subprocess.list2cmdline(
        [
            "cython" if generator != "other" else "tempita",
            str(command_input),
            "-o",
            str(generated_c),
        ]
    )
    queries = []
    generated = []

    def query(argv, **kwargs):
        queries.append(argv)
        assert argv[:1] == ["owned-ninja"]
        assert argv[-4:] == [
            "-t",
            "commands",
            "-s",
            generated_c.relative_to(build_root).as_posix(),
        ]
        return subprocess.CompletedProcess(
            argv, 0, "" if generator == "no-owner" else command + "\n", ""
        )

    def generate(**kwargs):
        generated.append(kwargs)
        return source_extension_cython.CythonRegeneration(
            pyx_path=kwargs["pyx_path"],
            original_c=kwargs["original_c"],
            regenerated_c=kwargs["out_dir"] / "probe.c",
            cython_version="test",
            cython_argv=(),
        ), None

    monkeypatch.setattr(
        source_extension_cython.process_guard, "run_completed_command", query
    )
    monkeypatch.setattr(
        source_extension_cython, "cython_execution", lambda **kw: nullcontext(None)
    )
    monkeypatch.setattr(
        source_extension_cython, "regenerate_cython_c_standalone", generate
    )
    # The .pyx is absent from selected target introspection and visible only in
    # the output's own Ninja command. Repeated compiled variants query it once.
    result, error = source_extension_cython.source_plan_cython_regenerations(
        plan=replace(
            plan, non_compiled_inputs=(), compile_units=plan.compile_units * 2
        ),
        pyproject={},
        ninja_command=("owned-ninja",),
        abi_tier="cpython-abi",
    )
    assert len(queries) == 1
    if generator in {"missing-input", "no-owner"}:
        expected = (
            "0 existing .pyx inputs"
            if generator == "missing-input"
            else "no owning generator command"
        )
        assert result is None and error is not None and expected in error
        assert not generated
    elif generator == "other":
        assert error is None and result == (None, None)
        assert not generated
    else:
        assert error is None and result is not None and result[0] is result[1]
        assert len(generated) == 1 and generated[0]["pyx_path"] == pyx


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
