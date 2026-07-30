from __future__ import annotations

import copy
import json

import pytest

from tools import nightly_profile_feedback, nightly_shard_profile, nightly_sharding


def test_promotion_requires_an_exact_candidate_report_pair() -> None:
    candidate = json.loads(
        (nightly_sharding.ROOT / "config/nightly_shard_profile.json").read_text(
            encoding="utf-8"
        )
    )
    report = {
        "schema": nightly_profile_feedback.REPORT_SCHEMA,
        "candidate_profile_sha256": nightly_shard_profile.profile_digest(candidate),
        "programs": {
            program: {"accepted": False}
            for program in nightly_sharding.SHARD_COUNTS
        },
    }

    assert nightly_profile_feedback.promote(candidate, report) == candidate
    tampered = copy.deepcopy(report)
    tampered["candidate_profile_sha256"] = "0" * 64
    with pytest.raises(ValueError, match="promotion report is invalid"):
        nightly_profile_feedback.promote(candidate, tampered)
