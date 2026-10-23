from __future__ import annotations

import json
import hashlib
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt.cli import cargo_build_reads as reads
from molt.process_environment import NameRule, ProcessEnvironment

ROOT = Path(__file__).resolve().parents[2]


def _write(path: Path, text: str = "x\n") -> Path:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")
    return path


def _escape(entry: str) -> str:
    return entry.replace(" ", "\\ ")


def _env_line(name: str, value: str) -> str:
    def encoded(text: str) -> str:
        return text.replace("\\", "\\\\").replace("\n", "\\n").replace("\r", "\\r")

    return f"# env-dep:{encoded(name)}={encoded(value)}"


def _unit(
    deps: Path,
    package: Path | None,
    crate: str,
    entries: list[str],
    *,
    kind: str = "lib",
    environment: dict[str, str | None] | None = None,
    registry: str | None = None,
) -> str:
    """A compiler-artifact message and the rustc dep-info beside its outputs."""
    stem = f"{crate}-0123456789abcdef"
    output = deps / (f"lib{stem}.rlib" if kind == "lib" else stem)
    dep_info = deps / f"{stem}.d"
    lines = [f"{_escape(os.fspath(output))}: {' '.join(map(_escape, entries))}", ""]
    lines += [f"{_escape(entry)}:" for entry in entries]
    lines += [""] + [
        _env_line(k, v) if v is not None else _env_line(k, "").removesuffix("=")
        for k, v in (environment or {}).items()
    ]
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
            **({"manifest_path": os.fspath(package / "Cargo.toml")} if package else {}),
        }
    )


def _script(
    build: Path,
    package: Path,
    name: str,
    lines: list[str],
    *,
    generated: dict[str, str] | None = None,
    registry: str | None = None,
) -> str:
    out_dir = build / f"{name}-1" / "out"
    _write(out_dir.parent / "output", "".join(f"{line}\n" for line in lines))
    return json.dumps(
        {
            "reason": "build-script-executed",
            "package_id": f"registry+https://github.com/rust-lang/crates.io-index#{registry}"
            if registry
            else f"path+{package.as_uri()}#{name}@0.1.0",
            "out_dir": os.fspath(out_dir),
            "env": [list(row) for row in (generated or {}).items()],
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
    _write(crate / "Cargo.toml", '[package]\nname = "crate"\nversion = "0.1.0"\n')
    return root, crate, target


def _successful_stdout(*messages: str) -> str:
    return "\n".join(
        (*messages, json.dumps({"reason": "build-finished", "success": True}))
    )


def _read_transcript(workspace, stdout: str, toolchain_roots=(), environment=None):
    root, _crate, target = workspace
    return reads.cargo_build_reads(
        stdout,
        workspace_root=root,
        dep_info_base=root,
        product_roots=(target,),
        toolchain_roots=toolchain_roots,
        environment=ProcessEnvironment({}) if environment is None else environment,
    )


def _read(workspace, *messages: str, toolchain_roots=(), environment=None):
    return _read_transcript(
        workspace,
        _successful_stdout(*messages),
        toolchain_roots=toolchain_roots,
        environment=environment,
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
        _script(
            target / "debug" / "build",
            crate,
            "crate",
            ["cargo:rerun-if-changed=build.rs"],
        ),
    ]

    environment = ProcessEnvironment({"UNSAFE_OBJC2_ALLOW_CLASS_OVERRIDE": "1"})
    result = _read(workspace, *messages, environment=environment)

    assert _relative(root, result.files) == [
        "runtime/crate/build.rs",
        "runtime/crate/src/data file.json",
        "runtime/crate/src/lib.rs",
        "vendor/zlib/src/lib.rs",
    ]
    # Registry sources are Cargo.lock's; the variables they read are not.
    assert result.environment.variables == (
        ("UNSAFE_OBJC2_ALLOW_CLASS_OVERRIDE", hashlib.sha256(b"1").hexdigest()),
    )
    assert result.environment.local_unit_count == 2
    result.environment.verify(environment)


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
    assert _relative(root, result.directories) == ["include/shared", "vendor/object"]
    assert result.environment.variables == (("LIBSQLITE3_FLAGS", None),)


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
        _read(workspace)


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize("separator", ["\n", "\r\n"])
def test_complete_successful_build_retains_all_units_and_allows_tool_text(
    workspace, rule, separator
) -> None:
    _root, crate, target = workspace
    local = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    external = json.loads(
        _unit(
            target / "deps",
            None,
            "external",
            ["/registry/external/src/lib.rs"],
            registry="external@1.0.0",
            environment={"TRANSITIVE_INPUT": "observed"},
        )
    )
    # JSON strings may contain Unicode line separators without escaping.
    external["extra"] = "first\u2028second\u0085third"
    stdout = separator.join(
        (
            "ordinary tool output before Cargo records",
            local,
            '{"unrelated_tool_json":true}',
            "ordinary tool output between Cargo records",
            json.dumps(external, ensure_ascii=False),
            '{"reason":"build-finished","success":true}',
            "ordinary tool output after the completed build",
        )
    )
    environment = ProcessEnvironment({"TRANSITIVE_INPUT": "observed"}, rule=rule)
    result = _read_transcript(workspace, stdout, environment=environment)
    assert result.environment.variables == (
        ("TRANSITIVE_INPUT", hashlib.sha256(b"observed").hexdigest()),
    )
    assert result.environment.local_unit_count == 1


@pytest.mark.parametrize(
    "tail, error",
    [
        ("", "no successful terminal"),
        ('{"reason":"build-finished","success":true', "malformed Cargo JSON"),
        ('{"reason":"build-finished"}', "does not report success:true"),
        ('{"reason":"build-finished","success":false}', "does not report success:true"),
        ('{"reason":"build-finished","success":1}', "does not report success:true"),
        (
            '{"reason":"build-finished","success":"true"}',
            "does not report success:true",
        ),
        (
            '{"reason":"build-finished","success":false,"success":true}',
            "malformed Cargo JSON",
        ),
        (
            '{"reason":"compiler-artifact","reason":"ignored"}\n'
            '{"reason":"build-finished","success":true}',
            "malformed Cargo JSON",
        ),
        (
            '{"reason":"build-finished","success":true}\n'
            '{"reason":"build-finished","success":true}',
            "JSON follows build-finished",
        ),
    ],
)
def test_a_valid_unit_prefix_cannot_establish_a_complete_successful_build(
    workspace, tail, error
) -> None:
    _root, crate, target = workspace
    unit = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    with pytest.raises(reads.CargoBuildReadsError, match=error):
        _read_transcript(workspace, unit + "\n" + tail)


@pytest.mark.parametrize("corruption", ["truncated-unit", "unit-after-terminal"])
@pytest.mark.parametrize("unit_kind", ["compiler-artifact", "build-script-executed"])
def test_a_later_transitive_environment_unit_cannot_be_silently_omitted(
    workspace, corruption, unit_kind
) -> None:
    _root, crate, target = workspace
    local = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    external = (
        _unit(
            target / "deps",
            None,
            "external",
            ["/registry/external/src/lib.rs"],
            registry="external@1.0.0",
            environment={"TRANSITIVE_INPUT": "observed"},
        )
        if unit_kind == "compiler-artifact"
        else _script(
            target / "build",
            crate,
            "external",
            ["cargo:rerun-if-env-changed=TRANSITIVE_INPUT"],
            registry="external@1.0.0",
        )
    )
    stdout = (
        _successful_stdout(local, external[:-1])
        if corruption == "truncated-unit"
        else _successful_stdout(local) + "\n" + external
    )
    environment = ProcessEnvironment({"TRANSITIVE_INPUT": "observed"})
    with pytest.raises(
        reads.CargoBuildReadsError,
        match="malformed Cargo JSON|JSON follows build-finished",
    ):
        _read_transcript(workspace, stdout, environment=environment)


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
                environment=ProcessEnvironment({"CARGO_TARGET_DIR": os.fspath(target)})
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
        SimpleNamespace(stdout=_successful_stdout(unit), stderr="", returncode=0),
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
    observed = runtime_native_build._runtime_build_reads(plan, build)
    assert (
        reads.paths_outside(
            (*observed.files, *observed.directories),
            ROOT,
            (
                *runtime_native_build.runtime_source_paths(
                    ROOT, plan.fingerprint_features
                ),
            ),
        )
        == ()
    )


@pytest.mark.parametrize("uncovered", ["README.md", "src/molt/dx.py"])
def test_publication_refuses_a_build_input_the_identity_misses(
    tmp_path, monkeypatch, uncovered
) -> None:
    from molt.cli import runtime_native_build

    plan, build = _native_plan(tmp_path, ["runtime/molt-runtime/src/lib.rs", uncovered])
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
    assert uncovered in str(failures[0]["summary"])


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
        _write(deps / f"libcrate-{metadata}.a")
        _write(
            deps / f"crate-{metadata}.d",
            f"{_escape(os.fspath(deps / f'libcrate-{metadata}.a'))}: {_escape(source)}\n"
            "\n" + _env_line("OUT_DIR", out_dir) + "\n",
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
    for metadata, source in (
        ("aaaaaaaaaaaaaaaa", "runtime/crate/src/lib.rs"),
        ("bbbbbbbbbbbbbbbb", "runtime/crate/src/data file.json"),
    ):
        output = _write(deps / f"libcrate-{metadata}.a")
        _write(
            deps / f"crate-{metadata}.d",
            f"{_escape(os.fspath(output))}: {_escape(source)}\n\n"
            + _env_line("OUT_DIR", out_dir)
            + "\n",
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


def _uplifted_candidate(
    deps: Path,
    metadata: str | None,
    entries: list[str],
    outputs: dict[str, str],
    *,
    environment: dict[str, str] | None = None,
) -> Path:
    """Actual rustc shape: dep-info and each emitted output name the reads."""
    stem = f"crate-{metadata}" if metadata is not None else "crate"
    dep_info = deps / f"{stem}.d"
    targets = [dep_info]
    for suffix, contents in outputs.items():
        targets.append(_write(deps / f"lib{stem}{suffix}", contents))
    sources = " ".join(map(_escape, entries))
    lines = [f"{_escape(os.fspath(target))}: {sources}" for target in targets]
    lines.extend(_env_line(name, value) for name, value in (environment or {}).items())
    return _write(dep_info, "\n".join(lines) + "\n")


def _uplifted_message(outputs: list[Path], kind: str = "staticlib"):
    return {
        "target": {"kind": [kind], "name": "crate"},
        "filenames": list(map(os.fspath, outputs)),
    }


@pytest.mark.parametrize("different_output", [None, ".a", ".wasm"])
def test_unhashed_combined_wasm_reads_bind_every_uplifted_product(
    workspace, different_output
):
    root, crate, target = workspace
    deps = target / "captured-generation" / "wasm32-wasip1" / "dev-fast" / "deps"
    final = target / "wasm32-wasip1" / "dev-fast"
    sources = ["runtime/crate/src/lib.rs", "runtime/crate/src/data file.json"]
    dep_info = deps / "crate.d"
    outputs = [
        _write(deps / "libcrate.a", "archive bytes"),
        _write(deps / "crate.wasm", "module bytes"),
    ]
    out_dir = target / "build" / "crate-1" / "out"
    _write(
        dep_info,
        "\n".join(
            [
                f"{_escape(os.fspath(path))}: {' '.join(map(_escape, sources))}"
                for path in (dep_info, *outputs)
            ]
            + [_env_line("OUT_DIR", os.fspath(out_dir))]
        )
        + "\n",
    )
    products = [
        _write(
            final / path.name,
            "different bytes"
            if path.suffix == different_output
            else path.read_text(encoding="utf-8"),
        )
        for path in outputs
    ]
    # Cargo reports this root only uplifted; a sibling unit establishes where
    # this invocation actually put intermediate outputs, independently of final.
    helper = _unit(deps, root / "vendor" / "zlib", "zlib", ["vendor/zlib/src/lib.rs"])
    executed = _script(
        target / "build", crate, "crate", ["cargo:rerun-if-changed=build.rs"]
    )
    root_unit = {
        **_uplifted_message(products),
        "target": {"kind": ["staticlib", "cdylib"], "name": "crate"},
        "reason": "compiler-artifact",
        "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
    }
    if different_output is not None:
        with pytest.raises(
            reads.CargoBuildReadsError, match="no dep-info output matches"
        ):
            _read(workspace, helper, executed, json.dumps(root_unit))
    else:
        result = _read(workspace, helper, executed, json.dumps(root_unit))
        assert _relative(root, result.files) == [
            "runtime/crate/build.rs",
            "runtime/crate/src/data file.json",
            "runtime/crate/src/lib.rs",
            "vendor/zlib/src/lib.rs",
        ]


def test_copied_output_selects_actual_unit_despite_equal_out_dir_and_size(workspace):
    root, crate, target = workspace
    deps = target / "dev-fast" / "deps"
    out_dir = os.fspath(target / "build" / "crate-1" / "out")
    _uplifted_candidate(
        deps,
        "old",
        ["runtime/crate/src/data file.json"],
        {".a": "old bytes"},
        environment={"OUT_DIR": out_dir},
    )
    _uplifted_candidate(
        deps,
        "new",
        ["runtime/crate/src/lib.rs"],
        {".a": "new bytes"},
        environment={"OUT_DIR": out_dir},
    )
    uplifted = _write(target / "dev-fast" / "libcrate.a", "new bytes")
    helper = _unit(deps, root / "vendor" / "zlib", "zlib", ["vendor/zlib/src/lib.rs"])
    executed = _script(
        target / "build", crate, "crate", ["cargo:rerun-if-changed=build.rs"]
    )
    root_unit = {
        **_uplifted_message([uplifted]),
        "reason": "compiler-artifact",
        "package_id": f"path+{crate.as_uri()}#crate@0.1.0",
    }

    # A newer competing output must not win; nor may equal file sizes bind it.
    os.utime(deps / "libcrate-old.a", ns=(2_000_000_000_000_000_000,) * 2)
    result = _read(workspace, helper, executed, json.dumps(root_unit))
    assert _relative(root, result.files) == [
        "runtime/crate/build.rs",
        "runtime/crate/src/lib.rs",
        "vendor/zlib/src/lib.rs",
    ]


@pytest.mark.parametrize("difference", ["files", "environment"])
@pytest.mark.parametrize("metadata", ["a", None], ids=["hashed", "unhashed"])
def test_identical_output_with_different_read_closures_refuses(
    tmp_path, difference, metadata
):
    deps = tmp_path / "deps"
    _uplifted_candidate(deps, metadata, ["src/lib.rs"], {".a": "same bytes"})
    _uplifted_candidate(
        deps,
        "b",
        ["src/other.rs"] if difference == "files" else ["src/lib.rs"],
        {".a": "same bytes"},
        environment={"FEATURE": "1"} if difference == "environment" else {},
    )
    output = _write(tmp_path / "libcrate.a", "same bytes")
    with pytest.raises(reads.CargoBuildReadsError, match="different reads"):
        reads._unit_dep_info(
            _uplifted_message([output]), (deps,), None, dep_info_base=tmp_path
        )


@pytest.mark.parametrize("metadata", ["a", None], ids=["hashed", "unhashed"])
def test_identical_output_and_normalized_reads_coalesce_deterministically(
    tmp_path, metadata
):
    deps = tmp_path / "deps"
    expected = _uplifted_candidate(
        deps,
        metadata,
        ["src/lib.rs", "src/other.rs"],
        {".a": "same bytes"},
        environment={"MODE": "1"},
    )
    equivalent = _uplifted_candidate(
        deps,
        "b",
        [os.fspath(tmp_path / "src/other.rs"), "src/lib.rs", "src/lib.rs"],
        {".a": "same bytes"},
        environment={"MODE": "1"},
    )
    output = _write(tmp_path / "libcrate.a", "same bytes")
    record = reads._unit_dep_info(
        _uplifted_message([output]), (deps,), None, dep_info_base=tmp_path
    )
    assert record is not None
    assert record.path == min(expected, equivalent)


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_arbitrary_ambient_inputs_bind_frozen_values_and_absence(
    workspace, monkeypatch, rule
):
    _root, crate, target = workspace
    environment = ProcessEnvironment(
        {"CUSTOM_SETTING": "private", "EMPTY": "", "CARGO_UNREGISTERED": "ordinary"},
        rule=rule,
    )
    message = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={
            "CUSTOM_SETTING": "private",
            "EMPTY": "",
            "MISSING": None,
            "CARGO_UNREGISTERED": "ordinary",
        },
    )
    monkeypatch.setenv("CUSTOM_SETTING", "unrelated-global")
    result = _read(workspace, message, environment=environment)
    result.environment.verify(environment)
    assert [name for name, _ in result.environment.variables] == [
        "CARGO_UNREGISTERED",
        "CUSTOM_SETTING",
        "EMPTY",
        "MISSING",
    ]
    with pytest.raises(ValueError, match="CUSTOM_SETTING"):
        result.environment.verify(environment.updated({"CUSTOM_SETTING": "changed"}))
    with pytest.raises(ValueError, match="MISSING"):
        result.environment.verify(environment.updated({"MISSING": ""}))


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize("declared", [False, True])
def test_generated_override_keeps_its_declaration_provenance(workspace, rule, declared):
    _root, crate, target = workspace
    environment = ProcessEnvironment({"VALUE": "ambient"}, rule=rule)
    unit = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={"VALUE": "generated"},
    )
    script = _script(
        target / "build",
        crate,
        "crate",
        ["cargo:rerun-if-changed=build.rs"]
        + (["cargo:rerun-if-env-changed=VALUE"] if declared else []),
        generated={"VALUE": "generated"},
    )
    result = _read(workspace, unit, script, environment=environment)
    if declared:
        assert result.environment.variables == (
            ("VALUE", hashlib.sha256(b"ambient").hexdigest()),
        )
        with pytest.raises(ValueError, match="VALUE"):
            result.environment.verify(environment.updated({"VALUE": "changed"}))
    else:
        assert result.environment.variables == ()
        result.environment.verify(environment.updated({"VALUE": "changed"}))


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_registry_script_and_rustc_inputs_are_both_observed(workspace, rule):
    _root, crate, target = workspace
    registry = "external@1.2.3"
    environment = ProcessEnvironment({"TRANSITIVE": "one", "DECLARED": ""}, rule=rule)
    local = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    external = _unit(
        target / "deps",
        None,
        "external",
        ["/registry/external/src/lib.rs"],
        environment={"TRANSITIVE": "one", "GENERATED": "script-value"},
        registry=registry,
    )
    script = _script(
        target / "build",
        crate,
        "external",
        ["cargo::rerun-if-env-changed=DECLARED"],
        registry=registry,
        generated={"GENERATED": "script-value"},
    )
    result = _read(workspace, local, external, script, environment=environment)
    assert [name for name, _ in result.environment.variables] == [
        "DECLARED",
        "TRANSITIVE",
    ]
    with pytest.raises(ValueError, match="DECLARED, TRANSITIVE"):
        result.environment.verify(ProcessEnvironment({}, rule=rule))


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_conflicting_ambient_unit_facts_cannot_be_last_wins(workspace, rule):
    _root, crate, target = workspace
    messages = [
        _unit(
            target / "deps",
            crate,
            name,
            ["runtime/crate/src/lib.rs"],
            environment={"VALUE": value},
        )
        for name, value in (("first", "stale"), ("second", "current"))
    ]
    with pytest.raises(reads.CargoBuildReadsError, match="VALUE"):
        _read(
            workspace,
            *messages,
            environment=ProcessEnvironment({"VALUE": "current"}, rule=rule),
        )


def test_out_dir_must_belong_to_the_actual_package_script(workspace):
    _root, crate, target = workspace
    unit = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={"OUT_DIR": os.fspath(target / "unrelated" / "out")},
    )
    script = _script(
        target / "build", crate, "crate", ["cargo:rerun-if-changed=build.rs"]
    )
    with pytest.raises(reads.CargoBuildReadsError, match="OUT_DIR"):
        _read(workspace, unit, script)


def test_cargo_manifest_and_target_values_have_closed_owners(workspace):
    _root, crate, target = workspace
    unit = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={
            "CARGO_PKG_NAME": "crate",
            "CARGO_PKG_VERSION": "0.1.0",
            "CARGO_PKG_VERSION_MINOR": "1",
            "CARGO_CRATE_NAME": "crate",
            "CARGO_MANIFEST_DIR": os.fspath(crate),
        },
    )
    result = _read(workspace, unit)
    assert result.environment.variables == ()
    forged = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={"CARGO_PKG_UNKNOWN": "unowned"},
    )
    with pytest.raises(reads.CargoBuildReadsError, match="CARGO_PKG_UNKNOWN"):
        _read(workspace, forged)


def test_registry_unit_without_dep_info_cannot_attest_environment_coverage(workspace):
    _root, crate, target = workspace
    local = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    external = json.loads(
        _unit(target / "deps", None, "external", [], registry="external@1.0.0")
    )
    (target / "deps" / "external-0123456789abcdef.d").unlink()
    with pytest.raises(reads.CargoBuildReadsError, match="no dep-info"):
        _read(workspace, local, json.dumps(external))


def test_relative_dep_info_uses_explicit_nested_base_and_admits_sibling_sources(
    workspace,
):
    root, _crate, target = workspace
    nested = root / "tools" / "supervisor"
    local_source = _write(nested / "src" / "main.rs")
    sibling = _write(root / "runtime" / "sibling.rs")
    message = _unit(
        target / "deps",
        nested,
        "supervisor",
        ["src/main.rs", "../../runtime/sibling.rs"],
    )
    result = reads.cargo_build_reads(
        _successful_stdout(message),
        workspace_root=root,
        dep_info_base=nested,
        product_roots=(target,),
        toolchain_roots=(),
        environment=ProcessEnvironment({}),
    )
    assert set(result.files) == {local_source.resolve(), sibling.resolve()}
    with pytest.raises(reads.CargoBuildReadsError, match="outside"):
        reads.cargo_build_reads(
            _successful_stdout(message),
            workspace_root=nested,
            dep_info_base=nested,
            product_roots=(target,),
            toolchain_roots=(),
            environment=ProcessEnvironment({}),
        )


def test_uplifted_relative_source_equivalence_uses_the_explicit_base(tmp_path):
    base = tmp_path / "source" / "nested"
    source = _write(base / "src" / "lib.rs")
    deps = tmp_path / "target" / "deps"
    first = _uplifted_candidate(deps, "first", ["src/lib.rs"], {".a": "same bytes"})
    _uplifted_candidate(deps, "second", [os.fspath(source)], {".a": "same bytes"})
    output = _write(tmp_path / "target" / "libcrate.a", "same bytes")
    record = reads._unit_dep_info(
        _uplifted_message([output]), (deps,), None, dep_info_base=base
    )
    assert record is not None and record.path == first


def test_generated_values_do_not_leak_between_packages(workspace):
    root, crate, target = workspace
    other = root / "other"
    _write(other / "src" / "lib.rs")
    messages = []
    for package, name, value, source in (
        (crate, "crate", "one", "runtime/crate/src/lib.rs"),
        (other, "other", "two", "other/src/lib.rs"),
    ):
        messages.extend(
            [
                _unit(
                    target / "deps",
                    package,
                    name,
                    [source],
                    environment={"VALUE": value},
                ),
                _script(
                    target / "build",
                    package,
                    name,
                    ["cargo:rerun-if-changed=src/lib.rs"],
                    generated={"VALUE": value},
                ),
            ]
        )
    result = _read(workspace, *messages)
    assert result.environment.local_unit_count == 2
    assert result.environment.variables == ()


def test_out_dir_selects_generated_values_from_the_same_package_unit(workspace):
    _root, crate, target = workspace
    scripts = []
    package_id = f"path+{crate.as_uri()}#crate@0.1.0"
    for name, value in (("first", "one"), ("second", "two")):
        script = json.loads(
            _script(
                target / "build",
                crate,
                name,
                ["cargo:rerun-if-changed=build.rs"],
                generated={"VALUE": value},
            )
        )
        script["package_id"] = package_id
        scripts.append(json.dumps(script))
    unit = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={
            "VALUE": "one",
            "OUT_DIR": os.fspath(target / "build" / "first-1" / "out"),
        },
    )
    result = _read(workspace, unit, *reversed(scripts))
    assert result.environment.variables == ()
    ambiguous = _unit(
        target / "deps",
        crate,
        "crate",
        ["runtime/crate/src/lib.rs"],
        environment={"VALUE": "one"},
    )
    with pytest.raises(reads.CargoBuildReadsError, match="ambiguous"):
        _read(workspace, ambiguous, *scripts)


def test_build_script_compilation_does_not_receive_its_runtime_overrides(workspace):
    _root, crate, target = workspace
    unit = json.loads(
        _unit(
            target / "build" / "crate-0123456789abcdef",
            crate,
            "build_script_build",
            ["runtime/crate/build.rs"],
            kind="custom-build",
            environment={"VALUE": "ambient"},
        )
    )
    unit["package_id"] = f"path+{crate.as_uri()}#crate@0.1.0"
    script = _script(
        target / "build",
        crate,
        "crate",
        ["cargo:rerun-if-changed=build.rs"],
        generated={"VALUE": "generated"},
    )
    environment = ProcessEnvironment({"VALUE": "ambient"})
    result = _read(workspace, json.dumps(unit), script, environment=environment)
    assert result.environment.variables == (
        ("VALUE", hashlib.sha256(b"ambient").hexdigest()),
    )


def test_generated_case_alias_conflicts_follow_the_frozen_name_rule(workspace):
    _root, crate, target = workspace
    unit = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    script = _script(
        target / "build",
        crate,
        "crate",
        ["cargo:rerun-if-changed=build.rs"],
        generated={"VALUE": "one", "value": "two"},
    )
    _read(
        workspace, unit, script, environment=ProcessEnvironment({}, rule=NameRule.EXACT)
    )
    with pytest.raises(reads.CargoBuildReadsError, match="conflicting.*VALUE"):
        _read(
            workspace,
            unit,
            script,
            environment=ProcessEnvironment({}, rule=NameRule.WINDOWS),
        )


def test_duplicate_rustc_fact_cannot_replace_an_earlier_value(tmp_path):
    info = _write(
        tmp_path / "unit.d", "unit: src.rs\n# env-dep:VALUE=old\n# env-dep:VALUE=new\n"
    )
    with pytest.raises(reads.CargoBuildReadsError, match="conflicting.*VALUE"):
        reads._read_dep_info(info)


@pytest.mark.parametrize(
    "suffix,kind",
    [
        (".a", "staticlib"),
        (".lib", "staticlib"),
        (".dll", "cdylib"),
        (".dylib", "cdylib"),
        (".so", "cdylib"),
        (".wasm", "cdylib"),
        (".exe", "bin"),
        ("", "bin"),
    ],
)
@pytest.mark.parametrize("metadata", ["one", None], ids=["hashed", "unhashed"])
def test_actual_output_binding_is_independent_of_host_format(
    tmp_path, suffix, kind, metadata
):
    deps = tmp_path / "deps"
    expected = _uplifted_candidate(
        deps, metadata, ["src/lib.rs"], {suffix: "format bytes"}
    )
    output = _write(tmp_path / ("crate" + suffix), "format bytes")
    record = reads._unit_dep_info(
        _uplifted_message([output], kind), (deps,), None, dep_info_base=tmp_path
    )
    assert record is not None
    assert record.path == expected


def test_every_represented_primary_output_must_match(tmp_path):
    deps = tmp_path / "deps"
    expected = _uplifted_candidate(
        deps, "new", ["src/lib.rs"], {".rlib": "new library", ".rmeta": "metadata"}
    )
    _uplifted_candidate(
        deps, "old", ["src/other.rs"], {".rlib": "old library", ".rmeta": "metadata"}
    )
    library = _write(tmp_path / "libcrate.rlib", "new library")
    metadata = _write(tmp_path / "libcrate.rmeta", "metadata")
    pdb = _write(tmp_path / "crate.pdb", "debug")
    record = reads._unit_dep_info(
        _uplifted_message([library, metadata, pdb], "lib"),
        (deps,),
        None,
        dep_info_base=tmp_path,
    )
    assert record is not None
    assert record.path == expected


def test_unique_out_dir_does_not_bind_different_output_bytes(tmp_path):
    deps = tmp_path / "deps"
    _uplifted_candidate(
        deps, "one", ["src/lib.rs"], {".a": "wrong"}, environment={"OUT_DIR": "/out"}
    )
    output = _write(tmp_path / "libcrate.a", "right")
    with pytest.raises(reads.CargoBuildReadsError, match="no dep-info output matches"):
        reads._unit_dep_info(
            _uplifted_message([output]), (deps,), "/out", dep_info_base=tmp_path
        )


def test_hardlinked_output_uses_owned_file_identity_without_hashing(
    tmp_path, monkeypatch
):
    deps = tmp_path / "deps"
    expected = _uplifted_candidate(deps, "one", ["src/lib.rs"], {".a": "same inode"})
    output = tmp_path / "libcrate.a"
    try:
        output.hardlink_to(deps / "libcrate-one.a")
    except OSError as exc:
        pytest.skip(f"host cannot create a hard link: {exc}")

    def forbidden_hash(*_args, **_kwargs):
        raise AssertionError("same owned file must not require a content hash")

    monkeypatch.setattr(reads, "stable_regular_file_handle_identity", forbidden_hash)
    record = reads._unit_dep_info(
        _uplifted_message([output]), (deps,), None, dep_info_base=tmp_path
    )
    assert record is not None
    assert record.path == expected


def test_copy_hashes_final_once_and_skips_different_sizes(tmp_path, monkeypatch):
    deps = tmp_path / "deps"
    _uplifted_candidate(deps, "a", ["src/lib.rs"], {".a": "same bytes"})
    _uplifted_candidate(deps, "b", ["src/lib.rs"], {".a": "same bytes"})
    _uplifted_candidate(deps, "c", ["src/other.rs"], {".a": "different size"})
    output = _write(tmp_path / "libcrate.a", "same bytes")
    hashed = []
    original = reads.stable_regular_file_handle_identity

    def observe(opened, **kwargs):
        hashed.append(opened.path)
        return original(opened, **kwargs)

    monkeypatch.setattr(reads, "stable_regular_file_handle_identity", observe)
    record = reads._unit_dep_info(
        _uplifted_message([output]), (deps,), None, dep_info_base=tmp_path
    )
    assert record is not None
    assert hashed.count(output) == 1
    assert set(hashed) == {output, deps / "libcrate-a.a", deps / "libcrate-b.a"}


@pytest.mark.parametrize("changed", ["final", "candidate"])
def test_mutation_during_output_identity_refuses(tmp_path, monkeypatch, changed):
    deps = tmp_path / "deps"
    _uplifted_candidate(deps, "one", ["src/lib.rs"], {".a": "same bytes"})
    output = _write(tmp_path / "libcrate.a", "same bytes")
    target = output if changed == "final" else deps / "libcrate-one.a"
    original = reads.stable_regular_file_handle_identity

    def mutate(opened, **kwargs):
        result = original(opened, **kwargs)
        if opened.path == target:
            target.write_text("evil bytes", encoding="utf-8")
        return result

    monkeypatch.setattr(reads, "stable_regular_file_handle_identity", mutate)
    with pytest.raises(reads.CargoBuildReadsError, match="changed"):
        reads._unit_dep_info(
            _uplifted_message([output]), (deps,), None, dep_info_base=tmp_path
        )


@pytest.mark.parametrize("declaration", ["directory", "package"])
@pytest.mark.parametrize("populated", [False, True])
def test_publication_requires_the_watched_root_not_only_present_files(
    workspace, monkeypatch, declaration: str, populated: bool
) -> None:
    from molt.cli import runtime_build_identity, runtime_native_build

    root, crate, target = workspace
    watched = root / "include" / "new-headers" if declaration == "directory" else crate
    watched.mkdir(parents=True, exist_ok=True)
    if populated:
        _write(watched / "existing.h")
    # Individual current files cannot cover future additions to a watched root.
    covered = tuple(path for path in root.rglob("*") if path.is_file())
    roots = tuple((path.relative_to(root).as_posix(), path) for path in covered)
    before = runtime_build_identity._tree_identity(roots, require_all=True)
    message = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    script = _script(
        target / "build",
        crate,
        "crate",
        ["cargo:rerun-if-changed=../../include/new-headers"]
        if declaration == "directory"
        else [],
    )
    plan = SimpleNamespace(
        cargo_plan=SimpleNamespace(
            environment=ProcessEnvironment({"CARGO_TARGET_DIR": os.fspath(target)})
        ),
        project_root=root,
        fingerprint_features=(),
        runtime_state=None,
        cmd=["cargo", "rustc"],
        json_output=True,
    )
    build = SimpleNamespace(
        stdout=_successful_stdout(message, script), stderr="", returncode=0
    )
    monkeypatch.setattr(
        runtime_native_build, "runtime_source_paths", lambda *_: covered
    )
    failures = []

    def refuse(_state, **fields):
        failures.append(fields)
        return False

    def publish(*_args, **_kwargs):
        pytest.fail("a generation with an uncovered watched root reached publication")

    monkeypatch.setattr(runtime_native_build, "_record_native_runtime_failure", refuse)
    monkeypatch.setattr(
        runtime_native_build, "publish_native_runtime_generation", publish
    )
    assert runtime_native_build._publish_native_runtime_build(plan, build) is False
    assert failures[0]["stage"] == "build-input-coverage"
    assert watched.relative_to(root).as_posix() in failures[0]["summary"]
    _write(watched / "added.h")
    assert runtime_build_identity._tree_identity(roots, require_all=True) == before

    # A recursive identity root covers additions without enumerating files twice.
    monkeypatch.setattr(
        runtime_native_build, "runtime_source_paths", lambda *_: (root,)
    )
    observed = runtime_native_build._runtime_build_reads(plan, build)
    assert watched.resolve() in observed.directories
    assert (
        reads.paths_outside((*observed.files, *observed.directories), root, (root,))
        == ()
    )


def test_uplifted_candidate_decodes_literal_windows_out_dir(tmp_path: Path) -> None:
    output = _write(tmp_path / "libcrate-1.a")
    dep_info = _write(
        tmp_path / "crate-1.d",
        f"{_escape(os.fspath(output))}: src/lib.rs\n"
        r"# env-dep:OUT_DIR=C:\\build\\target\\out" + "\n",
    )
    uplifted = _write(tmp_path / "release" / "libcrate.a")
    message = {
        "target": {"kind": ["staticlib"], "name": "crate"},
        "filenames": [os.fspath(uplifted)],
    }
    record = reads._unit_dep_info(
        message, (tmp_path,), r"C:\build\target\out", dep_info_base=tmp_path
    )
    assert record is not None
    assert record.path == dep_info


def test_dep_info_decodes_environment_names_and_values(tmp_path: Path) -> None:
    dep_info = _write(
        tmp_path / "unit.d",
        "out: src/lib.rs\tinclude.h\n# env-dep:LINE\\nNAME=line1\\nline2\\r\\\\\n",
    )
    record = reads._read_dep_info(dep_info)
    assert record.files == ("src/lib.rs", "include.h")
    assert record.environment == {"LINE\nNAME": "line1\nline2\r\\"}


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
@pytest.mark.parametrize("ending", ["\n", "\r\n"])
def test_dep_info_preserves_literal_separators_in_observed_environment(
    workspace, rule, ending
) -> None:
    _root, crate, target = workspace
    message = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    output = json.loads(message)["filenames"][0]
    name = "INPUT\u2028NAME"
    value = "one\u0085two\u2029three\vfour\ffive\x1csix\x1dseven\x1eeight"
    # Physical rustc records use LF/CRLF; these other separators are literal
    # UTF-8 data, not line endings or escapes in the dep-info protocol.
    (target / "deps" / "crate-0123456789abcdef.d").write_bytes(
        (
            f"{_escape(output)}: runtime/crate/src/lib.rs{ending}"
            f"# env-dep:{name}={value}{ending}"
        ).encode("utf-8")
    )
    environment = ProcessEnvironment({name: value}, rule=rule)
    result = _read(workspace, message, environment=environment)
    assert result.environment.variables == (
        (rule.key(name), hashlib.sha256(value.encode("utf-8")).hexdigest()),
    )
    result.environment.verify(environment)
    with pytest.raises(ValueError) as changed:
        result.environment.verify(
            environment.updated({name: value.replace("\u2029", "-")})
        )
    assert rule.key(name) in str(changed.value)


@pytest.mark.parametrize("ending", ["\n", "\r\n"])
def test_build_script_literal_separators_do_not_split_or_invent_directives(
    tmp_path, ending
) -> None:
    path = tmp_path / "output"
    filename = "data\u2028file.h"
    name = "INPUT\u0085MIDDLE\u2029NAME\vPART\fEND"
    path.write_bytes(
        (
            f"  cargo:rerun-if-changed={filename}  {ending}"
            f"  cargo::rerun-if-env-changed={name}  {ending}"
            f"ordinary output\u2028cargo:rerun-if-env-changed=PHANTOM{ending}"
        ).encode("utf-8")
    )
    assert reads._read_build_script_output(path) == ((filename,), (name,))


@pytest.mark.parametrize(
    "line",
    [
        "# env-dep:NAME=bad\\q",
        "# env-dep:NAME=end\\",
        "# env-dep:BAD\\q=value",
        "out: src\\",
    ],
)
def test_malformed_dep_info_is_refused(tmp_path: Path, line: str) -> None:
    dep_info = _write(
        tmp_path / "unit.d",
        "out: src/lib.rs\n" + line + "\n" if line.startswith("#") else line + "\n",
    )
    with pytest.raises(reads.CargoBuildReadsError):
        reads._read_dep_info(dep_info)


def test_local_package_file_uri_preserves_unc_authority(monkeypatch) -> None:
    import nturl2path

    converted = []

    def windows_file_path(url_path):
        converted.append(url_path)
        return nturl2path.url2pathname(url_path)

    monkeypatch.setattr(reads, "url2pathname", windows_file_path)
    actual = reads._local_package_root("path+file://server/share/my%20pkg#pkg@0.1.0")
    assert converted == ["//server/share/my%20pkg"]
    assert actual == Path(os.path.realpath(r"\\server\share\my pkg"))


def test_uninspectable_candidate_cannot_establish_uplifted_uniqueness(tmp_path) -> None:
    _write(tmp_path / "crate-1.d", "x: src/lib.rs\n# env-dep:OUT_DIR=/out\n")
    _write(tmp_path / "crate-2.d", "x: src/lib.rs\n# env-dep:OUT_DIR=/bad\\q\n")
    message = {
        "target": {"kind": ["staticlib"], "name": "crate"},
        "filenames": [os.fspath(tmp_path / "release" / "libcrate.a")],
    }
    with pytest.raises(reads.CargoBuildReadsError):
        reads._unit_dep_info(message, (tmp_path,), "/out", dep_info_base=tmp_path)


def test_build_script_trimming_retains_every_declared_input(workspace) -> None:
    root, crate, target = workspace
    message = _unit(target / "deps", crate, "crate", ["runtime/crate/src/lib.rs"])
    script = _script(
        target / "build",
        crate,
        "crate",
        [
            "cargo:rerun-if-changed=build.rs",
            "  cargo:rerun-if-changed=../../include/api.h  ",
        ],
    )
    result = _read(workspace, message, script)
    assert root.resolve() / "include/api.h" in result.files


def test_populated_output_rules_decode_windows_drives_and_spaces(tmp_path):
    record_path = _write(
        tmp_path / "crate.d",
        "C:\\build\\ lane\\crate-x.d: C:\\src\\ tree\\lib.rs\n"
        "C:\\build\\ lane\\crate-x.lib: C:\\src\\ tree\\lib.rs\n"
        "C:\\src\\ tree\\lib.rs:\n",
    )
    record = reads._read_dep_info(record_path)
    assert record.outputs == (r"C:\build lane\crate-x.d", r"C:\build lane\crate-x.lib")
    assert record.files == (r"C:\src tree\lib.rs",)


def test_populated_output_rules_with_different_sources_refuse(tmp_path):
    dep_info = _write(
        tmp_path / "crate.d",
        "crate.d: src/lib.rs\nlibcrate.a: src/other.rs\n",
    )
    with pytest.raises(
        reads.CargoBuildReadsError, match="output rules have different reads"
    ):
        reads._read_dep_info(dep_info)


@pytest.mark.parametrize("suffix", [".pdb", ".d"])
def test_binary_primary_is_cargo_executable_not_extension(tmp_path, suffix):
    deps = tmp_path / "deps"
    expected = _uplifted_candidate(
        deps, "one", ["src/main.rs"], {suffix: "program bytes"}
    )
    executable = _write(tmp_path / ("tool" + suffix), "program bytes")
    debug = _write(tmp_path / "tool.debug.pdb", "ancillary bytes")
    message = {
        **_uplifted_message([executable, debug], "bin"),
        "executable": str(executable),
    }
    record = reads._unit_dep_info(message, (deps,), None, dep_info_base=tmp_path)
    assert record is not None
    assert record.path == expected


@pytest.mark.parametrize("with_complete_candidate", [False, True])
@pytest.mark.parametrize("metadata", ["new", None], ids=["hashed", "unhashed"])
def test_missing_linked_output_rule_cannot_bind_metadata_only_candidate(
    tmp_path, with_complete_candidate, metadata
):
    deps = tmp_path / "deps"
    _uplifted_candidate(deps, "old", ["src/old.rs"], {".rlib": "shared rlib bytes"})
    expected = None
    if with_complete_candidate:
        expected = _uplifted_candidate(
            deps,
            metadata,
            ["src/lib.rs"],
            {".rlib": "shared rlib bytes", ".dylib": "actual linked bytes"},
        )
    library = _write(tmp_path / "libcrate.rlib", "shared rlib bytes")
    linked = _write(tmp_path / "libcrate.dylib", "actual linked bytes")
    message = {
        **_uplifted_message([library, linked]),
        "target": {"kind": ["rlib", "cdylib"], "name": "crate"},
    }
    if expected is None:
        with pytest.raises(
            reads.CargoBuildReadsError, match="no dep-info output matches"
        ):
            reads._unit_dep_info(message, (deps,), None, dep_info_base=tmp_path)
    else:
        record = reads._unit_dep_info(message, (deps,), None, dep_info_base=tmp_path)
        assert record is not None
        assert record.path == expected


@pytest.mark.parametrize(
    "suffix", [".pdb", ".dSYM", ".dwp", ".dll.lib", ".dll.exp", ".dll.a", ".wasm.map"]
)
def test_library_primary_does_not_require_cargo_auxiliary_output_rules(
    tmp_path, suffix
):
    deps = tmp_path / "deps"
    expected = _uplifted_candidate(
        deps, "one", ["src/lib.rs"], {".dll": "linked bytes"}
    )
    linked = _write(tmp_path / "crate.dll", "linked bytes")
    auxiliary = tmp_path / ("crate" + suffix)
    if suffix == ".dSYM":
        auxiliary.mkdir()
    else:
        _write(auxiliary, "auxiliary bytes")
    record = reads._unit_dep_info(
        _uplifted_message([linked, auxiliary], "cdylib"),
        (deps,),
        None,
        dep_info_base=tmp_path,
    )
    assert record is not None
    assert record.path == expected
