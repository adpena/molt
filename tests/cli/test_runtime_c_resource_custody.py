from __future__ import annotations

import hashlib
import json
import os
import shlex
import shutil
import time
from pathlib import Path

import pytest

from molt.cli.cargo_resource_custody import CargoResourceCustody, CargoResourceRoot
from molt.cli.runtime_cargo_plan import (
    _CargoEnvironment,
    _resolve_c_build_resources,
    _c_tool_environment_names,
    runtime_c_flag_environment_names,
    resolve_runtime_cargo_plan,
)
from molt.cli.cargo_execution import _run_resolved_cargo_plan
from tests.runtime_build_identity_helper import RuntimeFixtureRoot, runtime_cargo_plan
from tests.rust.process_guard import run_rust_test_process


TARGET = "wasm32-wasip1"


def _selection(resource: CargoResourceRoot) -> tuple[tuple[str, Path], ...]:
    return tuple((entry.label, entry.entrypoint) for entry in resource.walk().entries)


def test_llvm_install_directory_backedge_preserves_resource_custody(
    tmp_path: Path,
) -> None:
    root = tmp_path / "llvm-22"
    (root / "build").mkdir(parents=True)
    (root / "lib").mkdir()
    library = root / "lib" / "libLLVM.so.22.1"
    library.write_bytes(b"llvm resource")
    alias = root / "build" / "Debug+Asserts"
    alias.symlink_to("..", target_is_directory=True)
    resource = CargoResourceRoot("llvm/install", root)
    assert _selection(resource) == (("llvm/install/lib/libLLVM.so.22.1", library),)
    custody = CargoResourceCustody.capture((resource,))
    assert custody.content_identity()["file_count"] == 1
    custody.verify()
    library.write_bytes(b"changed resource")
    with pytest.raises(ValueError):
        custody.verify()


def test_directory_backedges_do_not_hide_sibling_aliases_or_new_resources(
    tmp_path: Path,
) -> None:
    root = tmp_path / "install"
    include = root / "include"
    include.mkdir(parents=True)
    header = include / "api.h"
    header.write_bytes(b"header")
    (include / "install").symlink_to("..", target_is_directory=True)
    (root / "alias").symlink_to("include", target_is_directory=True)
    resource = CargoResourceRoot("toolchain", root)
    assert _selection(resource) == (
        ("toolchain/alias/api.h", root / "alias" / "api.h"),
        ("toolchain/include/api.h", header),
    )
    custody = CargoResourceCustody.capture((resource,))
    custody.verify()
    (include / "new.h").write_bytes(b"new resource")
    with pytest.raises(ValueError, match="resource selection changed"):
        custody.verify()


def test_resource_directory_backedge_retargeting_is_fenced(tmp_path: Path) -> None:
    root = tmp_path / "install"
    (root / "build").mkdir(parents=True)
    (root / "input.h").write_bytes(b"header")
    alias = root / "build" / "Debug+Asserts"
    alias.symlink_to("..", target_is_directory=True)
    custody = CargoResourceCustody.capture((CargoResourceRoot("toolchain", root),))
    other = tmp_path / "other"
    other.mkdir()
    (other / "input.h").write_bytes(b"other header")
    alias.unlink()
    alias.symlink_to(other, target_is_directory=True)
    with pytest.raises(ValueError, match="resource selection changed"):
        custody.verify()


def test_c_flag_names_cover_cc_rs_target_and_host_layers() -> None:
    names = runtime_c_flag_environment_names(TARGET)
    assert len(names) == len(set(names))
    assert {
        "CFLAGS",
        "CFLAGS_wasm32-wasip1",
        "CFLAGS_wasm32_wasip1",
        "HOST_CFLAGS",
        "TARGET_CFLAGS",
        "HOST_ARFLAGS",
        "TARGET_ARFLAGS",
        "CC_SHELL_ESCAPED_FLAGS",
    } <= set(names)


@pytest.mark.parametrize(
    "name",
    [
        "CPATH",
        "C_INCLUDE_PATH",
        "CPLUS_INCLUDE_PATH",
        "OBJC_INCLUDE_PATH",
        "LIBRARY_PATH",
        "INCLUDE",
        "LIB",
        "LIBPATH",
        "SDKROOT",
    ],
)
def test_c_search_paths_have_content_and_lifetime_custody(
    tmp_path: Path, name: str
) -> None:
    directory = tmp_path / "headers"
    directory.mkdir()
    header = directory / "value.h"
    header.write_bytes(b"#define VALUE 1\n")
    env = _CargoEnvironment({name: str(directory)})
    before = dict(env)
    plan = _resolve_c_build_resources(env, target=TARGET)
    assert dict(env) == before
    assert plan.environment[name] == (f"${{c/{name}/search/0}}",)
    custody = CargoResourceCustody.capture(plan.roots)
    digest = custody.content_identity()["digest"]
    header.write_bytes(b"#define VALUE 200\n")
    with pytest.raises(ValueError):
        custody.verify()
    assert (
        CargoResourceCustody.capture(plan.roots).content_identity()["digest"] != digest
    )


def test_c_search_order_and_windows_case_are_preserved(tmp_path: Path) -> None:
    one, two = tmp_path / "one", tmp_path / "two"
    one.mkdir()
    two.mkdir()
    (one / "selected.h").write_bytes(b"one")
    (two / "selected.h").write_bytes(b"two")
    env = _CargoEnvironment({"Include": f"{one};;{two};"}, case_insensitive=True)
    plan = _resolve_c_build_resources(env, target=TARGET)
    assert plan.environment["INCLUDE"] == (
        "${c/INCLUDE/search/0}",
        "",
        "${c/INCLUDE/search/2}",
        "",
    )
    assert tuple(root.path for root in plan.roots) == (one, two)
    reversed_plan = _resolve_c_build_resources(
        _CargoEnvironment({"INCLUDE": f"{two};;{one};"}, case_insensitive=True),
        target=TARGET,
    )
    assert (
        CargoResourceCustody.capture(plan.roots).content_identity()
        != CargoResourceCustody.capture(reversed_plan.roots).content_identity()
    )


@pytest.mark.parametrize("value", ["", "relative", ":/absolute"])
def test_c_search_rejects_unbound_build_script_cwd(value: str) -> None:
    with pytest.raises(ValueError, match="unbound build-script cwd"):
        _resolve_c_build_resources(
            _CargoEnvironment({"CPATH": value}, case_insensitive=False), target=TARGET
        )


@pytest.mark.parametrize(
    "name",
    [
        "MACOSX_DEPLOYMENT_TARGET",
        "IPHONEOS_DEPLOYMENT_TARGET",
        "WATCHOS_DEPLOYMENT_TARGET",
        "TVOS_DEPLOYMENT_TARGET",
        "XROS_DEPLOYMENT_TARGET",
    ],
)
def test_c_deployment_controls_preserve_output_selection(name: str) -> None:
    before = _resolve_c_build_resources(
        _CargoEnvironment({name: "12.0"}), target=TARGET
    )
    after = _resolve_c_build_resources(_CargoEnvironment({name: "15.0"}), target=TARGET)
    assert dict(before.environment) == {name: ("12.0",)}
    assert dict(after.environment) == {name: ("15.0",)}
    assert not before.roots and not after.roots


@pytest.mark.parametrize(
    "option,kind",
    [
        ("-I", "directory"),
        ("-isystem", "directory"),
        ("-iquote", "directory"),
        ("-idirafter", "directory"),
        ("-L", "directory"),
        ("-F", "directory"),
        ("--sysroot", "directory"),
        ("-resource-dir", "directory"),
        ("-include", "file"),
        ("-imacros", "file"),
        ("-include-pch", "file"),
        ("-ivfsoverlay", "file"),
        ("-fmodule-map-file", "file"),
        ("-fprofile-use", "file"),
        ("/I", "directory"),
        ("/external:I", "directory"),
        ("/FI", "file"),
        ("/LIBPATH:", "directory"),
    ],
)
def test_c_flag_resource_family_is_captured(
    tmp_path: Path, option: str, kind: str
) -> None:
    path = tmp_path / "selected resource"
    if kind == "directory":
        path.mkdir()
        (path / "input.h").write_bytes(b"header")
    else:
        path.write_bytes(b"input")
    value = shlex.join((option, str(path)))
    env = _CargoEnvironment({"CFLAGS": value, "CC_SHELL_ESCAPED_FLAGS": "1"})
    plan = _resolve_c_build_resources(env, target=TARGET)
    assert plan.environment["CFLAGS"] == (option, "${c/CFLAGS/flags/0}")
    assert plan.roots[0].path == path
    assert (
        CargoResourceCustody.capture(plan.roots).content_identity()["file_count"] == 1
    )
    assert env["CFLAGS"] == value


@pytest.mark.parametrize("option", ["-I", "-isystem", "/I", "/external:I", "/LIBPATH:"])
def test_c_joined_search_paths_are_captured(tmp_path: Path, option: str) -> None:
    plan = _resolve_c_build_resources(
        _CargoEnvironment(
            {
                "CFLAGS": shlex.quote(option + str(tmp_path)),
                "CC_SHELL_ESCAPED_FLAGS": "1",
            }
        ),
        target=TARGET,
    )
    assert plan.roots[0].path == tmp_path
    assert plan.environment["CFLAGS"] == (option, "${c/CFLAGS/flags/0}")


def test_c_linker_forwarding_captures_script_and_search_inputs(tmp_path: Path) -> None:
    script = tmp_path / "exports.map"
    script.write_bytes(b"{ global: entry; local: *; };")
    plan = _resolve_c_build_resources(
        _CargoEnvironment(
            {
                "LDFLAGS": shlex.quote(
                    f"-Wl,-Bsymbolic,-L,{tmp_path},--version-script={script}"
                ),
                "CC_SHELL_ESCAPED_FLAGS": "1",
            }
        ),
        target=TARGET,
    )
    assert tuple(root.path for root in plan.roots) == (tmp_path, script)
    assert "-Bsymbolic" in plan.environment["LDFLAGS"][0]


@pytest.mark.parametrize("control", ["", "0", "false", "no"])
def test_c_flags_default_to_cc_rs_ascii_whitespace(control: str) -> None:
    plan = _resolve_c_build_resources(
        _CargoEnvironment(
            {"CFLAGS": "-O2\t-DTEXT=a\u00a0b", "CC_SHELL_ESCAPED_FLAGS": control}
        ),
        target=TARGET,
    )
    assert plan.environment["CFLAGS"] == ("-O2", "-DTEXT=a\u00a0b")


def test_c_optional_search_directory_creation_is_fenced(tmp_path: Path) -> None:
    directory = tmp_path / "not-yet-present"
    plan = _resolve_c_build_resources(
        _CargoEnvironment({"CPATH": str(directory)}), target=TARGET
    )
    custody = CargoResourceCustody.capture(plan.roots)
    directory.mkdir()
    (directory / "injected.h").write_bytes(b"changed search selection")
    with pytest.raises(ValueError, match="resource selection changed"):
        custody.verify()


def test_resolved_cargo_plan_consumes_c_resource_custody(
    runtime_fixture_root: RuntimeFixtureRoot, tmp_path: Path
) -> None:
    include = tmp_path / "include"
    include.mkdir()
    header = include / "selected.h"
    header.write_bytes(b"original")
    plan = runtime_cargo_plan(
        tmp_path,
        fixture_root=runtime_fixture_root,
        env={"CPATH": str(include)},
        cargo_command=("cargo", "rustc"),
    )
    plan.verify()
    assert plan.c_environment["CPATH"] == ("${c/CPATH/search/0}",)
    header.write_bytes(b"mutated resource")
    with pytest.raises(ValueError):
        plan.verify()


@pytest.fixture
def external_header_cargo_project(tmp_path: Path):
    """One real registry-free graph whose HOST build input lives outside Cargo."""
    if shutil.which("cargo") is None or shutil.which("rustc") is None:
        pytest.skip("real Cargo/Rust compiler capability is unavailable")
    project = tmp_path / "crate"
    (project / "src").mkdir(parents=True)
    host = project / "host-input"
    (host / "src").mkdir(parents=True)
    include = tmp_path / "external-headers"
    include.mkdir()
    header = include / "value.h"
    header.write_text("#define VALUE 7\n", encoding="utf-8")
    runs = tmp_path / "build-script-runs.txt"
    (project / "Cargo.toml").write_text(
        '[package]\nname = "molt-resource-reuse"\nversion = "0.0.0"\n'
        'edition = "2024"\n[build-dependencies]\n'
        'molt-resource-host = { path = "host-input" }\n[workspace]\n',
        encoding="utf-8",
    )
    (host / "Cargo.toml").write_text(
        '[package]\nname = "molt-resource-host"\nversion = "0.0.0"\nedition = "2024"\n',
        encoding="utf-8",
    )
    (project / "Cargo.lock").write_text(
        'version = 4\n[[package]]\nname = "molt-resource-reuse"\n'
        'version = "0.0.0"\ndependencies = ["molt-resource-host"]\n'
        '[[package]]\nname = "molt-resource-host"\nversion = "0.0.0"\n',
        encoding="utf-8",
    )
    (host / "build.rs").write_text(
        r"""use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CPATH");
    let include = env::split_paths(&env::var_os("CPATH").unwrap()).next().unwrap();
    let header = fs::read_to_string(include.join("value.h")).unwrap();
    let value: u32 = header.split_whitespace().nth(2).unwrap().parse().unwrap();
    let flags = env::var("CFLAGS").unwrap_or_default();
    let deployment = env::var("MACOSX_DEPLOYMENT_TARGET").unwrap_or_default();
    fs::write(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("value.rs"),
        format!("pub const VALUE: u32 = {};\npub const CFLAGS: &str = {:?};\npub const DEPLOYMENT: &str = {:?};\n",
            value, flags, deployment)).unwrap();
    let mut runs = fs::OpenOptions::new().create(true).append(true)
        .open(env::var_os("PROBE_RUN_LOG").unwrap()).unwrap();
    writeln!(runs, "host={}", value).unwrap();
}
""",
        encoding="utf-8",
    )
    (host / "src/lib.rs").write_text(
        'include!(concat!(env!("OUT_DIR"), "/value.rs"));\n', encoding="utf-8"
    )
    (project / "build.rs").write_text(
        r"""use std::{env, fs, io::Write, path::PathBuf};
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let value = molt_resource_host::VALUE;
    fs::write(PathBuf::from(env::var_os("OUT_DIR").unwrap()).join("value.rs"),
        format!("const VALUE: u32 = {};\nconst CFLAGS: &str = {:?};\nconst DEPLOYMENT: &str = {:?};\n",
            value, molt_resource_host::CFLAGS, molt_resource_host::DEPLOYMENT)).unwrap();
    let mut runs = fs::OpenOptions::new().create(true).append(true)
        .open(env::var_os("PROBE_RUN_LOG").unwrap()).unwrap();
    writeln!(runs, "root={}", value).unwrap();
}
""",
        encoding="utf-8",
    )
    (project / "src/main.rs").write_text(
        'include!(concat!(env!("OUT_DIR"), "/value.rs"));\n'
        'fn main() { println!("{}{}{}", VALUE, CFLAGS, DEPLOYMENT); }\n',
        encoding="utf-8",
    )
    environment = dict(os.environ)
    environment.pop("MACOSX_DEPLOYMENT_TARGET", None)
    environment.update(
        CPATH=str(include),
        CFLAGS="",
        CARGO_HOME=str(tmp_path / "cargo-home"),
        CARGO_TARGET_DIR=str(tmp_path / "target"),
        CARGO_INCREMENTAL="0",
        CARGO_BUILD_JOBS="1",
        # Two registry-free fixture crates keep real capacity admission with
        # their own headroom, independently of full-runtime build policy.
        MOLT_DISK_GUARD_HIGH_WATER_GB="1",
        RUSTC_WRAPPER="",
        RUSTC_WORKSPACE_WRAPPER="",
        PROBE_RUN_LOG=str(runs),
    )
    Path(environment["CARGO_HOME"]).mkdir()
    return project, header, runs, environment


@pytest.mark.parametrize(
    "explicit_target", [False, True], ids=["native", "explicit-host"]
)
def test_real_cargo_reuses_only_matching_external_resource_generation(
    external_header_cargo_project, explicit_target: bool
) -> None:
    project, header, runs, environment = external_header_cargo_project
    command = (
        "cargo",
        "build",
        "--offline",
        "--message-format=json",
        "--config",
        "build.target-dir=" + json.dumps(str(project / "escaped-target")),
    )
    first = resolve_runtime_cargo_plan(
        project, env=environment, cargo_command=command, requested_target=None
    )
    requested_target = first.host_target if explicit_target else None
    if explicit_target:
        command = (*command, "--target", first.host_target)

    def capture(env=environment):
        return resolve_runtime_cargo_plan(
            project,
            env=env,
            cargo_command=command,
            requested_target=requested_target,
        )

    # This is cold fixture qualification, including the host's first-image
    # assessment, rather than a warm program latency assertion.
    cold_timeout = 90

    def execute(plan, expected: str, *, fresh: bool) -> Path:
        result = _run_resolved_cargo_plan(
            plan,
            timeout=cold_timeout,
            json_output=True,
            label="external-resource-reuse-test",
        )
        assert result.returncode == 0, result.stderr
        artifacts = [
            json.loads(line)
            for line in result.stdout.splitlines()
            if line.startswith("{")
        ]
        artifact = next(
            row
            for row in artifacts
            if row.get("reason") == "compiler-artifact"
            and row.get("target", {}).get("name") == "molt-resource-reuse"
            and row.get("executable")
        )
        assert artifact["fresh"] is fresh
        binary = Path(artifact["executable"])
        assert binary.is_relative_to(Path(environment["CARGO_TARGET_DIR"]))
        output = run_rust_test_process(
            [str(binary)],
            cwd=str(project),
            env=plan.environment,
            timeout=cold_timeout,
        )
        assert output.returncode == 0, output.stderr
        assert output.stdout.strip() == expected
        assert plan.environment["CARGO_TARGET_DIR"] == environment["CARGO_TARGET_DIR"]
        return binary

    if explicit_target:
        first = capture()
    generation = first.environment["CARGO_BUILD_BUILD_DIR"]
    first_binary = execute(first, "7", fresh=False)
    assert runs.read_text(encoding="utf-8").splitlines() == ["host=7", "root=7"]
    assert not (project / "escaped-target").exists()
    unchanged = capture(first.environment)
    assert unchanged.environment["CARGO_BUILD_BUILD_DIR"] == generation
    execute(unchanged, "7", fresh=True)
    assert runs.read_text(encoding="utf-8").splitlines() == ["host=7", "root=7"]

    header.write_text("#define VALUE 9\n", encoding="utf-8")
    with pytest.raises(ValueError):
        first.verify()
    changed = capture()
    assert changed.environment["CARGO_BUILD_BUILD_DIR"] != generation
    assert execute(changed, "9", fresh=False) == first_binary
    expected_runs = ["host=7", "root=7", "host=9", "root=9"]
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs
    generation = changed.environment["CARGO_BUILD_BUILD_DIR"]
    unchanged = capture()
    assert unchanged.environment["CARGO_BUILD_BUILD_DIR"] == generation
    execute(unchanged, "9", fresh=True)
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs

    # Native build scripts may suppress cc-rs metadata. This HOST dependency
    # deliberately does not declare CFLAGS to Cargo: the admitted plan owns it.
    environment["CFLAGS"] = "-DMOLT_RESOURCE_MARKER=changed"
    flags_changed = capture()
    assert flags_changed.environment["CARGO_BUILD_BUILD_DIR"] != generation
    assert (
        execute(flags_changed, "9-DMOLT_RESOURCE_MARKER=changed", fresh=False)
        == first_binary
    )
    expected_runs += ["host=9", "root=9"]
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs
    generation = flags_changed.environment["CARGO_BUILD_BUILD_DIR"]
    unchanged = capture(flags_changed.environment)
    assert unchanged.environment["CARGO_BUILD_BUILD_DIR"] == generation
    execute(unchanged, "9-DMOLT_RESOURCE_MARKER=changed", fresh=True)
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs

    # cc-rs consumes deployment controls even with cargo_metadata(false).
    # This input changes no file or search root and must still rebuild HOST.
    environment["MACOSX_DEPLOYMENT_TARGET"] = "15.0"
    deployment_changed = capture()
    assert deployment_changed.environment["CARGO_BUILD_BUILD_DIR"] != generation
    assert (
        execute(deployment_changed, "9-DMOLT_RESOURCE_MARKER=changed15.0", fresh=False)
        == first_binary
    )
    expected_runs += ["host=9", "root=9"]
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs
    generation = deployment_changed.environment["CARGO_BUILD_BUILD_DIR"]
    unchanged = capture(deployment_changed.environment)
    assert unchanged.environment["CARGO_BUILD_BUILD_DIR"] == generation
    execute(unchanged, "9-DMOLT_RESOURCE_MARKER=changed15.0", fresh=True)
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs

    source = project / "src/main.rs"
    source.write_text(
        'include!(concat!(env!("OUT_DIR"), "/value.rs"));\n'
        'fn main() { println!("{}{}{}", VALUE + 1, CFLAGS, DEPLOYMENT); }\n',
        encoding="utf-8",
    )
    source_edit = capture()
    assert source_edit.environment["CARGO_BUILD_BUILD_DIR"] == generation
    assert (
        execute(source_edit, "10-DMOLT_RESOURCE_MARKER=changed15.0", fresh=False)
        == first_binary
    )
    assert runs.read_text(encoding="utf-8").splitlines() == expected_runs


@pytest.mark.parametrize(
    "flags", ["-Irelative", "-include", "@compiler.rsp", "-Xclang -load plugin"]
)
def test_unbound_c_resource_grammar_fails_closed(flags: str) -> None:
    with pytest.raises(ValueError):
        _resolve_c_build_resources(_CargoEnvironment({"CFLAGS": flags}), target=TARGET)


@pytest.mark.parametrize("role", ["cc", "cxx", "ar", "ranlib"])
def test_explicit_host_c_tool_is_selected_and_fenced(
    runtime_fixture_root: RuntimeFixtureRoot, tmp_path: Path, role: str
) -> None:
    host_tool = runtime_fixture_root.native_executable(role + ".exe")
    plan = runtime_cargo_plan(
        tmp_path,
        fixture_root=runtime_fixture_root,
        env={"HOST_" + role.upper(): str(host_tool)},
        cargo_command=("cargo", "rustc", "--target", TARGET),
        requested_target=TARGET,
    )
    assert plan.tools["host_" + role] == host_tool
    selector = f"{role.upper()}_{plan.host_target}"
    assert plan.environment[selector] == str(host_tool)
    plan.verify()
    host_tool.write_bytes(b"MZ-host-tool-generation-changed")
    with pytest.raises(ValueError):
        plan.verify()


def test_cc_rs_native_tool_precedence_uses_host_not_target() -> None:
    assert _c_tool_environment_names(
        "cc", target="x86_64-unknown-linux-gnu", host_target="x86_64-unknown-linux-gnu"
    ) == ("CC_x86_64-unknown-linux-gnu", "CC_x86_64_unknown_linux_gnu", "HOST_CC", "CC")
    assert _c_tool_environment_names(
        "cc", target=TARGET, host_target="x86_64-unknown-linux-gnu"
    ) == ("CC_wasm32-wasip1", "CC_wasm32_wasip1", "TARGET_CC", "CC")


def test_host_triple_selector_wins_over_host_generic_tool(
    runtime_fixture_root: RuntimeFixtureRoot, tmp_path: Path
) -> None:
    first = runtime_fixture_root.native_executable("specific.exe")
    second = runtime_fixture_root.native_executable("generic.exe")
    plan = runtime_cargo_plan(
        tmp_path,
        fixture_root=runtime_fixture_root,
        env={"CC_x86_64-unknown-linux-gnu": str(first), "HOST_CC": str(second)},
        cargo_command=("cargo", "rustc", "--target", TARGET),
        requested_target=TARGET,
    )
    assert plan.tools["host_cc"] == first
    assert not any(item.entrypoint == second for item in plan.executable_custody)
    plan.verify()


def test_host_triple_c_flag_inputs_are_captured(tmp_path: Path) -> None:
    header = tmp_path / "host-header.h"
    header.write_bytes(b"host-specific header")
    env = _CargoEnvironment(
        {
            "CFLAGS_x86_64-unknown-linux-gnu": shlex.join(("-include", str(header))),
            "CC_SHELL_ESCAPED_FLAGS": "1",
        }
    )
    plan = _resolve_c_build_resources(
        env, target=TARGET, host_target="x86_64-unknown-linux-gnu"
    )
    key = env.canonical_key("CFLAGS_x86_64-unknown-linux-gnu")
    assert key in plan.environment
    assert plan.roots[0].path == header


def test_msvc_linker_flags_do_not_collide_with_include_prefix(tmp_path: Path) -> None:
    flags = shlex.join(
        (
            "/INCREMENTAL",
            "/IGNORE:4099",
            f"/OUT:{tmp_path / 'output.dll'}",
            f"/LIBPATH:{tmp_path}",
        )
    )
    plan = _resolve_c_build_resources(
        _CargoEnvironment({"LDFLAGS": flags, "CC_SHELL_ESCAPED_FLAGS": "1"}),
        target="x86_64-pc-windows-msvc",
    )
    assert len(plan.roots) == 1
    assert plan.roots[0].path == tmp_path
    assert plan.environment["LDFLAGS"][:2] == ("/INCREMENTAL", "/IGNORE:4099")


def test_c_output_and_runtime_search_arguments_are_not_build_inputs(
    tmp_path: Path,
) -> None:
    flags = shlex.join(("-o", str(tmp_path / "output.o"), "-Wl,-rpath,/runtime/lib"))
    plan = _resolve_c_build_resources(
        _CargoEnvironment({"CFLAGS": flags, "CC_SHELL_ESCAPED_FLAGS": "1"}),
        target=TARGET,
    )
    assert not plan.roots


@pytest.mark.parametrize(
    "option", ["-I", "-L", "-isystem", "-iquote", "-idirafter", "-iframework"]
)
@pytest.mark.parametrize("marker", ["=", "$SYSROOT", "${SYSROOT}"])
@pytest.mark.parametrize("joined", [False, True])
def test_c_sysroot_relative_search_never_captures_host_suffix(
    tmp_path: Path, option: str, marker: str, joined: bool
) -> None:
    operand = marker + str(tmp_path)
    tokens = (option + operand,) if joined else (option, operand)
    env = _CargoEnvironment(
        {
            "CFLAGS": shlex.join(tokens),
            "CC_SHELL_ESCAPED_FLAGS": "1",
            "SDKROOT": str(tmp_path),
        }
    )
    with pytest.raises(ValueError, match="unbound sysroot-relative operand"):
        _resolve_c_build_resources(env, target=TARGET)


def test_forwarded_linker_sysroot_relative_search_fails_closed(tmp_path: Path) -> None:
    env = _CargoEnvironment(
        {
            "LDFLAGS": shlex.quote(f"-Wl,-L={tmp_path}"),
            "CC_SHELL_ESCAPED_FLAGS": "1",
        }
    )
    with pytest.raises(ValueError, match="unbound sysroot-relative operand"):
        _resolve_c_build_resources(env, target=TARGET)


@pytest.mark.parametrize("option", ["--sysroot", "-resource-dir", "--gcc-toolchain"])
def test_c_directory_option_equals_separator_is_not_a_sysroot_marker(
    tmp_path: Path, option: str
) -> None:
    env = _CargoEnvironment(
        {
            "CFLAGS": shlex.quote(f"{option}={tmp_path}"),
            "CC_SHELL_ESCAPED_FLAGS": "1",
        }
    )
    plan = _resolve_c_build_resources(env, target=TARGET)
    assert plan.roots[0].path == tmp_path
    assert plan.environment["CFLAGS"] == (option, "${c/CFLAGS/flags/0}")


@pytest.mark.parametrize("option", ["-fmodule-map-file", "-specs", "--version-script"])
def test_c_file_option_equals_separator_is_not_a_sysroot_marker(
    tmp_path: Path, option: str
) -> None:
    resource = tmp_path / "selected.input"
    resource.write_bytes(b"content")
    env = _CargoEnvironment(
        {
            "CFLAGS": shlex.quote(f"{option}={resource}"),
            "CC_SHELL_ESCAPED_FLAGS": "1",
        }
    )
    plan = _resolve_c_build_resources(env, target=TARGET)
    assert plan.roots[0].path == resource


@pytest.mark.parametrize(
    "option",
    ["-iframeworkwithsysroot", "-iprefix", "-iwithprefix", "-iwithprefixbefore"],
)
@pytest.mark.parametrize("joined", [False, True])
def test_c_implicit_prefix_options_require_their_own_selection_authority(
    tmp_path: Path, option: str, joined: bool
) -> None:
    tokens = (option + str(tmp_path),) if joined else (option, str(tmp_path))
    env = _CargoEnvironment(
        {"CFLAGS": shlex.join(tokens), "CC_SHELL_ESCAPED_FLAGS": "1"}
    )
    with pytest.raises(
        ValueError, match="unbound sysroot/include-prefix resource selection"
    ):
        _resolve_c_build_resources(env, target=TARGET)


def test_a_process_reuses_a_current_capture_and_recaptures_a_changed_one(
    tmp_path: Path,
) -> None:
    root = tmp_path / "sdk"
    root.mkdir()
    header = root / "api.h"
    header.write_bytes(b"first generation")
    roots = (CargoResourceRoot("c/SDKROOT", root),)

    first = CargoResourceCustody.capture(roots)
    assert CargoResourceCustody.capture(roots) is first

    header.write_bytes(b"second generation, longer")
    second = CargoResourceCustody.capture(roots)
    assert second is not first
    assert (
        second.file_identity(header).sha256
        == hashlib.sha256(b"second generation, longer").hexdigest()
    )


@pytest.mark.skipif(os.name == "nt", reason="Windows always walks exactly")
def test_settled_directory_tokens_detect_a_new_resource(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import cargo_resource_custody

    # Close the racy window so the directory-token proof itself is exercised.
    monkeypatch.setattr(cargo_resource_custody, "DIGEST_RACY_WINDOW_NS", 0)
    root = tmp_path / "sdk"
    (root / "include").mkdir(parents=True)
    (root / "include" / "api.h").write_bytes(b"header")
    time.sleep(0.01)
    custody = CargoResourceCustody.capture((CargoResourceRoot("c/SDKROOT", root),))
    assert not custody.walks[0].racy
    custody.verify()

    (root / "include" / "new.h").write_bytes(b"new resource")
    with pytest.raises(ValueError, match="resource selection changed"):
        custody.verify()


def test_a_freshly_changed_tree_is_always_walked_exactly(tmp_path: Path) -> None:
    root = tmp_path / "sdk"
    root.mkdir()
    (root / "api.h").write_bytes(b"header")
    walk = CargoResourceRoot("c/SDKROOT", root).walk()
    assert walk.racy


@pytest.mark.skipif(os.name == "nt", reason="Windows always walks exactly")
def test_root_sets_that_share_a_root_walk_it_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import cargo_resource_custody

    monkeypatch.setattr(cargo_resource_custody, "DIGEST_RACY_WINDOW_NS", 0)
    monkeypatch.setattr(cargo_resource_custody, "_WALKED_ROOTS", {})
    shared, other = tmp_path / "sdk", tmp_path / "libdir"
    for directory in (shared, other):
        directory.mkdir()
        (directory / "a.h").write_bytes(directory.name.encode())
    time.sleep(0.01)
    sdk = CargoResourceRoot("c/SDKROOT", shared)
    libdir = CargoResourceRoot("rust/lib", other)
    alone = CargoResourceCustody.capture((sdk,))
    pair = CargoResourceCustody.capture((libdir, sdk))
    assert pair.walks[1] is alone.walks[0], "the second root set reuses the SDK walk"
    assert (
        pair.file_identity(shared / "a.h").sha256 == hashlib.sha256(b"sdk").hexdigest()
    )

    (shared / "b.h").write_bytes(b"new")
    widened = CargoResourceCustody.capture((sdk, libdir))
    assert widened.walks[0] is not alone.walks[0], "a new entry changes the token"
    assert widened.walks[1] is pair.walks[0], "the unchanged root keeps its walk"
    assert {item.entrypoint.name for item in widened.files} == {"a.h", "b.h"}


@pytest.mark.skipif(os.name == "nt", reason="Windows always walks exactly")
def test_a_recording_receives_each_shared_walk_once_with_captured_tokens(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt import input_report
    from molt.cli import cargo_resource_custody

    monkeypatch.setattr(cargo_resource_custody, "DIGEST_RACY_WINDOW_NS", 0)
    monkeypatch.setattr(cargo_resource_custody, "_WALKED_ROOTS", {})
    monkeypatch.setattr(cargo_resource_custody, "_CAPTURED_RESOURCES", {})
    root, other = tmp_path / "sdk", tmp_path / "include"
    root.mkdir()
    other.mkdir()
    header = root / "api.h"
    header.write_bytes(b"header")
    (other / "extra.h").write_bytes(b"extra")
    time.sleep(0.01)
    sdk = CargoResourceRoot("c/SDKROOT", root)

    class Sink:
        def __init__(self) -> None:
            self.files: list[tuple[str, object, object]] = []

        def file(self, path, token, sha256) -> None:
            self.files.append((os.fspath(path), token, sha256))

        def directory(self, path, token) -> None:
            pass

        def probe(self, path, outcome) -> None:
            pass

        def refuse(self, reason) -> None:
            pass

        def observe(self, entry) -> None:
            pass

    def record(sink: Sink) -> None:
        input_report.install_sink(sink, os.fspath(tmp_path / "workers"))
        try:
            CargoResourceCustody.capture((sdk,))
            CargoResourceCustody.capture((sdk, CargoResourceRoot("c/CPATH", other)))
        finally:
            input_report.remove_sink()

    first, second = Sink(), Sink()
    record(first)
    digest = hashlib.sha256(b"header").hexdigest()
    token = input_report.stat_token(header)
    assert first.files.count((os.fspath(header), token, digest)) == 1, first.files
    record(second)
    assert second.files.count((os.fspath(header), token, digest)) == 1, (
        "a new recording receives the reused capture again"
    )


@pytest.mark.skipif(os.name == "nt", reason="Windows always walks exactly")
def test_an_apple_sdk_root_is_identified_by_its_settings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import cargo_resource_custody

    monkeypatch.setattr(cargo_resource_custody, "DIGEST_RACY_WINDOW_NS", 0)
    sdk = tmp_path / "MacOSX.sdk"
    (sdk / "usr/include").mkdir(parents=True)
    settings = sdk / "SDKSettings.json"
    settings.write_bytes(b'{"Version": "26.5"}')
    (sdk / "usr/include/stdio.h").write_bytes(b"int printf(const char *, ...);\n")
    time.sleep(0.01)
    plan = _resolve_c_build_resources(
        _CargoEnvironment({"SDKROOT": str(sdk)}), target=TARGET
    )
    custody = CargoResourceCustody.capture(plan.roots)
    assert [item.entrypoint for item in custody.files] == [settings]
    digest = custody.content_identity()["digest"]

    # Each SDK version is installed whole: its settings name its headers and
    # entries, so neither is an input of its own.
    (sdk / "usr/include/stdio.h").write_bytes(b"int printf(const char *);\n")
    (sdk / "SDKSettings.plist").write_bytes(b"<plist/>")
    custody.verify()

    settings.write_bytes(b'{"Version": "26.6"}')
    with pytest.raises(ValueError, match="changed since identity capture"):
        custody.verify()
    assert (
        CargoResourceCustody.capture(plan.roots).content_identity()["digest"] != digest
    )
