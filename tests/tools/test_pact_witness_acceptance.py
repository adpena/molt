from __future__ import annotations

import json
from pathlib import Path

import tools.pact_witness_acceptance as acceptance


def test_pact_witness_acceptance_uses_run_scoped_attempt_dirs(
    tmp_path: Path,
    monkeypatch,
) -> None:
    monkeypatch.setattr(acceptance, "ROOT", tmp_path)
    monkeypatch.setenv("MOLT_PROOF_QUEUE_RUN_ID", "run:id/with spaces")
    out_dir = tmp_path / "tmp" / "pact_witness_acceptance_queue"
    stale_build = out_dir / "build"
    stale_build.mkdir(parents=True)
    stale_file = stale_build / "output_linked.wat"
    stale_file.write_text(
        "still held by a previous Windows process\n", encoding="utf-8"
    )

    build_dir, run_dir = acceptance._prepare_attempt_dirs(out_dir)
    second_build_dir, second_run_dir = acceptance._prepare_attempt_dirs(out_dir)

    assert build_dir == out_dir / "runs" / "run_id_with_spaces" / "build"
    assert run_dir == out_dir / "runs" / "run_id_with_spaces" / "run"
    assert second_build_dir == out_dir / "runs" / "run_id_with_spaces-2" / "build"
    assert second_run_dir == out_dir / "runs" / "run_id_with_spaces-2" / "run"
    assert stale_file.read_text(encoding="utf-8").startswith("still held")
    assert (out_dir / "latest_attempt.txt").read_text(encoding="utf-8").strip() == str(
        second_build_dir.parent
    )


def test_pact_witness_acceptance_prefers_split_runtime_app_entry(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    app_wasm = build_dir / "app.wasm"
    runtime_wasm = build_dir / "molt_runtime.wasm"
    output_wasm.write_bytes(b"monolithic-prelink")
    app_wasm.write_bytes(b"split-app")
    runtime_wasm.write_bytes(b"split-runtime")

    selected = acceptance._select_wasm_entry(build_dir)
    env = acceptance._wasm_run_env(selected)

    assert selected == app_wasm
    assert env["MOLT_WASM_DIRECT_LINK"] == "1"
    assert env["MOLT_WASM_PREFER_LINKED"] == "0"
    assert env["MOLT_RUNTIME_WASM"] == str(runtime_wasm)


def test_pact_witness_acceptance_uses_output_wasm_without_split_runtime(
    tmp_path: Path,
) -> None:
    build_dir = tmp_path / "build"
    build_dir.mkdir()
    output_wasm = build_dir / "output.wasm"
    output_wasm.write_bytes(b"monolithic")

    selected = acceptance._select_wasm_entry(build_dir)
    env = acceptance._wasm_run_env(selected)

    assert selected == output_wasm
    assert "MOLT_WASM_DIRECT_LINK" not in env
    assert "MOLT_RUNTIME_WASM" not in env


def test_pact_witness_acceptance_reports_static_extension_capsule_drift(
    tmp_path: Path,
) -> None:
    module_root = tmp_path / "site"
    manifest_path = (
        module_root
        / "scipy"
        / "ndimage"
        / "_nd_image.molt.wasm.extension_manifest.json"
    )
    manifest_path.parent.mkdir(parents=True)
    source_path = tmp_path / "scipy" / "ndimage" / "src" / "nd_image.c"
    source_path.parent.mkdir(parents=True)
    source_path.write_text(
        "static int _nd_image_module_exec(PyObject *module) {\n"
        "    if (_import_array() < 0) { return -1; }\n"
        "    return 0;\n"
        "}\n",
        encoding="utf-8",
    )
    manifest_path.write_text(
        json.dumps(
            {
                "module": "scipy.ndimage._nd_image",
                "extension": "_nd_image.molt.wasm",
                "init_symbol": "PyInit__nd_image",
                "runtime_linkage": "static_link",
                "artifact_kind": "wasm_relocatable_object",
                "sources": [str(source_path)],
                "object_closure": {
                    "defined_symbols": [],
                    "runtime_symbols": [],
                    "undefined_symbols": [],
                },
            }
        ),
        encoding="utf-8",
    )
    output_text = (
        "Error: Unhandled Molt exception: ImportError: _nd_image: "
        "static-link PyModuleDef Py_mod_exec slot returned non-zero\n"
    )

    report = acceptance._static_extension_init_failure_report(
        output_text=output_text,
        env={"MOLT_MODULE_ROOTS": str(module_root)},
    )

    assert report is not None
    assert report["failure"]["module"] == "_nd_image"
    match = report["manifest_matches"][0]
    assert match["manifest_path"] == str(manifest_path.resolve())
    assert match["manifest_module"] == "scipy.ndimage._nd_image"
    assert match["missing_manifest_required_capsules"] == [
        "numpy.core._multiarray_umath._ARRAY_API"
    ]
    assert match["source_required_capsules"] == [
        "numpy.core._multiarray_umath._ARRAY_API"
    ]
    assert match["sources"][0]["line_hits"][0]["hits"][0]["line"] == 2
    assert match["sources"][0]["line_hits"][0]["hits"][0]["token"] == "_import_array"


def test_pact_witness_acceptance_writes_static_extension_diagnostic(
    tmp_path: Path,
) -> None:
    module_root = tmp_path / "site"
    manifest_path = module_root / "_native.molt.wasm.extension_manifest.json"
    manifest_path.parent.mkdir(parents=True)
    source_path = tmp_path / "native.c"
    source_path.write_text("int ready(void) { return import_array1(-1); }\n")
    manifest_path.write_text(
        json.dumps(
            {
                "module": "_native",
                "init_symbol": "PyInit__native",
                "sources": [str(source_path)],
                "object_closure": {"required_capsules": []},
            }
        ),
        encoding="utf-8",
    )
    run_dir = tmp_path / "run"
    run_dir.mkdir()

    report_path = acceptance._write_static_extension_init_failure_diagnostic(
        output_text=(
            "ImportError: _native: static-link PyModuleDef "
            "Py_mod_exec slot returned non-zero\n"
        ),
        run_dir=run_dir,
        env={"MOLT_MODULE_ROOTS": str(module_root)},
    )

    assert report_path == run_dir / "static_extension_init_failure.json"
    report = json.loads(report_path.read_text(encoding="utf-8"))
    assert report["manifest_matches"][0]["missing_manifest_required_capsules"] == [
        "numpy.core._multiarray_umath._ARRAY_API"
    ]
