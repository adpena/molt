import tools.triality as triality


def entry():
    invariant = "one fact"
    fp = triality.invariant_fingerprint(invariant)
    return {
        "finding_id": "x",
        "invariant": invariant,
        "legs": {leg: {"authority": leg, "fingerprint": fp} for leg in triality.LEGS},
    }


def test_all_three_agree_is_known():
    assert triality.decide(entry()).known


def test_missing_leg_has_teeth():
    bad = entry()
    del bad["legs"]["equations"]
    verdict = triality.decide(bad)
    assert not verdict.known and "missing equations leg" in verdict.reasons


def test_disagreement_has_teeth():
    bad = entry()
    bad["legs"]["dsl"]["fingerprint"] = "wrong"
    assert not triality.decide(bad).known
