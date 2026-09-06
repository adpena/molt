from __future__ import annotations

import shlex
from pathlib import Path

import pytest

from molt.cli.runtime_cargo_plan import (
    CargoResourceCustody,
    _CargoEnvironment,
    _resolve_c_build_resources,
    _c_tool_environment_names,
    runtime_c_flag_environment_names,
)
from tests.runtime_build_identity_helper import runtime_cargo_plan


TARGET = "wasm32-wasip1"


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


def test_resolved_cargo_plan_consumes_c_resource_custody(tmp_path: Path) -> None:
    include = tmp_path / "include"
    include.mkdir()
    header = include / "selected.h"
    header.write_bytes(b"original")
    plan = runtime_cargo_plan(
        tmp_path, env={"CPATH": str(include)}, cargo_command=("cargo", "rustc")
    )
    plan.verify()
    assert plan.c_environment["CPATH"] == ("${c/CPATH/search/0}",)
    header.write_bytes(b"mutated resource")
    with pytest.raises(ValueError):
        plan.verify()


@pytest.mark.parametrize(
    "flags", ["-Irelative", "-include", "@compiler.rsp", "-Xclang -load plugin"]
)
def test_unbound_c_resource_grammar_fails_closed(flags: str) -> None:
    with pytest.raises(ValueError):
        _resolve_c_build_resources(_CargoEnvironment({"CFLAGS": flags}), target=TARGET)


@pytest.mark.parametrize("role", ["cc", "cxx", "ar", "ranlib"])
def test_explicit_host_c_tool_is_selected_and_fenced(tmp_path: Path, role: str) -> None:
    host_tool = tmp_path / (role + ".exe")
    host_tool.write_bytes(b"MZ-selected-host-tool")
    plan = runtime_cargo_plan(
        tmp_path,
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


def test_host_triple_selector_wins_over_host_generic_tool(tmp_path: Path) -> None:
    first, second = tmp_path / "specific.exe", tmp_path / "generic.exe"
    first.write_bytes(b"MZ-specific-host")
    second.write_bytes(b"MZ-generic-host")
    plan = runtime_cargo_plan(
        tmp_path,
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
