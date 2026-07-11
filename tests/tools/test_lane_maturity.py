from tools import lane_maturity as lm


def test_wasm_refused_below_l1():
    decision = lm.decide(maturity="L0", resource_family="wasm-browser")
    assert not decision.allow and decision.required == "L1"


def test_cross_lane_requires_exact_verified():
    assert not lm.decide(maturity="L3", resource_family="python", cross_lane=True).allow
    assert lm.decide(maturity="L4", resource_family="python", cross_lane=True).allow


def test_l7_transition_requires_paired_authorities_and_matrix():
    evidence = list(lm.REQUIRED_EVIDENCE["L7"])
    assert lm.decide_transition(target="L7", evidence=evidence).allow
    evidence.remove("windows_wasm_parity")
    assert not lm.decide_transition(target="L7", evidence=evidence).allow


def test_registry_failure_is_loud_fail_open(monkeypatch, tmp_path, capsys):
    monkeypatch.setattr(
        lm, "read_registry", lambda root: (_ for _ in ()).throw(OSError("boom"))
    )
    assert lm.admission_check(
        repo_root=tmp_path, lane_id="x", resource_family="wasm"
    ).allow
    assert "LOUD fail-open" in capsys.readouterr().err
