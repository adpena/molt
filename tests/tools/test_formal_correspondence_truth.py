from __future__ import annotations


from tools import check_correspondence


def test_general_correspondence_receipt_rejects_no_categories() -> None:
    receipt = check_correspondence.json_report([])
    assert receipt["status"] == "failure"
    assert receipt["executed"] == 0
    assert receipt["zero_work"] is True


def test_correspondence_receipt_enforces_counted_invariants() -> None:
    categories = [
        check_correspondence.CategoryResult(
            category="one",
            description="passing category",
            items=[
                check_correspondence.CheckItem("a", True),
                check_correspondence.CheckItem("b", True),
            ],
        ),
        check_correspondence.CategoryResult(
            category="two",
            description="failing category",
            items=[check_correspondence.CheckItem("c", False)],
        ),
    ]

    receipt = check_correspondence.json_report(categories)
    assert receipt["selected"] == receipt["executed"] == 3
    assert receipt["executed"] == (
        receipt["passed"] + receipt["failed"] + receipt["errors"]
    )
    assert receipt["status"] == "failure"
    for category in receipt["categories"]:
        assert category["selected"] == category["executed"]
        assert category["executed"] == (
            category["passed"] + category["failed"] + category["errors"]
        )
        assert category["zero_work"] is False
