from __future__ import annotations

import json
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import cargo_build_reads as reads

ROOT = Path(__file__).resolve().parents[2]


def _write(path: Path, text: str = "x\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _escape(entry: str) -> str:
    return entry.replace(" ", "\\ ")


def _unit(
    deps: Path,
    package: Path | None,
    crate: str,
    entries: list[str],
    *,
    kind: str = "lib",
    environment: dict[str, str] | None = None,
    registry: str | None = None,
) -> str:
    """A compiler-artifact message and the rustc dep-info beside its outputs."""
    stem = f"{crate}-0123456789abcdef"
    output = deps / (f"lib{stem}.rlib" if kind == "lib" else stem)
    dep_info = deps / f"{stem}.d"
    lines = [f"{_escape(os.fspath(output))}: {' '.join(map(_escape, entries))}", ""]
    lines += [f"{_escape(entry)}:" for entry in entries]
    lines += [""] + [f"# env-dep:{k}={v}" for k, v in (environment or {}).items()]
    _write(dep_info, "\n".join(lines) + "\n")
    _write(output)
    package_id = (
        f"registry+https://github.com/rust-lang/crates.io-index#{registry}"
        if package is None
        else f"path+{package.as_uri()}#{crate}@0.1.0"
    )
    return json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": package_id,
            "target": {"kind": [kind], "name": crate},
            "filenames": [os.fspath(output)],
            "fresh": True,
        }
    )


def _script(build: Path, package: Path, name: str, lines: list[str]) -> str:
    out_dir = build / f"{name}-1" / "out"
    _write(out_dir.parent / "output", "".join(f"{line}\n" for line in lines))
    return json.dumps(
        {
            "reason": "build-script-executed",
            "package_id": f"path+{package.as_uri()}#{name}@0.1.0",
            "out_dir": os.fspath(out_dir),
        }
    )


@pytest.fixture
def workspace(tmp_path: Path):
    root = tmp_path / "work space"
    crate = root / "runtime" / "crate"
    for path in (
        crate / "src" / "lib.rs",
        crate / "src" / "data file.json",
        crate / "build.rs",
        root / "include" / "api.h",
        root / "include" / "shared" / "layout.h",
        root / "vendor" / "zlib" / "src" / "lib.rs",
    ):
        _write(path)
    target = tmp_path / "target"
    return root, crate, target


def _read(workspace, *messages: str, toolchain_roots=()):
    root, _crate, target = workspace
    return reads.cargo_build_reads(
        "\n".join(messages),
        workspace_root=root,
        product_roots=(target,),
        toolchain_roots=toolchain_roots,
    )


def _relative(root: Path, paths) -> list[str]:
    return [path.relative_to(os.path.realpath(root)).as_posix() for path in paths]


def test_each_local_unit_contributes_what_rustc_read(workspace) -> None:
    root, crate, target = workspace
    deps = target / "debug" / "deps"
    generated = _write(target / "debug" / "build" / "crate-1" / "out" / "tables.rs")
    messages = [
        _unit(
            deps,
            crate,
            "crate",
            [
                "runtime/crate/src/lib.rs",
                "runtime/crate/src/data file.json",
                os.fspath(generated),
            ],
            environment={
                "CARGO_PKG_NAME": "crate",
                "OUT_DIR": os.fspath(generated.parent),
            },
        ),
        # A local package that only registry crates reach, as zlib-rs is.
        _unit(deps, root / "vendor" / "zlib", "zlib", ["vendor/zlib/src/lib.rs"]),
        _unit(
            deps,
            None,
            "objc2",
            ["/registry/objc2/src/lib.rs"],
            environment={"UNSAFE_OBJC2_ALLOW_CLASS_OVERRIDE": "1"},
            registry="objc2@0.6.5",
        ),
    ]

    result = _read(workspace, *messages)

    assert _relative(root, result.files) == [
        "runtime/crate/src/data file.json",
        "runtime/crate/src/lib.rs",
        "vendor/zlib/src/lib.rs",
    ]
    # Registry sources are Cargo.lock's; the variables they read are not.
    assert ("UNSAFE_OBJC2_ALLOW_CLASS_OVERRIDE", "1") in result.environment
    assert ("CARGO_PKG_NAME", "crate") in result.environment


def test_build_scripts_contribute_their_declared_inputs(workspace) -> None:
    root, crate, target = workspace
    build = target / "debug" / "build"
    deps = target / "debug" / "deps"
    vendored = root / "vendor" / "object"
    for path in (
        vendored / "Cargo.toml",
        vendored / "README.md",
        vendored / "src" / "lib.rs",
    ):
        _write(path)
    _write(vendored / "fuzz" / "Cargo.toml")
    _write(vendored / "fuzz" / "seed.rs")
    _write(vendored / "target" / "debug" / "out.o")

    result = _read(
        workspace,
        _unit(deps, crate, "crate", ["runtime/crate/src/lib.rs"]),
        _script(
            build,
            crate,
            "crate",
            [
                "cargo:rerun-if-changed=build.rs",
                "cargo:rerun-if-changed=../../include/api.h",
                "cargo::rerun-if-changed=../../include/shared",
                "cargo:rerun-if-env-changed=LIBSQLITE3_FLAGS",
            ],
        ),
        # A script that declares nothing reruns on any change to its package.
        _script(build, vendored, "object", ["cargo:rustc-cfg=feature_x"]),
    )

    assert _relative(root, result.files) == [
        "include/api.h",
        "include/shared/layout.h",
        "runtime/crate/build.rs",
        "runtime/crate/src/lib.rs",
        "vendor/object/Cargo.toml",
        "vendor/object/README.md",
        "vendor/object/src/lib.rs",
    ]
    assert _relative(root, result.directories) == ["include/shared"]
    assert ("LIBSQLITE3_FLAGS", None) in result.environment


def test_a_read_of_another_workspace_is_refused(workspace, tmp_path) -> None:
    _root, crate, target = workspace
    foreign = _write(tmp_path / "other checkout" / "lib.rs")
    unit = _unit(target / "deps", crate, "crate", [os.fspath(foreign)])
    with pytest.raises(reads.CargoBuildReadsError, match="outside"):
        _read(workspace, unit)


def test_a_toolchain_root_is_left_to_the_plan_identity(workspace, tmp_path) -> None:
    root, crate, target = workspace
    header = _write(tmp_path / "wasi-sdk" / "include" / "stdio.h")
    unit = _unit(
        target / "deps", crate, "crate", ["runtime/crate/src/lib.rs", os.fspath(header)]
    )
    result = _read(workspace, unit, toolchain_roots=(tmp_path / "wasi-sdk",))
    assert _relative(root, result.files) == ["runtime/crate/src/lib.rs"]


def test_a_build_without_local_units_is_refused(workspace) -> None:
    with pytest.raises(reads.CargoBuildReadsError, match="no local unit"):
        _read(workspace, json.dumps({"reason": "build-finished", "success": True}))


def test_a_local_unit_without_dep_info_is_refused(workspace) -> None:
    _root, crate, _target = workspace
    message = json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
            "target": {"kind": ["lib"], "name": "crate"},
            "filenames": [os.fspath(crate / "missing" / "libcrate-1.rlib")],
        }
    )
    with pytest.raises(reads.CargoBuildReadsError, match="no dep-info"):
        _read(workspace, message)


def _native_plan(tmp_path: Path, entries: list[str]):
    target = tmp_path / "target"
    unit = _unit(
        target / "dev-fast" / "deps",
        ROOT / "runtime" / "molt-runtime",
        "molt_runtime",
        entries,
    )
    return (
        SimpleNamespace(
            cargo_plan=SimpleNamespace(
                environment={"CARGO_TARGET_DIR": os.fspath(target)}
            ),
            runtime_lib=target / "dev-fast" / "libmolt_runtime.stdlib_edge.a",
            target_triple=None,
            project_root=ROOT,
            fingerprint_features=(),
            runtime_state=None,
            cmd=["cargo", "rustc"],
            json_output=True,
            stage_timings_ms=None,
        ),
        SimpleNamespace(stdout=unit, stderr="", returncode=0),
    )


def test_the_runtime_identity_covers_what_its_build_reads(tmp_path) -> None:
    """The 29 inputs the runtime identity once missed (HF-300) are covered:
    headers and a table that build scripts read outside every crate, and
    sibling-crate files molt-runtime compiles through ``#[path]``."""
    from molt.cli import runtime_native_build

    plan, build = _native_plan(
        tmp_path,
        [
            "runtime/molt-runtime/src/lib.rs",
            "runtime/molt-runtime/../../include/molt/molt.h",
            "include/molt/shared/_buffer_abi.h",
            "runtime/molt-runtime-constants/../wasm_table_layout.inc",
            "runtime/molt-runtime/src/builtins/../../../molt-runtime-path/src/pathlib.rs",
        ],
    )
    assert (
        runtime_native_build._runtime_build_inputs_outside_identity(plan, build) == ()
    )


def test_publication_refuses_a_build_input_the_identity_misses(
    tmp_path, monkeypatch
) -> None:
    from molt.cli import runtime_native_build

    plan, build = _native_plan(
        tmp_path, ["runtime/molt-runtime/src/lib.rs", "README.md"]
    )
    failures: list[dict[str, object]] = []

    def record_failure(_state, **fields) -> bool:
        failures.append(fields)
        return False

    def publish(*_args, **_kwargs):
        raise AssertionError("an uncovered input must stop publication")

    monkeypatch.setattr(
        runtime_native_build, "_record_native_runtime_failure", record_failure
    )
    monkeypatch.setattr(
        runtime_native_build, "publish_native_runtime_generation", publish
    )

    assert runtime_native_build._publish_native_runtime_build(plan, build) is False
    assert [failure["stage"] for failure in failures] == ["build-input-coverage"]
    assert "README.md" in str(failures[0]["summary"])


def test_uplifted_units_resolve_to_the_dep_info_this_build_used(workspace) -> None:
    """Cargo's real layouts: a build script reported as its uplifted
    ``build-script-build``, and a root staticlib reported only in the profile
    directory, with one dep-info candidate per feature lane."""
    root, crate, target = workspace
    build_dir = target / ".build" / "dev-fast"
    script_dir = build_dir / "build" / "crate-988acd9c2b90addb"
    _write(script_dir / "build-script-build")
    _write(script_dir / "build_script_build-988acd9c2b90addb")
    _write(
        script_dir / "build_script_build-988acd9c2b90addb.d",
        f"{_escape(os.fspath(script_dir / 'build_script_build-988acd9c2b90addb'))}:"
        " runtime/crate/build.rs\n",
    )
    script = json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
            "target": {"kind": ["custom-build"], "name": "build-script-build"},
            "filenames": [os.fspath(script_dir / "build-script-build")],
        }
    )
    deps = build_dir / "deps"
    lane_out = os.fspath(build_dir / "build" / "crate-3376a64776671bc0" / "out")
    other_out = os.fspath(build_dir / "build" / "crate-60afce28417c3601" / "out")
    for metadata, out_dir, source in (
        ("f092e115d9c64469", lane_out, "runtime/crate/src/lib.rs"),
        ("0987fd8fa999fa1f", other_out, "runtime/crate/src/data file.json"),
    ):
        _write(
            deps / f"crate-{metadata}.d",
            f"{_escape(os.fspath(deps / f'libcrate-{metadata}.a'))}: {_escape(source)}\n"
            f"\n# env-dep:OUT_DIR={out_dir}\n",
        )
    # Another local unit in deps tells the reader where this build's units live.
    helper = _unit(deps, root / "vendor" / "zlib", "zlib", ["vendor/zlib/src/lib.rs"])
    uplifted = _write(target / "dev-fast" / "libcrate.a")
    root_unit = json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
            "target": {"kind": ["staticlib"], "name": "crate"},
            "filenames": [os.fspath(uplifted)],
        }
    )
    executed = _script(
        build_dir / "build", crate, "crate", ["cargo:rerun-if-changed=build.rs"]
    )
    executed = json.dumps({**json.loads(executed), "out_dir": lane_out})
    _write(Path(lane_out).parent / "output", "cargo:rerun-if-changed=build.rs\n")

    result = _read(workspace, script, helper, executed, root_unit)

    # The lane's own unit, not the other lane's data file.
    assert _relative(root, result.files) == [
        "runtime/crate/build.rs",
        "runtime/crate/src/lib.rs",
        "vendor/zlib/src/lib.rs",
    ]


def test_an_ambiguous_uplifted_unit_is_refused(workspace) -> None:
    root, crate, target = workspace
    deps = target / "deps"
    out_dir = os.fspath(target / "build" / "crate-1" / "out")
    for metadata in ("aaaaaaaaaaaaaaaa", "bbbbbbbbbbbbbbbb"):
        _write(
            deps / f"crate-{metadata}.d",
            f"x: runtime/crate/src/lib.rs\n\n# env-dep:OUT_DIR={out_dir}\n",
        )
    helper = _unit(deps, root / "vendor" / "zlib", "zlib", ["vendor/zlib/src/lib.rs"])
    _write(Path(out_dir).parent / "output", "cargo:rerun-if-changed=build.rs\n")
    executed = json.dumps(
        {
            "reason": "build-script-executed",
            "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
            "out_dir": out_dir,
        }
    )
    uplifted = _write(target / "release" / "libcrate.a")
    root_unit = json.dumps(
        {
            "reason": "compiler-artifact",
            "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
            "target": {"kind": ["staticlib"], "name": "crate"},
            "filenames": [os.fspath(uplifted)],
        }
    )
    with pytest.raises(reads.CargoBuildReadsError, match="several dep-info"):
        _read(workspace, helper, executed, root_unit)
