"""Runtime receipts are bound to the release source through the canonical tree
identity, hashing the union of every receipt's source roots exactly once."""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from molt.cli import runtime_build_identity as identity


@pytest.fixture
def release_source(tmp_path: Path, monkeypatch) -> Path:
    root = tmp_path / "source"
    for relative, data in {
        "runtime/core/lib.rs": b"core",
        "runtime/core/mod.rs": b"mod",
        "runtime/net/lib.rs": b"net",
        "runtime/gpu/lib.rs": b"gpu",
        "Cargo.toml": b"[workspace]\n",
    }.items():
        path = root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
    closure = {
        (): ("runtime/core", "Cargo.toml"),
        ("stdlib_net",): ("runtime/core", "runtime/net", "Cargo.toml"),
        ("molt_gpu_primitives",): ("runtime/core", "runtime/gpu", "runtime/absent"),
    }

    def runtime_source_paths(project_root, runtime_features=()):
        key = tuple(sorted(set(runtime_features) - {"no-default-features"}))
        return tuple(Path(project_root) / item for item in closure[key])

    monkeypatch.setattr(identity, "runtime_source_paths", runtime_source_paths)
    return root


_FEATURES = ((), ("stdlib_net",), ("no-default-features", "molt_gpu_primitives"))


def _recorded(root: Path, features):
    return identity._tree_identity(
        identity.runtime_source_roots(root, features), require_all=False
    )


def test_one_index_projects_every_canonical_tree_identity(release_source, monkeypatch):
    hashed: list[str] = []
    real_hash = identity._hash_tree_input_file

    def counting(file):
        hashed.append(file.label)
        return real_hash(file)

    expected = [_recorded(release_source, features) for features in _FEATURES]
    monkeypatch.setattr(identity, "_hash_tree_input_file", counting)
    identity.verify_runtime_source_trees(
        release_source, list(zip(_FEATURES, expected, strict=True))
    )
    # Five distinct files across three receipts: each is hashed once.
    assert sorted(hashed) == sorted(set(hashed)) and len(hashed) == 5
    assert expected[2]["missing"] == ["source/runtime/absent"]


def test_tree_identity_is_the_index_projection(release_source):
    roots = identity.runtime_source_roots(release_source, ("stdlib_net",))
    index = identity.RuntimeTreeIndex.capture(roots + roots)
    assert index.identity(roots, require_all=False) == identity._tree_identity(
        roots, require_all=False
    )
    with pytest.raises(ValueError, match="missing"):
        identity._tree_identity(
            identity.runtime_source_roots(release_source, ("molt_gpu_primitives",)),
            require_all=True,
        )


def test_a_receipt_from_other_sources_is_rejected(release_source):
    recorded = [_recorded(release_source, features) for features in _FEATURES]
    (release_source / "runtime" / "net" / "lib.rs").write_bytes(b"patched")
    with pytest.raises(ValueError, match="stdlib_net"):
        identity.verify_runtime_source_trees(
            release_source, list(zip(_FEATURES, recorded, strict=True))
        )


def test_index_rejects_roots_it_did_not_capture(release_source):
    index = identity.RuntimeTreeIndex.capture(
        identity.runtime_source_roots(release_source, ())
    )
    with pytest.raises(ValueError, match="not in this tree index"):
        index.identity(
            identity.runtime_source_roots(release_source, ("stdlib_net",)),
            require_all=False,
        )


@pytest.fixture
def build_trees(release_source, monkeypatch):
    tooling = release_source / "planner.py"
    tooling.write_bytes(b"before")
    monkeypatch.setattr(identity, "runtime_build_tooling_paths", lambda _root: (tooling,))
    source_roots = identity.runtime_source_roots(release_source, ("molt_gpu_primitives",))
    return release_source, source_roots, tooling


def test_source_and_tooling_union_preserves_separate_tree_receipts(
    build_trees, monkeypatch
):
    root, sources, tooling = build_trees
    tooling_roots = (("runtime-tooling/planner.py", tooling),)
    expected_sources = identity._tree_identity(sources, require_all=False)
    expected_tooling = {
        "schema": "molt.runtime-build-tooling-authority.v2",
        **identity._tree_identity(tooling_roots, require_all=True),
    }
    hashed = []
    real_hash = identity._hash_tree_input_file

    def hash_file(file):
        hashed.append(file.label)
        return real_hash(file)

    monkeypatch.setattr(identity, "_hash_tree_input_file", hash_file)
    observed_sources, observed_tooling = identity._capture_runtime_build_trees(
        root, sources
    )

    assert observed_sources == expected_sources
    assert observed_tooling == expected_tooling
    assert observed_sources["missing"] == ["source/runtime/absent"]
    assert sorted(hashed) == [
        "runtime-tooling/planner.py",
        "source/runtime/core/lib.rs",
        "source/runtime/core/mod.rs",
        "source/runtime/gpu/lib.rs",
    ]


@pytest.mark.parametrize("mutation", ["rewrite", "remove", "missing-root", "tooling"])
def test_next_live_tree_capture_observes_bytes_and_membership(build_trees, mutation):
    root, sources, tooling = build_trees
    before_sources, before_tooling = identity._capture_runtime_build_trees(root, sources)
    if mutation == "missing-root":
        path = root / "runtime/absent/new.rs"
        path.parent.mkdir()
        path.write_bytes(b"newly selected file")
    elif mutation == "remove":
        (root / "runtime/core/mod.rs").unlink()
    else:
        path = tooling if mutation == "tooling" else root / "runtime/core/lib.rs"
        metadata = path.stat()
        path.write_bytes(b"x" * metadata.st_size)
        os.utime(path, ns=(metadata.st_atime_ns, metadata.st_mtime_ns))
    after_sources, after_tooling = identity._capture_runtime_build_trees(root, sources)
    assert (before_sources == after_sources) is (mutation == "tooling")
    assert (before_tooling == after_tooling) is (mutation != "tooling")
    if mutation == "missing-root":
        assert after_sources["missing"] == []


def test_union_still_requires_every_tooling_root(build_trees):
    root, sources, tooling = build_trees
    tooling.unlink()
    with pytest.raises(
        ValueError, match="required runtime inputs.*runtime-tooling/planner.py"
    ):
        identity._capture_runtime_build_trees(root, sources)
