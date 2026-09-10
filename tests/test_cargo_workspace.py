from __future__ import annotations

from pathlib import Path

import pytest

from molt.cargo_workspace import (
    LocalCargoDependency,
    workspace_manifest_facts,
    workspace_member_manifests,
    workspace_package_names,
)


def _manifest(root: Path, directory: str, name: str) -> Path:
    path = root / directory / "Cargo.toml"
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(f'[package]\nname = "{name}"\n', encoding="utf-8")
    return path


def test_membership_uses_root_names_order_globs_and_exclusions(tmp_path: Path) -> None:
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["runtime/z", "runtime/*", "runtime/a"]\n'
        'exclude = ["runtime/isolated"]\n',
        encoding="utf-8",
    )
    z = _manifest(tmp_path, "runtime/z", "package-z")
    a = _manifest(tmp_path, "runtime/a", "package-a")
    _manifest(tmp_path, "runtime/isolated", "not-a-member")
    _manifest(tmp_path, "scratch", "not-discovered")

    assert workspace_member_manifests(tmp_path) == (z, a)
    assert workspace_package_names(tmp_path) == ("package-z", "package-a")


@pytest.mark.parametrize(
    "source,diagnostic",
    [
        ("broken [", "Cargo manifest"),
        ("[package]\nname='root'", "missing \\[workspace\\]"),
        ("[workspace]", "workspace.members"),
        ('[workspace]\nmembers = "runtime/a"', "workspace.members"),
        ("[workspace]\nmembers = [42]", "workspace.members"),
        ('[workspace]\nmembers = [""]', "workspace.members"),
        ('[workspace]\nmembers = ["runtime/a "]', "workspace.members"),
        ('[workspace]\nmembers = ["../a"]', "root-relative"),
        ('[workspace]\nmembers = ["C:/outside"]', "root-relative"),
        ('[workspace]\nmembers = ["/outside"]', "root-relative"),
        ('[workspace]\nmembers = ["missing"]', "manifest is missing"),
        ('[workspace]\nmembers = ["missing/*"]', "no matches"),
        ("[workspace]\nmembers = []\nexclude = [false]", "workspace.exclude"),
    ],
)
def test_bad_authority_is_not_an_empty_or_recursive_suite(
    tmp_path: Path, source: str, diagnostic: str
) -> None:
    (tmp_path / "Cargo.toml").write_text(source, encoding="utf-8")
    _manifest(tmp_path, "scratch", "must-not-be-fallback")
    with pytest.raises(ValueError, match=diagnostic):
        workspace_member_manifests(tmp_path)


def test_missing_workspace_is_an_error(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="Cargo manifest"):
        workspace_member_manifests(tmp_path)


def test_lock_inputs_detect_same_size_rewrite_with_restored_timestamp(
    tmp_path: Path,
) -> None:
    import os

    from molt.cli.lockfiles import _lock_check_inputs

    manifest = tmp_path / "Cargo.toml"
    manifest.write_bytes(b"first")
    before = manifest.stat()
    inputs = _lock_check_inputs(tmp_path, [manifest])
    manifest.write_bytes(b"other")
    os.utime(manifest, ns=(before.st_atime_ns, before.st_mtime_ns))
    assert _lock_check_inputs(tmp_path, [manifest]) != inputs


@pytest.mark.parametrize("source", ["[package]", "[package]\nname=42", "malformed ["])
def test_bad_member_is_not_silently_dropped(tmp_path: Path, source: str) -> None:
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["runtime/a"]\n', encoding="utf-8"
    )
    member = _manifest(tmp_path, "runtime/a", "a")
    member.write_text(source, encoding="utf-8")
    with pytest.raises(ValueError, match="Cargo.toml"):
        workspace_package_names(tmp_path)


def test_duplicate_package_identity_is_rejected(tmp_path: Path) -> None:
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["runtime/a", "runtime/b"]\n', encoding="utf-8"
    )
    _manifest(tmp_path, "runtime/a", "same")
    _manifest(tmp_path, "runtime/b", "same")
    with pytest.raises(ValueError, match="duplicate package.name"):
        workspace_package_names(tmp_path)


def test_consumers_share_exact_root_membership() -> None:
    from tools.canonicalization_contract import workspace_members

    root = Path(__file__).resolve().parents[1]
    manifests = workspace_member_manifests(root)
    assert workspace_members(root) == [manifest.parent for manifest in manifests]
    assert len(workspace_package_names(root)) == len(manifests)
    assert not (root / "runtime" / "Cargo.toml").exists()
    assert not (root / "runtime" / "Cargo.lock").exists()
    members = {manifest.parent.name for manifest in manifests}
    assert {"molt-runtime-core", "molt-cext-discovery", "molt-wasm-facts"} <= members
    assert not members & {
        "molt-backend-mlir",
        "molt-python",
        "molt-cpython-abi-test-support",
    }


def test_lock_validation_rejects_bad_membership_before_cache_or_cargo(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from molt.cli import lockfiles

    (tmp_path / "Cargo.toml").write_text("invalid [", encoding="utf-8")
    monkeypatch.setattr(lockfiles.shutil, "which", lambda _tool: "cargo")
    monkeypatch.setattr(
        lockfiles,
        "_lock_check_inputs",
        lambda *_args: pytest.fail("partial cache inputs"),
    )
    monkeypatch.setattr(
        lockfiles,
        "_run_completed_command",
        lambda *_args, **_kwargs: pytest.fail("Cargo launched"),
    )
    error = lockfiles._verify_cargo_lock(tmp_path)
    assert error is not None and "Cannot validate Cargo.lock workspace inputs" in error


def test_lock_inputs_follow_local_dependency_edges_without_changing_membership(
    tmp_path: Path,
) -> None:
    root_manifest = tmp_path / "Cargo.toml"
    root_manifest.write_text(
        '[workspace]\nmembers = ["runtime/main"]\n'
        'exclude = ["runtime/support"]\n'
        '[workspace.dependencies]\nshared = { path = "local/shared", package = "renamed" }\n'
        '[patch.crates-io]\npatched = { path = "local/patched" }\n'
        '[replace]\n"replaced:0.1.0" = { path = "local/replaced" }\n',
        encoding="utf-8",
    )
    main = _manifest(tmp_path, "runtime/main", "main")
    main.write_text(
        '[package]\nname = "main"\nedition.workspace = true\n'
        '[dependencies]\nsupport = { path = "../support", optional = true }\n'
        'shared.workspace = true\nregistry = "1"\n'
        '[build-dependencies]\nbuild = { path = "../../local/build" }\n'
        '[dev-dependencies]\ndev = { path = "../../local/dev" }\n'
        "[target.'cfg(windows)'.dependencies]\nplatform = { path = \"../../local/platform\" }\n"
        "[target.'cfg(unix)'.build-dependencies]\nplatform_build = { path = \"../../local/platform-build\" }\n"
        "[target.'cfg(unix)'.dev-dependencies]\nplatform_dev = { path = \"../../local/platform-dev\" }\n",
        encoding="utf-8",
    )
    support = _manifest(tmp_path, "runtime/support", "support")
    support.write_text(
        '[package]\nname = "support"\n'
        '[build-dependencies]\ntransitive = { path = "../../local/transitive" }\n',
        encoding="utf-8",
    )
    paths = [
        _manifest(tmp_path, f"local/{name}", name)
        for name in (
            "shared",
            "patched",
            "replaced",
            "build",
            "dev",
            "platform",
            "platform-build",
            "platform-dev",
            "transitive",
        )
    ]
    paths[-1].write_text(
        '[package]\nname = "transitive"\n'
        '[dev-dependencies]\ncycle = { path = "../../runtime/support" }\n',
        encoding="utf-8",
    )
    stray = _manifest(tmp_path, "isolated", "unrelated")
    stray.write_text("broken [", encoding="utf-8")

    facts = workspace_manifest_facts(tmp_path)
    manifests = facts.input_manifests
    assert workspace_member_manifests(tmp_path) == (main,)
    assert manifests[:2] == (root_manifest, main)
    assert set(manifests) == {root_manifest, main, support, *paths}
    assert len(manifests) == len(set(manifests))
    assert workspace_manifest_facts(tmp_path) == facts
    assert set(facts.dependencies) == {
        LocalCargoDependency(main, support, "support", "normal", None),
        LocalCargoDependency(main, paths[0], "shared", "normal", None),
        LocalCargoDependency(main, paths[3], "build", "build", None),
        LocalCargoDependency(main, paths[4], "dev", "dev", None),
        LocalCargoDependency(main, paths[5], "platform", "normal", "cfg(windows)"),
        LocalCargoDependency(main, paths[6], "platform_build", "build", "cfg(unix)"),
        LocalCargoDependency(main, paths[7], "platform_dev", "dev", "cfg(unix)"),
        LocalCargoDependency(support, paths[8], "transitive", "build", None),
        LocalCargoDependency(paths[8], support, "cycle", "dev", None),
    }
    # Workspace inheritance and root overrides are inputs, not invented edges.
    assert not any(edge.source_manifest == root_manifest for edge in facts.dependencies)
    assert not any(
        edge.dependency_manifest in paths[1:3] for edge in facts.dependencies
    )


@pytest.mark.parametrize("explicit_workspace", [False, True])
def test_external_dependency_inheritance_tracks_its_own_workspace(
    tmp_path: Path, explicit_workspace: bool
) -> None:
    project = tmp_path / "project"
    project.mkdir()
    (project / "Cargo.toml").write_text(
        '[workspace]\nmembers = ["main"]\n[workspace.dependencies]\nshared = "1"\n',
        encoding="utf-8",
    )
    main = _manifest(project, "main", "main")
    main.write_text(
        '[package]\nname="main"\n'
        '[dependencies]\nexternal = { path = "../../external/member" }\n'
        'versioned = { path = "../../external/versioned" }\n',
        encoding="utf-8",
    )
    external = tmp_path / "external"
    member = _manifest(external, "member", "member")
    owner = external / ("authority" if explicit_workspace else ".") / "Cargo.toml"
    owner.parent.mkdir(parents=True, exist_ok=True)
    owner.write_text(
        '[workspace]\nmembers=[]\n[workspace.package]\nversion="1.2.3"\n'
        '[workspace.dependencies]\nshared = { path = "shared" }\n',
        encoding="utf-8",
    )
    package_workspace = 'workspace="../authority"\n' if explicit_workspace else ""
    member.write_text(
        '[package]\nname="member"\n'
        + package_workspace
        + "[dependencies]\nshared.workspace=true\n",
        encoding="utf-8",
    )
    versioned = _manifest(external, "versioned", "versioned")
    versioned.write_text(
        '[package]\nname="versioned"\nversion.workspace=true\n' + package_workspace,
        encoding="utf-8",
    )
    shared = _manifest(owner.parent, "shared", "shared")
    facts = workspace_manifest_facts(project)
    assert set(facts.input_manifests) == {
        project / "Cargo.toml",
        main,
        member,
        owner.resolve(),
        shared,
        versioned,
    }
    assert set(facts.dependencies) == {
        LocalCargoDependency(main, member, "external", "normal", None),
        LocalCargoDependency(main, versioned, "versioned", "normal", None),
        LocalCargoDependency(member, shared, "shared", "normal", None),
    }


@pytest.mark.parametrize(
    "dependency,diagnostic",
    [
        ('local = { path = "../missing" }', "Cargo manifest"),
        ("local = { path = 42 }", "nonempty path"),
        ("local = { workspace = true }", "missing workspace.dependencies.local"),
        ("local = { workspace = false }", "workspace must be true"),
        ("local = 42", "must be a table"),
    ],
)
def test_lock_inputs_fail_closed_on_invalid_referenced_dependencies(
    tmp_path: Path, dependency: str, diagnostic: str
) -> None:
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers=["main"]\n', encoding="utf-8"
    )
    main = _manifest(tmp_path, "main", "main")
    main.write_text(
        '[package]\nname="main"\n[dependencies]\n' + dependency, encoding="utf-8"
    )
    with pytest.raises(ValueError, match=diagnostic):
        workspace_manifest_facts(tmp_path)


def test_lock_cache_invalidates_when_excluded_dependency_manifest_changes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    from subprocess import CompletedProcess

    from molt.cli import lockfiles

    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers=["main"]\nexclude=["support"]\n', encoding="utf-8"
    )
    (tmp_path / "Cargo.lock").write_text("# synthetic lock\n", encoding="utf-8")
    main = _manifest(tmp_path, "main", "main")
    main.write_text(
        '[package]\nname="main"\n[build-dependencies]\nsupport={path="../support"}\n',
        encoding="utf-8",
    )
    support = _manifest(tmp_path, "support", "support")
    cached: dict = {}
    commands: list = []
    monkeypatch.setattr(lockfiles.shutil, "which", lambda _tool: "cargo")
    monkeypatch.setattr(
        lockfiles,
        "_is_lock_check_cache_valid",
        lambda _root, _name, inputs: cached.get("inputs") == inputs,
    )
    monkeypatch.setattr(
        lockfiles,
        "_write_lock_check_cache",
        lambda _root, _name, inputs: cached.update(inputs=inputs),
    )

    def cargo(command, **kwargs):
        commands.append(command)
        assert kwargs["cwd"] == tmp_path
        assert "--locked" in command
        return CompletedProcess(command, 0, "", "")

    monkeypatch.setattr(lockfiles, "_run_completed_command", cargo)
    assert lockfiles._verify_cargo_lock(tmp_path) is None
    assert lockfiles._verify_cargo_lock(tmp_path) is None
    assert len(commands) == 1
    support.write_text('[package]\nname="support"\nversion="0.2.0"\n', encoding="utf-8")
    assert lockfiles._verify_cargo_lock(tmp_path) is None
    assert len(commands) == 2
    support.write_text("broken [", encoding="utf-8")
    error = lockfiles._verify_cargo_lock(tmp_path)
    assert error is not None and "Cannot validate Cargo.lock workspace inputs" in error
    assert len(commands) == 2
