from __future__ import annotations

import tomllib
from pathlib import Path

import pytest

from molt.cli import runtime_source_closure as closure
from molt.cli.cargo_source_closure import (
    _cargo_crate_source_closure,
    _cargo_locked_dependency_projection,
)
from molt.cli.runtime_build_identity import _tree_identity


def _manifest_tree(root: Path) -> None:
    (root / "runtime" / "molt-runtime").mkdir(parents=True)
    (root / "runtime" / "crate-a").mkdir(parents=True)
    for path, text in (
        (root / "Cargo.toml", "[workspace]\n"),
        (root / "Cargo.lock", "# lock\n"),
        (root / "runtime" / "crate-a" / "Cargo.toml", "[package]\nname='a'\n"),
    ):
        path.write_text(text, encoding="utf-8")


def test_source_closure_deduplicates_features_and_keeps_shared_inputs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _manifest_tree(tmp_path)
    calls: list[tuple[str, ...]] = []
    extras: list[tuple[Path, ...]] = []

    def fake_cargo_closure(**kwargs: object) -> tuple[Path, ...]:
        calls.append(tuple(kwargs["crate_features"]))
        extras.append(tuple(kwargs["extra_source_paths"]))
        return (tmp_path / "runtime" / "crate-a",)

    monkeypatch.setattr(closure, "_cargo_crate_source_closure", fake_cargo_closure)

    first = closure.runtime_source_paths(
        tmp_path / ".", ("feature-b", "feature-a", "feature-b", "default-features")
    )
    assert first == (tmp_path / "runtime" / "crate-a",)
    assert calls == [("feature-a", "feature-b")]
    assert extras == [
        (
            tmp_path / "Cargo.toml",
            tmp_path / "Cargo.lock",
            tmp_path / "runtime/build_support",
            tmp_path / "LICENSE",
            tmp_path / "include/molt",
            tmp_path / "runtime/wasm_table_layout.inc",
            tmp_path / "runtime/molt-runtime-http/src/functions_logging.rs",
            tmp_path / "runtime/molt-runtime-itertools/src/itertools.rs",
            tmp_path / "runtime/molt-runtime-path/src/os_ext.rs",
            tmp_path / "runtime/molt-runtime-path/src/pathlib.rs",
        )
    ]


def test_cargo_closure_owns_build_script_non_src_inputs(tmp_path: Path) -> None:
    crate = tmp_path / "runtime" / "molt-cpython-abi"
    (crate / "src").mkdir(parents=True)
    (crate / "include").mkdir()
    (crate / "Cargo.toml").write_text(
        "[package]\nname='molt-cpython-abi'\nversion='0.1.0'\n",
        encoding="utf-8",
    )
    (crate / "build.rs").write_text(
        'fn main() { println!("cargo:rerun-if-changed=include/Python.h"); }\n',
        encoding="utf-8",
    )
    header = crate / "include" / "Python.h"
    header.write_text("typedef int Py_ssize_t;\n", encoding="utf-8")

    paths = _cargo_crate_source_closure(
        project_root=tmp_path,
        crate_root=crate,
        crate_features=(),
    )
    before = _tree_identity((("crate", paths[0]),), require_all=True)
    header.write_text("typedef long Py_ssize_t;\n", encoding="utf-8")
    after = _tree_identity((("crate", paths[0]),), require_all=True)

    assert paths == [crate]
    assert before["digest"] != after["digest"]


def test_repeated_directory_closure_observes_added_rust_module(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    _manifest_tree(tmp_path)
    source = tmp_path / "runtime" / "molt-runtime" / "src"
    source.mkdir()
    lib = source / "lib.rs"
    lib.write_text("mod first;\n", encoding="utf-8")
    (source / "first.rs").write_text("pub fn first() {}\n", encoding="utf-8")
    monkeypatch.setattr(
        closure,
        "_cargo_crate_source_closure",
        lambda **_kwargs: (source,),
    )
    paths = closure.runtime_source_paths(tmp_path)
    before = _tree_identity((("source", paths[0]),), require_all=True)

    lib.write_text("mod first;\nmod second;\n", encoding="utf-8")
    (source / "second.rs").write_text("pub fn second() {}\n", encoding="utf-8")
    cached_paths = closure.runtime_source_paths(tmp_path)
    after = _tree_identity((("source", cached_paths[0]),), require_all=True)

    assert cached_paths == paths
    assert after["digest"] != before["digest"]
    assert after["file_count"] == before["file_count"] + 1


def _patched_cargo_tree(root: Path) -> tuple[Path, Path, Path]:
    compiler = root / "runtime/compiler"
    runtime = root / "runtime/runtime"
    patched = root / "vendor/patched"
    leaf = root / "vendor/leaf"
    for crate, name, version in [
        (compiler, "compiler", "0.1.0"),
        (runtime, "runtime", "0.1.0"),
        (patched, "object", "0.39.1"),
        (leaf, "leaf", "1.0.0"),
    ]:
        (crate / "src").mkdir(parents=True)
        (crate / "Cargo.toml").write_text(
            f'[package]\nname="{name}"\nversion="{version}"\n', encoding="utf-8"
        )
        (crate / "src/lib.rs").write_text("pub fn value() {}\n", encoding="utf-8")
    with (compiler / "Cargo.toml").open("a") as stream:
        stream.write('[dependencies]\nbridge="1.0"\n')
    with (runtime / "Cargo.toml").open("a") as stream:
        stream.write('[dependencies]\nobject="0.40"\n')
    with (patched / "Cargo.toml").open("a") as stream:
        stream.write(
            "[target.'cfg(windows)'.dependencies]\nleaf={path=\"../leaf\", optional=true}\n"
        )
    (root / "Cargo.toml").write_text(
        '[workspace]\nmembers=["runtime/compiler", "runtime/runtime"]\n'
        '[patch.crates-io]\nrenamed={package="object",path="vendor/patched"}\n',
        encoding="utf-8",
    )
    (root / "Cargo.lock").write_text(
        "version=4\n"
        '[[package]]\nname="compiler"\nversion="0.1.0"\ndependencies=["bridge"]\n'
        '[[package]]\nname="bridge"\nversion="1.0.0"\n'
        'source="registry+https://github.com/rust-lang/crates.io-index"\n'
        'dependencies=["object 0.39.1"]\n'
        '[[package]]\nname="object"\nversion="0.39.1"\ndependencies=["leaf"]\n'
        '[[package]]\nname="leaf"\nversion="1.0.0"\n'
        '[[package]]\nname="runtime"\nversion="0.1.0"\ndependencies=["object 0.40.0"]\n'
        '[[package]]\nname="object"\nversion="0.40.0"\n'
        'source="registry+https://github.com/rust-lang/crates.io-index"\n',
        encoding="utf-8",
    )
    return compiler, runtime, patched


def _patched_source_digest(root: Path, crate: Path) -> tuple[list[Path], str]:
    paths = _cargo_crate_source_closure(
        project_root=root, crate_root=crate, crate_features=()
    )
    identity = _tree_identity(
        tuple((str(path.relative_to(root)), path) for path in paths), require_all=True
    )
    return paths, identity["digest"]


def test_selected_patch_bytes_and_transitive_paths_change_compiler_identity(
    tmp_path: Path,
) -> None:
    compiler, runtime, patched = _patched_cargo_tree(tmp_path)
    compiler_paths, before = _patched_source_digest(tmp_path, compiler)
    runtime_paths, runtime_before = _patched_source_digest(tmp_path, runtime)
    assert set(compiler_paths) == {compiler, patched, tmp_path / "vendor/leaf"}
    assert runtime_paths == [runtime]
    # Registry->patched edges must be followed even though neither compiler's
    # local manifest nor its feature graph names the patched crate directly.
    (patched / "src/lib.rs").write_text(
        "pub fn fixed_relocation() {}\n", encoding="utf-8"
    )
    assert _patched_source_digest(tmp_path, compiler)[1] != before
    assert _patched_source_digest(tmp_path, runtime)[1] == runtime_before
    before_leaf = _patched_source_digest(tmp_path, compiler)[1]
    (tmp_path / "vendor/leaf/src/lib.rs").write_text(
        "pub fn changed() {}\n", encoding="utf-8"
    )
    assert _patched_source_digest(tmp_path, compiler)[1] != before_leaf
    assert _patched_source_digest(tmp_path, runtime)[1] == runtime_before


@pytest.mark.parametrize(
    "defect",
    ["missing", "outside", "identity", "stale-lock", "stale-version", "bad-path"],
)
def test_path_patch_admission_fails_closed(tmp_path: Path, defect: str) -> None:
    compiler, _, patched = _patched_cargo_tree(tmp_path)
    if defect == "missing":
        (patched / "Cargo.toml").unlink()
    elif defect == "outside":
        (tmp_path / "Cargo.toml").write_text(
            '[workspace]\n[patch.crates-io]\nobject={path="../outside"}\n',
            encoding="utf-8",
        )
    elif defect == "identity":
        manifest = patched / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                'name="object"', 'name="impostor"'
            ),
            encoding="utf-8",
        )
    elif defect == "stale-version":
        manifest = patched / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                'version="0.39.1"', 'version="0.39.2"'
            ),
            encoding="utf-8",
        )
    elif defect == "bad-path":
        manifest = tmp_path / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                'path="vendor/patched"', "path=7"
            ),
            encoding="utf-8",
        )
    else:
        lock = tmp_path / "Cargo.lock"
        lock.write_text(
            lock.read_text(encoding="utf-8").replace(
                'name="object"\nversion="0.39.1"\n',
                'name="object"\nversion="0.39.1"\nsource="registry+https://github.com/rust-lang/crates.io-index"\n',
            ),
            encoding="utf-8",
        )
    with pytest.raises((OSError, ValueError)):
        _patched_source_digest(tmp_path, compiler)


def test_patch_symlink_escape_is_rejected(tmp_path: Path) -> None:
    root = tmp_path / "project"
    compiler, _, patched = _patched_cargo_tree(root)
    external = tmp_path / "external"
    external.mkdir()
    (external / "Cargo.toml").write_text(
        '[package]\nname="object"\nversion="0.39.1"\n', encoding="utf-8"
    )
    alias = root / "vendor/escape"
    try:
        alias.symlink_to(external, target_is_directory=True)
    except OSError as error:
        pytest.skip(f"Host cannot create directory symlinks: {error}")
    manifest = root / "Cargo.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace("vendor/patched", "vendor/escape"),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="escapes"):
        _patched_source_digest(root, compiler)


def test_cargo_unused_patch_record_does_not_claim_registry_package_bytes(
    tmp_path: Path,
) -> None:
    compiler, _, patched = _patched_cargo_tree(tmp_path)
    lock = tmp_path / "Cargo.lock"
    lock.write_text(
        lock.read_text(encoding="utf-8").replace(
            'name="object"\nversion="0.39.1"\n',
            'name="object"\nversion="0.39.1"\nsource="registry+https://other.invalid/index"\n',
        )
        + '\n[[patch.unused]]\nname="object"\nversion="0.39.1"\n',
        encoding="utf-8",
    )
    paths, before = _patched_source_digest(tmp_path, compiler)
    assert paths == [compiler]
    (patched / "src/lib.rs").write_text("pub fn unused_edit() {}\n", encoding="utf-8")
    assert _patched_source_digest(tmp_path, compiler)[1] == before


def test_selected_patch_follows_workspace_inherited_local_dependency(
    tmp_path: Path,
) -> None:
    compiler, _, patched = _patched_cargo_tree(tmp_path)
    workspace = tmp_path / "Cargo.toml"
    workspace.write_text(
        workspace.read_text(encoding="utf-8")
        + '\n[workspace.dependencies]\nleaf={path="vendor/leaf"}\n',
        encoding="utf-8",
    )
    manifest = patched / "Cargo.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            'path="../leaf"', "workspace=true"
        ),
        encoding="utf-8",
    )
    paths, before = _patched_source_digest(tmp_path, compiler)
    assert tmp_path / "vendor/leaf" in paths
    (tmp_path / "vendor/leaf/src/lib.rs").write_text(
        "pub fn changed_inherited() {}\n", encoding="utf-8"
    )
    assert _patched_source_digest(tmp_path, compiler)[1] != before


def test_workspace_inherited_patch_dependency_rejects_invalid_features(
    tmp_path: Path,
) -> None:
    compiler, _, patched = _patched_cargo_tree(tmp_path)
    workspace = tmp_path / "Cargo.toml"
    workspace.write_text(
        workspace.read_text(encoding="utf-8")
        + '\n[workspace.dependencies]\nleaf={path="vendor/leaf", features="invalid"}\n',
        encoding="utf-8",
    )
    manifest = patched / "Cargo.toml"
    manifest.write_text(
        manifest.read_text(encoding="utf-8").replace(
            'path="../leaf"', "workspace=true"
        ),
        encoding="utf-8",
    )
    with pytest.raises(ValueError, match="Invalid Cargo dependency features"):
        _patched_source_digest(tmp_path, compiler)


def _assert_runtime_zlib_provider(
    repo_root: Path, workspace_root: Path, consumer_root: Path
) -> None:
    vendor = (repo_root / "vendor/rust/zlib-rs").resolve()
    package = tomllib.loads((vendor / "Cargo.toml").read_text(encoding="utf-8"))[
        "package"
    ]
    assert package["name"] == "zlib-rs"
    workspace = tomllib.loads(
        (workspace_root / "Cargo.toml").read_text(encoding="utf-8")
    )
    patch = workspace["patch"]["crates-io"]["zlib-rs"]
    assert patch.get("package", "zlib-rs") == "zlib-rs"
    assert (workspace_root / patch["path"]).resolve() == vendor
    projection = _cargo_locked_dependency_projection(workspace_root, consumer_root)
    selected = [node for node in projection["packages"] if node["name"] == "zlib-rs"]
    # Testing the patched workspace member alone proves nothing about these
    # actual consumers: a newer registry version can satisfy their ^0.6 ranges.
    assert selected, f"No zlib provider reachable from {consumer_root}"
    assert [(node["version"], node.get("source", "")) for node in selected] == [
        (package["version"], "")
    ], f"Runtime bypasses its maintained zlib provider: {selected!r}"


@pytest.mark.parametrize(
    ("workspace_relative", "consumer_relative"),
    [
        (".", "runtime/molt-runtime-compression"),
        (".", "runtime/molt-runtime"),
        ("runtime/molt-runtime/fuzz", "runtime/molt-runtime/fuzz"),
    ],
)
def test_actual_runtime_consumers_select_maintained_zlib_provider(
    workspace_relative: str, consumer_relative: str
) -> None:
    repo_root = Path(__file__).resolve().parents[2]
    _assert_runtime_zlib_provider(
        repo_root, repo_root / workspace_relative, repo_root / consumer_relative
    )


@pytest.mark.parametrize(
    "selection",
    [
        "local",
        "newer-registry",
        "same-version-registry",
        "mixed",
        "missing",
        "wrong-path",
    ],
)
def test_runtime_zlib_provider_guard_observes_consumer_edges(
    tmp_path: Path, selection: str
) -> None:
    # Keep the vendor package selected as a workspace member in every case,
    # including the broken graph where actual runtime edges use the registry.
    vendor = tmp_path / "vendor/rust/zlib-rs"
    consumer = tmp_path / "runtime/molt-runtime-compression"
    for path, name, version in [
        (vendor, "zlib-rs", "0.6.8"),
        (consumer, "molt-runtime-compression", "0.1.0"),
    ]:
        path.mkdir(parents=True)
        (path / "Cargo.toml").write_text(
            f'[package]\nname="{name}"\nversion="{version}"\n', encoding="utf-8"
        )
    (tmp_path / "Cargo.toml").write_text(
        '[workspace]\nmembers=["runtime/molt-runtime-compression", "vendor/rust/zlib-rs"]\n'
        '[patch.crates-io]\nzlib-rs={path="vendor/rust/zlib-rs"}\n',
        encoding="utf-8",
    )
    registry = "registry+https://github.com/rust-lang/crates.io-index"
    registry_version = "0.6.8" if selection == "same-version-registry" else "0.6.9"
    selected = ["zlib-rs 0.6.8"]
    if selection in {"newer-registry", "same-version-registry"}:
        selected = [f"zlib-rs {registry_version} ({registry})"]
    elif selection == "mixed":
        selected.append(f"zlib-rs {registry_version} ({registry})")
    elif selection == "missing":
        selected = []
    dependencies = ", ".join(f'"{reference}"' for reference in selected)
    (tmp_path / "Cargo.lock").write_text(
        "version=4\n"
        '[[package]]\nname="molt-runtime-compression"\nversion="0.1.0"\n'
        'dependencies=["flate2"]\n'
        '[[package]]\nname="flate2"\nversion="1.1.9"\n'
        f'source="{registry}"\ndependencies=[{dependencies}]\n'
        '[[package]]\nname="zlib-rs"\nversion="0.6.8"\n'
        f'[[package]]\nname="zlib-rs"\nversion="{registry_version}"\nsource="{registry}"\n'
        # Unrelated multiple object versions remain valid; this invariant is
        # specific to the one maintained runtime compression provider family.
        '[[package]]\nname="object"\nversion="0.39.1"\n'
        f'[[package]]\nname="object"\nversion="0.40.0"\nsource="{registry}"\n',
        encoding="utf-8",
    )
    if selection == "wrong-path":
        other = tmp_path / "vendor/other-zlib"
        other.mkdir()
        (other / "Cargo.toml").write_bytes((vendor / "Cargo.toml").read_bytes())
        manifest = tmp_path / "Cargo.toml"
        manifest.write_text(
            manifest.read_text(encoding="utf-8").replace(
                'path="vendor/rust/zlib-rs"', 'path="vendor/other-zlib"'
            ),
            encoding="utf-8",
        )
    if selection == "local":
        _assert_runtime_zlib_provider(tmp_path, tmp_path, consumer)
    else:
        with pytest.raises(AssertionError):
            _assert_runtime_zlib_provider(tmp_path, tmp_path, consumer)
