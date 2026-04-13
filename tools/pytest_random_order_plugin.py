from __future__ import annotations

import random
from typing import Any


def pytest_addoption(parser) -> None:
    group = parser.getgroup("molt-random-order")
    group.addoption(
        "--molt-random-order",
        action="store_true",
        help="Shuffle collected test order using --molt-random-seed.",
    )
    group.addoption(
        "--molt-random-seed",
        default=None,
        help="Deterministic seed for Molt random-order test collection.",
    )


def _seed_value(seed: str) -> int | str:
    try:
        return int(seed)
    except (TypeError, ValueError):
        return seed


def reorder_items_in_place(items: list[Any], seed: str) -> None:
    rng = random.Random(_seed_value(seed))
    rng.shuffle(items)


def _configured_seed(config) -> str | None:
    if not config.getoption("--molt-random-order"):
        return None
    seed = config.getoption("--molt-random-seed")
    if seed is None or str(seed).strip() == "":
        raise RuntimeError("--molt-random-seed is required with --molt-random-order")
    return str(seed)


def pytest_report_header(config) -> str | None:
    seed = _configured_seed(config)
    if seed is None:
        return None
    return f"molt-random-order seed={seed}"


def pytest_collection_modifyitems(config, items) -> None:
    seed = _configured_seed(config)
    if seed is None:
        return
    reorder_items_in_place(items, seed)
