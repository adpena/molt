"""Platform selection precedes tool byte admission; no compiler invocation."""

from __future__ import annotations

import hashlib
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from molt import platform_toolchain as platform_tools

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
            "component", {"ProgramFiles(x86)": str(tmp_path)}
        )
        == install
    )
    assert calls[0][0][0] == str(vswhere)
    assert calls[0][1]["timeout"] == 30
    assert calls[0][1]["memory_guard_prefix"] == "MOLT_PLATFORM_TOOLCHAIN"


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
    monkeypatch.setattr(platform_tools.platform, "system", lambda: "Windows")
    monkeypatch.setattr(
        platform_tools, "visual_studio_installation", lambda *_: install
    )
    monkeypatch.setattr(
        platform_tools,
        "resolve_executable",
        lambda *_args, **_kwargs: tmp_path / "cmd.exe",
    )
    monkeypatch.setattr(
        platform_tools.shutil, "which", lambda name, **_kwargs: "cl.exe"
    )

    def query(argv, env, **kwargs):
        calls.append((argv, env.copy(), kwargs))
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
        platform_tools.shutil, "which", lambda *_args, **_kwargs: "cl.exe"
    )
    env = platform_tools.activate_msvc_environment(
        os.environ, repo_root=ROOT, machine="AMD64"
    )
    assert Path(env["VSINSTALLDIR"]) == install


def test_darwin_resolves_real_tools_under_pinned_developer_and_sdk(
    tmp_path, monkeypatch
):
    developer = tmp_path / "Xcode/Contents/Developer"
    sdk = developer / "SDKs/MacOSX.sdk"
    sdk.mkdir(parents=True)
    settings = b'{"Version":"15.4"}'
    (sdk / "SDKSettings.json").write_bytes(settings)
    actual = developer / "Toolchains/XcodeDefault.xctoolchain/usr/bin"
    actual.mkdir(parents=True)
    for name in ("clang", "clang++", "ar", "ranlib", "ld"):
        (actual / name).write_bytes(name.encode())
    calls = []

    def resolve(name, **_kwargs):
        return Path(name) if Path(name).is_absolute() else tmp_path / name

    monkeypatch.setattr(platform_tools, "resolve_executable", resolve)

    def query(argv, env, **_kwargs):
        calls.append((argv, env.copy()))
        if "--print-path" in argv:
            return str(developer)
        assert env["DEVELOPER_DIR"] == str(developer)
        if "--show-sdk-path" in argv:
            return str(sdk)
        if "--show-sdk-version" in argv:
            return "15.4"
        assert env["SDKROOT"] == str(sdk)
        return str(actual / argv[-1])

    monkeypatch.setattr(platform_tools, "_query", query)
    selection = platform_tools.select_darwin_toolchain({"PATH": "/usr/bin"})
    assert set(selection.tools.values()) == set(actual.iterdir())
    assert selection.versions() == {
        "sdk": "15.4",
        "deployment_target": "15.4",
        "sdk_settings_sha256": hashlib.sha256(settings).hexdigest(),
    }
    assert selection.environment()["MACOSX_DEPLOYMENT_TARGET"] == "15.4"
    before = len([argv for argv, _ in calls if "--print-path" in argv])
    assert selection == platform_tools.select_darwin_toolchain(
        selection.environment(), developer_dir=developer
    )
    assert len([argv for argv, _ in calls if "--print-path" in argv]) == before
    (sdk / "SDKSettings.json").write_bytes(b"changed SDK")
    assert selection != platform_tools.select_darwin_toolchain(
        selection.environment(), developer_dir=developer
    )
