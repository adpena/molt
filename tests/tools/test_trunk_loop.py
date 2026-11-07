from __future__ import annotations

import zipfile

import pytest

from tools import trunk_loop

CELLS = ("portability.queue.linux",)


def _member(name: str, size: int = 1) -> zipfile.ZipInfo:
    item = zipfile.ZipInfo(name)
    item.file_size = size
    return item


def test_cell_results_may_nest_receipt_detail():
    # The proof executor writes guard profiles under `<receipt>.d/`; postsubmit
    # collection refused them on 2026-11-07 because non-landing runs were held
    # to exactly `cell/file`.
    for name in (
        "portability.queue.linux/receipt.json",
        "portability.queue.linux/receipt.d/"
        "portability.queue.linux.attempt-1.profile.jsonl",
    ):
        path = trunk_loop.admitted_result_member(_member(name), CELLS)
        assert path.parts[0] == "portability.queue.linux"


@pytest.mark.parametrize(
    "name, size",
    [
        ("portability.queue.linux", 1),  # no file below the cell
        ("/portability.queue.linux/receipt.json", 1),
        ("portability.queue.linux/../escape.json", 1),
        ("portability.queue.linux\\..\\escape.json", 1),
        ("other.cell/receipt.json", 1),
        ("portability.queue.linux/" + "/".join(["d"] * 16) + "/deep.json", 1),
        ("portability.queue.linux/receipt.json", 512 * 1024 * 1024 + 1),
    ],
)
def test_result_members_stay_inside_a_selected_cell(name, size):
    with pytest.raises(ValueError, match="unexpected result archive member"):
        trunk_loop.admitted_result_member(_member(name, size), CELLS)
