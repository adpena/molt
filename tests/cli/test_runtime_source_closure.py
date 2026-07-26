from __future__ import annotations

import os
from pathlib import Path

import pytest

from molt.cli import runtime_source_closure as closure
from molt.cli.cargo_source_closure import _cargo_crate_source_closure
from molt.cli.runtime_build_identity import _tree_identity


def _manifest_tree(root: Path) -> None:
    (root / "runtime" / "molt-runtime").mkdir(parents=True)
    (root / "runtime" / "crate-a").mkdir(parents=True)
    for path, text in (
        (root / "Cargo.toml", "[workspace]\n"),
        (root / "Cargo.lock", "# lock\n"),
        (root / "runtime" / "Cargo.toml", "[workspace]\n"),
        (root / "runtime" / "Cargo.lock", "# lock\n"),
        (root / "runtime" / "crate-a" / "Cargo.toml", "[package]\nname='a'\n"),
    ):
        path.write_text(text, encoding="utf-8")


def test_manifest_membership_stamp_observes_same_metadata_content_change(
    tmp_path: Path,
) -> None:
    _manifest_tree(tmp_path)
    manifest = tmp_path / "runtime" / "crate-a" / "Cargo.toml"
    before_stat = manifest.stat()
    before = closure._runtime_manifest_cache_stamp(tmp_path)
    original = manifest.read_text(encoding="utf-8")
    manifest.write_text(original.replace("'a'", "'b'"), encoding="utf-8")
    os.utime(manifest, ns=(before_stat.st_atime_ns, before_stat.st_mtime_ns))

    after = closure._runtime_manifest_cache_stamp(tmp_path)

    assert after != before


def test_source_closure_deduplicates_features_and_keeps_libmpdec(
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
    second = closure.runtime_source_paths(tmp_path, ("feature-a", "feature-b"))

    assert first == second
    assert calls == [("feature-a", "feature-b")]
    assert tmp_path / "third_party/cpython/Modules/_decimal/libmpdec" in extras[0]
    assert tmp_path / "LICENSE" in extras[0]


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


def test_warm_cached_directory_closure_observes_added_rust_module(
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
