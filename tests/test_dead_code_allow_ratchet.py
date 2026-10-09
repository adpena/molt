"""Teeth for the dead-code and cfg-corpse registry ratchet.

The canaries plant their probes in a fixture checkout, never in this
repository: a file there would dirty the checkout that every parallel proof
command attests.
"""

from __future__ import annotations
from tests.process_guard_common import run_guarded_test_process

import json
from pathlib import Path
import subprocess
import sys

import pytest

ROOT = Path(__file__).resolve().parents[1]
GATE = ROOT / "tools" / "dead_code_allow_ratchet.py"
REGISTRY = ROOT / "tools" / "dead_code_allow_baseline.json"


def _run(root: Path = ROOT) -> subprocess.CompletedProcess[str]:
    return run_guarded_test_process(
        [sys.executable, str(GATE), "--root", str(root)],
        cwd=ROOT,
        capture_output=False,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        check=False,
    )


def test_registry_entries_name_owner_and_waiver() -> None:
    data = json.loads(REGISTRY.read_text(encoding="utf-8"))
    assert data["baseline_total"] == len(data["entries"])
    assert data["entries"]
    assert all(entry["owner"].strip() for entry in data["entries"])
    assert all(entry["waiver"].strip() for entry in data["entries"])


def test_passes_at_registered_baseline() -> None:
    proc = _run()
    assert proc.returncode == 0, proc.stdout
    assert "PASS" in proc.stdout


def _fixture_checkout(root: Path) -> Path:
    """A checkout with an empty registry and an empty runtime source tree."""
    (root / "tools").mkdir(parents=True)
    (root / "tools" / REGISTRY.name).write_text(
        json.dumps({"baseline_total": 0, "entries": []}), encoding="utf-8"
    )
    source = root / "runtime" / "molt-runtime" / "src"
    source.mkdir(parents=True)
    return source


@pytest.mark.parametrize(
    ("probe_source", "finding"),
    [
        ("#[allow(dead_code)]\nfn probe() {}\n", "unwaived allow_dead_code"),
        ("#[cfg(any())]\nfn corpse() {}\n", "unwaived cfg_corpse"),
    ],
)
def test_liveness_canary_rejects_an_unregistered_site(
    tmp_path: Path, probe_source: str, finding: str
) -> None:
    root = tmp_path / "checkout"
    source = _fixture_checkout(root)
    assert _run(root).returncode == 0

    probe = source / "probe.rs"
    probe.write_text(probe_source, encoding="utf-8")
    proc = _run(root)
    assert proc.returncode == 2, proc.stderr
    assert finding in proc.stderr
    assert "runtime/molt-runtime/src/probe.rs" in proc.stderr

    probe.unlink()
    assert _run(root).returncode == 0


def _scan_ids(root: Path) -> list[str]:
    from tools import dead_code_allow_ratchet as ratchet

    return [site.id.split("::", 1)[1] for site in ratchet.scan(root)]


@pytest.mark.parametrize(
    ("source", "item"),
    [
        ("#[allow(dead_code)]\nfn probe() {}\n", "fn:probe"),
        (
            "/// Doc.\n#[allow(clippy::foo, dead_code)]\n#[inline]\n"
            "pub(in crate::a) const unsafe fn probe() {}\n",
            "fn:probe",
        ),
        ("#[allow(dead_code)]\nconst LIMIT: u8 = 1;\n", "const:LIMIT"),
        ("#[allow(dead_code)]\nstatic mut STATE: u8 = 0;\n", "static:STATE"),
        ("#[allow(dead_code)]\nmod helpers;\n", "mod:helpers"),
        ("#[allow(dead_code)]\ntype Alias = u8;\n", "type:Alias"),
        ("#[allow(dead_code)]\nuse crate::a::{b, c};\n", "use:crate::a::{b,c}"),
        ("#[allow(dead_code)]\nmacro_rules! probe { () => {} }\n", "macro_rules:probe"),
        (
            "#[allow(dead_code)]\nimpl<T: Copy> Probe<T> for Holder<T>\nwhere T: Eq {}\n",
            "impl:<T: Copy> Probe<T> for Holder<T>",
        ),
        (
            "struct S {\n    #[allow(dead_code)]\n    used_count: u8,\n}\n",
            "field:used_count",
        ),
        ("enum E {\n    #[allow(dead_code)]\n    Unused(u8),\n}\n", "variant:Unused"),
        ("#![allow(dead_code)]\nfn probe() {}\n", "inner"),
        ("#[cfg(any())]\nfn corpse() {}\n", "fn:corpse"),
    ],
)
def test_site_ids_name_the_masked_item(tmp_path: Path, source: str, item: str) -> None:
    root = tmp_path / "checkout"
    (_fixture_checkout(root) / "probe.rs").write_text(source, encoding="utf-8")
    assert _scan_ids(root) == [
        f"{'cfg_corpse' if 'cfg(any' in source else 'allow_dead_code'}::{item}"
    ]


def test_masks_in_strings_are_emitted_sites_and_comments_are_not(
    tmp_path: Path,
) -> None:
    root = tmp_path / "checkout"
    (_fixture_checkout(root) / "generator.rs").write_text(
        "// #[allow(dead_code)] is what the table gets.\n"
        "fn write_table(out: &mut String) {\n"
        '    out.push_str("#[allow(dead_code)]\\n");\n'
        '    out.push_str(r#"#[allow(dead_code)] const X: u8 = 1;"#);\n'
        "}\n",
        encoding="utf-8",
    )
    assert _scan_ids(root) == [
        "allow_dead_code::emitted-by:write_table",
        "allow_dead_code::emitted-by:write_table#2",
    ]


def test_an_inserted_mask_leaves_the_existing_site_ids_alone(tmp_path: Path) -> None:
    root = tmp_path / "checkout"
    probe = _fixture_checkout(root) / "probe.rs"
    probe.write_text("#[allow(dead_code)]\nfn kept() {}\n", encoding="utf-8")
    update = [
        sys.executable,
        str(GATE),
        "--root",
        str(root),
        "--update",
        "--owner",
        "fixture-owner",
        "--waiver",
        "fixture site kept for the test",
    ]
    assert run_guarded_test_process(update, cwd=ROOT, check=False).returncode == 0

    probe.write_text(
        "#[allow(dead_code)]\nfn inserted() {}\n#[allow(dead_code)]\nfn kept() {}\n",
        encoding="utf-8",
    )
    proc = _run(root)
    assert proc.returncode == 2
    # An ordinal ID would rename `kept` and stale its waiver.
    assert "fn:inserted" in proc.stderr
    assert "stale" not in proc.stderr


def test_an_unnamed_masked_shape_fails_closed(tmp_path: Path) -> None:
    root = tmp_path / "checkout"
    (_fixture_checkout(root) / "probe.rs").write_text(
        "fn f() {\n    #[allow(dead_code)]\n    42;\n}\n", encoding="utf-8"
    )
    proc = _run(root)
    assert proc.returncode == 3
    assert "runtime/molt-runtime/src/probe.rs: line 2" in proc.stderr


def test_update_keeps_waivers_and_demands_one_for_each_new_site(
    tmp_path: Path,
) -> None:
    from tools import dead_code_allow_ratchet as ratchet

    root = tmp_path / "checkout"
    source = _fixture_checkout(root)
    (source / "probe.rs").write_text(
        "#[allow(dead_code)]\nfn kept() {}\n#[allow(dead_code)]\nfn new() {}\n",
        encoding="utf-8",
    )
    registry = {
        "baseline_total": 2,
        "entries": [
            {
                "id": "runtime/molt-runtime/src/probe.rs::allow_dead_code::fn:kept",
                "owner": "kept-owner",
                "waiver": "kept for a reason",
            },
            {
                "id": "runtime/molt-runtime/src/probe.rs::allow_dead_code::fn:gone",
                "owner": "gone-owner",
                "waiver": "deleted since",
            },
        ],
    }
    sites = ratchet.scan(root)
    with pytest.raises(ValueError, match="fn:new needs --owner and --waiver"):
        ratchet.updated_entries(sites, registry, owner=None, waiver=None)
    entries = ratchet.updated_entries(
        sites, registry, owner="new-owner", waiver="new mask reviewed"
    )
    assert [(entry["id"].rsplit("::", 1)[1], entry["owner"]) for entry in entries] == [
        ("fn:kept", "kept-owner"),
        ("fn:new", "new-owner"),
    ]
