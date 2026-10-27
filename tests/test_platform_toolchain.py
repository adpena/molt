"""Platform selection precedes tool byte admission; no compiler invocation."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
from types import SimpleNamespace
import platform

import pytest

from molt import platform_toolchain as platform_tools
from molt.process_environment import NameRule, ProcessEnvironment
from tests.executable_test_support import write_mock_executable
from tests.process_guard_common import install_module_view
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def test_visual_studio_selection_uses_bounded_guarded_probe(tmp_path, monkeypatch):
    vswhere = tmp_path / "Microsoft Visual Studio/Installer/vswhere.exe"
    vswhere.parent.mkdir(parents=True)
    vswhere.write_bytes(b"tool")
    install = tmp_path / "Build Tools"
    install.mkdir()
    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs))
        return SimpleNamespace(stdout=str(install), returncode=0)

    monkeypatch.setattr(platform_tools.process_guard, "run_completed_command", run)
    assert (
        platform_tools.visual_studio_installation(
            "component",
            ProcessEnvironment(
                {"ProgramFiles(x86)": str(tmp_path)}, rule=NameRule.WINDOWS
            ),
        )
        == install
    )
    assert calls[0][0][0] == str(vswhere)
    assert calls[0][1]["timeout"] == 30
    assert calls[0][1]["memory_guard_prefix"] == "MOLT_PLATFORM_TOOLCHAIN"


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_visual_studio_selection_preserves_the_callers_name_rule(
    tmp_path, monkeypatch, rule
):
    install = tmp_path / "installation"
    install.mkdir()
    selected_root = tmp_path / "selected"
    vswhere = selected_root / "Microsoft Visual Studio/Installer/vswhere.exe"
    vswhere.parent.mkdir(parents=True)
    vswhere.write_bytes(b"selected selector")
    values = (
        {"PROGRAMFILES(X86)": str(selected_root), "ProgramFiles(x86)": "unselected"}
        if rule is NameRule.EXACT
        else {"ProgramFiles(x86)": str(selected_root)}
    )
    environment = ProcessEnvironment(values, rule=rule)
    calls = []

    def run(argv, **kwargs):
        calls.append((argv, kwargs["env"]))
        return SimpleNamespace(stdout=str(install), returncode=0)

    monkeypatch.setattr(platform_tools.process_guard, "run_completed_command", run)
    assert (
        platform_tools.visual_studio_installation("component", environment) == install
    )
    assert calls == [
        (
            [
                str(vswhere),
                "-latest",
                "-products",
                "*",
                "-requires",
                "component",
                "-property",
                "installationPath",
            ],
            environment,
        )
    ]
    assert calls[0][1].rule is rule


@pytest.mark.parametrize("rule", [NameRule.EXACT, NameRule.WINDOWS])
def test_msvc_activation_updates_only_equivalent_environment_names(
    tmp_path, monkeypatch, rule
):
    install = tmp_path / "Visual Studio"
    script = install / "Common7/Tools/VsDevCmd.bat"
    script.parent.mkdir(parents=True)
    script.write_text("", encoding="utf-8")
    (install / "VC").mkdir()
    install_module_view(
        monkeypatch, "platform", platform, platform_tools, system=lambda: "Windows"
    )
    monkeypatch.setattr(
        platform_tools, "visual_studio_installation", lambda *_: install
    )
    monkeypatch.setattr(
        platform_tools, "resolve_executable", lambda *_a, **_k: tmp_path / "cmd"
    )
    monkeypatch.setattr(
        platform_tools, "find_executable", lambda *_a, **_k: tmp_path / "cl"
    )
    captured = []

    def query(_argv, env, **_kwargs):
        captured.append(env)
        return (
            f"PATH=activated\nVCINSTALLDIR={install / 'VC'}\nVSINSTALLDIR={install}\n"
            "VSCMD_ARG_TGT_ARCH=x64\nVSCMD_ARG_HOST_ARCH=x64\nLIB=owned\n"
        )

    monkeypatch.setattr(platform_tools, "_query", query)
    environment = ProcessEnvironment(
        {
            "Path": "case-sensitive path",
            "PATH": "canonical path",
            "lib": "lower",
            "LIB": "foreign",
        },
        rule=rule,
    )
    result = platform_tools.activate_msvc_environment(
        environment, repo_root=ROOT, machine="AMD64"
    )
    assert result.rule is captured[0].rule is rule
    assert "LIB" not in captured[0]
    if rule is NameRule.EXACT:
        assert result["Path"] == "case-sensitive path"
        assert result["PATH"] == "activated"
        assert result["lib"] == "lower"
    else:
        assert result["Path"] == result["PATH"] == "activated"
        assert "Path" in tuple(result) and "PATH" not in tuple(result)
    assert result["LIB"] == "owned"
    assert environment["PATH"] == "canonical path"


@pytest.mark.parametrize("machine,arch", [("AMD64", "x64"), ("ARM64", "arm64")])
def test_msvc_activation_owns_installation_and_search_roots_without_atl(
    tmp_path, monkeypatch, machine, arch
):
    install = tmp_path / "Visual Studio Build Tools"
    script = install / "Common7/Tools/VsDevCmd.bat"
    script.parent.mkdir(parents=True)
    script.write_text("", encoding="utf-8")
    (install / "VC").mkdir()
    calls = []
    install_module_view(
        monkeypatch, "platform", platform, platform_tools, system=lambda: "Windows"
    )
    monkeypatch.setattr(
        platform_tools, "visual_studio_installation", lambda *_: install
    )
    monkeypatch.setattr(
        platform_tools,
        "resolve_executable",
        lambda *_args, **_kwargs: tmp_path / "cmd.exe",
    )
    monkeypatch.setattr(
        platform_tools,
        "find_executable",
        lambda name, **_kwargs: Path(name) if name == "cl" else None,
    )

    def query(argv, env, **kwargs):
        calls.append((argv, ProcessEnvironment(env), kwargs))
        return f"PATH=activated\nVCINSTALLDIR={install / 'VC'}\nVSINSTALLDIR={install}\nVSCMD_ARG_TGT_ARCH={arch}\nVSCMD_ARG_HOST_ARCH={arch}\nINCLUDE=owned\nLIB=owned\n"

    monkeypatch.setattr(platform_tools, "_query", query)
    env = platform_tools.activate_msvc_environment(
        {
            "PATH": "base",
            "VCINSTALLDIR": "foreign",
            "VSINSTALLDIR": "foreign",
            "LIB": "injected",
            "INCLUDE": "injected",
        },
        repo_root=ROOT,
        machine=machine,
    )
    assert env["VCINSTALLDIR"] == str(install / "VC")
    assert env["VSINSTALLDIR"] == str(install)
    assert env["LIB"] == env["INCLUDE"] == "owned"
    assert "LIB" not in calls[0][1] and "VCINSTALLDIR" not in calls[0][1]
    assert f"-arch={arch} -host_arch={arch}" in calls[0][0][-1]
    assert "MOLT_VSDEVCMD_CALL" not in env
    assert not list(install.rglob("atlbase.h"))


@pytest.mark.skipif(os.name != "nt", reason="requires Windows batch semantics")
def test_msvc_activation_preserves_batch_path_with_spaces(tmp_path, monkeypatch):
    install = tmp_path / "Visual Studio Build Tools"
    script = install / "Common7/Tools/VsDevCmd.bat"
    script.parent.mkdir(parents=True)
    (install / "VC").mkdir()
    script.write_text(
        f'@echo off\nset "VCINSTALLDIR={install / "VC"}"\nset "VSINSTALLDIR={install}"\nset "VSCMD_ARG_TGT_ARCH=x64"\nset "VSCMD_ARG_HOST_ARCH=x64"\n',
        encoding="utf-8",
    )
    monkeypatch.setattr(
        platform_tools, "visual_studio_installation", lambda *_: install
    )
    monkeypatch.setattr(
        platform_tools,
        "find_executable",
        lambda name, **_kwargs: Path(name) if name == "cl" else None,
    )
    env = platform_tools.activate_msvc_environment(
        os.environ, repo_root=ROOT, machine="AMD64"
    )
    assert Path(env["VSINSTALLDIR"]) == install


@pytest.mark.parametrize("names", [None, ("clang", "ar", "ld")])
def test_darwin_resolves_real_tools_under_pinned_developer_and_sdk(
    tmp_path, monkeypatch, names
):
    developer = tmp_path / "Xcode/Contents/Developer"
    sdk = developer / "SDKs/MacOSX.sdk"
    sdk.mkdir(parents=True)
    settings = b'{"Version":"15.4"}'
    (sdk / "SDKSettings.json").write_bytes(settings)
    actual = developer / "Toolchains/XcodeDefault.xctoolchain/usr/bin"
    actual.mkdir(parents=True)
    selected_names = (
        ("clang", "clang++", "ar", "ranlib", "ld") if names is None else names
    )
    for name in selected_names:
        (actual / name).write_bytes(name.encode())
    calls = []
    # The selectors are real files: the toolchain facts key each answer by
    # the selector's bytes before asking it (HF-357). Only the process is faked.
    selectors = tmp_path / "selectors"
    selectors.mkdir()
    for name in ("xcode-select", "xcrun"):
        write_mock_executable(selectors / name, b"MZ " + name.encode())

    def query(argv, **kwargs):
        env = kwargs["env"]
        calls.append((argv, ProcessEnvironment(env)))
        if "--print-path" in argv:
            stdout = str(developer)
        else:
            assert env["DEVELOPER_DIR"] == str(developer)
            if "--show-sdk-path" in argv:
                stdout = str(sdk)
            else:
                assert "--show-sdk-version" not in argv
                assert argv[argv.index("--sdk") + 1] == str(sdk)
                assert env["SDKROOT"] == str(sdk)
                stdout = str(actual / argv[-1])
        return subprocess.CompletedProcess(argv, 0, stdout + "\n", "")

    monkeypatch.setattr(platform_tools.process_guard, "run_completed_command", query)
    options = {} if names is None else {"names": names}
    selection = platform_tools.select_darwin_toolchain(
        {"PATH": str(selectors)}, **options
    )
    assert set(selection.tools) == set(selected_names)
    assert set(selection.tools.values()) == set(actual.iterdir())
    assert selection.versions() == {
        "sdk": "15.4",
        "deployment_target": "15.4",
        "sdk_settings_sha256": hashlib.sha256(settings).hexdigest(),
    }
    assert selection.environment()["MACOSX_DEPLOYMENT_TARGET"] == "15.4"
    before = len([argv for argv, _ in calls if "--print-path" in argv])
    pinned = {**selection.environment(), "PATH": str(selectors)}
    assert selection == platform_tools.select_darwin_toolchain(pinned, **options)
    assert len([argv for argv, _ in calls if "--print-path" in argv]) == before
    (sdk / "SDKSettings.json").write_bytes(b'{"Version":"15.4","changed":true}')
    assert selection != platform_tools.select_darwin_toolchain(pinned, **options)


@pytest.mark.parametrize("names", [(), ("clang", "clang"), ("foreign",)])
def test_darwin_tool_selection_rejects_invalid_demands_before_discovery(
    monkeypatch, names
):
    def forbidden(*_args, **_kwargs):
        pytest.fail("invalid tool demands must not discover the SDK")

    monkeypatch.setattr(platform_tools, "_darwin_developer_directory", forbidden)
    with pytest.raises(ValueError, match="nonempty unique declared selection"):
        platform_tools.select_darwin_toolchain({}, names=names)


@pytest.mark.parametrize("developer_override", [False, True])
def test_explicit_darwin_sdk_is_authoritative_without_selector_queries(
    tmp_path, monkeypatch, developer_override
):
    sdk = tmp_path / "Selected SDK"
    sdk.mkdir()
    settings = b'{"Version":"14.6"}'
    (sdk / "SDKSettings.json").write_bytes(settings)
    env = {"SDKROOT": str(sdk)}
    if developer_override:
        developer = tmp_path / "Selected Developer"
        developer.mkdir()
        env["DEVELOPER_DIR"] = str(developer)
    monkeypatch.setattr(
        platform_tools,
        "resolve_executable",
        lambda *_args, **_kwargs: pytest.fail("explicit SDK selection queried a tool"),
    )
    selected = platform_tools.select_darwin_sdk(env)
    assert selected.sdk_root == sdk
    assert selected.sdk_version == "14.6"
    assert selected.sdk_settings_sha256 == hashlib.sha256(settings).hexdigest()
    assert selected.environment() == env
    # Re-selection reads current metadata; it never serves a stale global memo.
    (sdk / "SDKSettings.json").write_bytes(b'{"Version":"15.2"}')
    assert platform_tools.select_darwin_sdk(env).sdk_version == "15.2"


@pytest.mark.parametrize(
    "settings",
    [b"not JSON", b"[]", b"{}", b'{"Version":true}', b'{"Version":"1;flags"}'],
)
def test_darwin_sdk_rejects_invalid_version_metadata(tmp_path, settings):
    sdk = tmp_path / "SDK"
    sdk.mkdir()
    (sdk / "SDKSettings.json").write_bytes(settings)
    with pytest.raises(ValueError, match="Darwin SDK"):
        platform_tools.select_darwin_sdk({"SDKROOT": str(sdk)})


@pytest.mark.parametrize(
    "arch,expected",
    [("x86_64", "10.13"), ("amd64", "10.13"), ("arm64", "15.4"), (None, "15.4")],
)
def test_darwin_deployment_policy_retains_native_and_release_floors(arch, expected):
    assert (
        platform_tools.darwin_deployment_target({}, sdk_version="15.4", arch=arch)
        == expected
    )


@pytest.mark.parametrize(
    "env,expected",
    [
        ({"MACOSX_DEPLOYMENT_TARGET": "12.7"}, "12.7"),
        (
            {
                "MOLT_MACOSX_DEPLOYMENT_TARGET": "13.3",
                "MACOSX_DEPLOYMENT_TARGET": "12.7",
            },
            "13.3",
        ),
    ],
)
def test_darwin_deployment_policy_honors_explicit_build_selection(env, expected):
    assert (
        platform_tools.darwin_deployment_target(env, sdk_version="15.4", arch="arm64")
        == expected
    )


def test_darwin_deployment_policy_rejects_malformed_override():
    with pytest.raises(ValueError, match="deployment target"):
        platform_tools.darwin_deployment_target(
            {"MACOSX_DEPLOYMENT_TARGET": "13.0 -extra"}, sdk_version="15.4"
        )


@pytest.mark.parametrize(
    "error",
    [
        FileNotFoundError("missing selector"),
        subprocess.CalledProcessError(2, ["selector"], stderr="selected-tool-failed"),
        subprocess.TimeoutExpired(["selector"], 1),
    ],
)
def test_selector_failures_stay_inside_platform_selection_diagnostics(
    tmp_path, monkeypatch, error
):
    # The selector's bytes are identified before it runs; the failure under
    # test is the process, so the selector itself is a real file.
    selector = write_mock_executable(tmp_path / "selector", b"MZ selector")
    monkeypatch.setattr(
        platform_tools, "resolve_executable", lambda *_args, **_kwargs: selector
    )

    def fail(*_args, **_kwargs):
        raise error

    monkeypatch.setattr(platform_tools.process_guard, "run_completed_command", fail)
    with pytest.raises(
        ValueError, match="Platform toolchain selector.*failed"
    ) as raised:
        platform_tools.select_darwin_sdk({})
    assert raised.value.__cause__ is error


@pytest.mark.parametrize(
    "value,expected",
    [("15", "15.0"), ("15.0.0", "15.0"), ("15.2.0", "15.2"), ("15.2.1", "15.2.1")],
)
def test_darwin_versions_have_one_numeric_identity(value, expected):
    assert platform_tools.normalize_darwin_version(value, label="test") == expected


def test_darwin_sdk_metadata_budget_is_checked_before_json_read(tmp_path):
    sdk = tmp_path / "SDK"
    sdk.mkdir()
    with (sdk / "SDKSettings.json").open("wb") as stream:
        stream.truncate(8 * 1024 * 1024)
    with pytest.raises(ValueError, match="metadata budget"):
        platform_tools.select_darwin_sdk({"SDKROOT": str(sdk)})
