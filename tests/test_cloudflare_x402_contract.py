"""The Falcon-OCR worker's x402 payment verification, run under Node.

``deploy/cloudflare/x402.js`` decides whether a paid OCR call may run. Each
test builds a payment proof the way an x402 client does (base64 JSON in the
``X-Payment-402`` header), stubs the signature verification endpoint, and
reads what ``verifyX402`` decides. The price, currency, recipient and the
300-second clock window are the protocol's terms, so they are the oracle.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path

import pytest

from tests.process_guard_common import run_guarded_test_process

ROOT = Path(__file__).resolve().parents[1]
WALLET = "0x00000000000000000000000000000000000000A1"
VERIFIER = "https://verifier.example/verify"

_HARNESS = f"""
const {{ verifyX402 }} = await import('./deploy/cloudflare/x402.js');
const verified = [];
globalThis.fetch = async (url, init) => {{
  verified.push({{ url, body: JSON.parse(init.body) }});
  const proof = JSON.parse(init.body);
  const valid = proof.signature === '0xgood';
  return new Response(JSON.stringify(valid ? {{ valid, receipt: 'receipt-1' }} : {{ valid }}));
}};
const now = Math.floor(Date.now() / 1000);
const proof = (overrides = {{}}, payload = {{}}) => btoa(JSON.stringify({{
  version: '1',
  network: 'base',
  payload: {{
    sender: '0x00000000000000000000000000000000000000b2',
    recipient: '{WALLET.lower()}',
    amount: '1000',
    currency: 'USDC',
    timestamp: now,
    nonce: 'n-1',
    ...payload,
  }},
  signature: '0xgood',
  ...overrides,
}}));
const env = {{ X402_WALLET_ADDRESS: '{WALLET}', X402_VERIFICATION_URL: '{VERIFIER}' }};
async function decide(header, environment = env) {{
  const headers = header === null ? {{}} : {{ 'X-Payment-402': header }};
  const request = new Request('https://falcon-ocr.example/ocr', {{ method: 'POST', headers }});
  const result = await verifyX402(request, environment, 'rid', {{}});
  if (result.authorized) return {{ authorized: true, receipt: result.receipt ?? null }};
  const body = await result.response.json();
  return {{
    authorized: false,
    status: result.response.status,
    error: body.error,
    amount: body.payment_required.amount,
    price_usd: body.payment_required.price_usd,
  }};
}}
const results = {{}};
"""


def _run(cases: str) -> tuple[dict, list]:
    if shutil.which("node") is None:
        pytest.skip("node is required for the x402 contract tests")
    script = (
        _HARNESS
        + cases
        + "\nprocess.stdout.write(JSON.stringify({ results, verified }) + '\\n');\n"
    )
    run = run_guarded_test_process(
        ["node", "--input-type=module", "-e", script],
        prefix="MOLT_NODE_TEST",
        cwd=str(ROOT),
        check=True,
        capture_output=True,
        text=True,
    )
    payload = json.loads(run.stdout.strip().splitlines()[-1])
    return payload["results"], payload["verified"]


def test_a_valid_proof_is_authorized_after_signature_verification() -> None:
    results, verified = _run(
        """
        results.exact = await decide(proof());
        results.overpaid = await decide(proof({}, { amount: '5000' }));
        """
    )

    assert results["exact"] == {"authorized": True, "receipt": "receipt-1"}
    assert results["overpaid"]["authorized"] is True
    # The signature check sends the proof itself to the configured verifier.
    assert [call["url"] for call in verified] == [VERIFIER, VERIFIER]
    assert verified[0]["body"]["payload"]["nonce"] == "n-1"
    assert verified[0]["body"]["signature"] == "0xgood"


def test_each_broken_term_of_a_proof_answers_402() -> None:
    results, verified = _run(
        """
        results.missing = await decide(null);
        results.notBase64 = await decide('!!!');
        results.version = await decide(proof({ version: '2' }));
        results.noNonce = await decide(proof({}, { nonce: '' }));
        results.noSignature = await decide(proof({ signature: '' }));
        results.recipient = await decide(proof({}, { recipient: '0xother' }));
        results.underpaid = await decide(proof({}, { amount: '999' }));
        results.currency = await decide(proof({}, { currency: 'ETH' }));
        results.amountText = await decide(proof({}, { amount: 'lots' }));
        results.stale = await decide(proof({}, { timestamp: now - 301 }));
        results.future = await decide(proof({}, { timestamp: now + 301 }));
        results.badSignature = await decide(proof({ signature: '0xbad' }));
        results.noVerifier = await decide(proof(), { X402_WALLET_ADDRESS: env.X402_WALLET_ADDRESS });
        """
    )

    reasons = {
        "missing": "Missing X-Payment-402 header",
        "notBase64": "Failed to parse X-Payment-402 header",
        "version": "Unsupported x402 version: 2",
        "noNonce": "Missing or invalid nonce",
        "noSignature": "Missing payment signature",
        "recipient": "Payment recipient does not match expected wallet",
        "underpaid": "Insufficient payment: 999 USDC units (minimum: 1000)",
        "currency": "Unsupported currency: ETH. Only USDC accepted.",
        "amountText": "Invalid payment amount: lots",
        "stale": "Payment timestamp too far from server time",
        "future": "Payment timestamp too far from server time",
        "badSignature": "Signature verification failed",
        "noVerifier": "x402 verification endpoint not configured",
    }
    for name, reason in reasons.items():
        decision = results[name]
        assert decision["authorized"] is False, name
        assert decision["status"] == 402, name
        assert decision["error"].startswith(reason), (name, decision["error"])
        # Every refusal states the price: 1000 USDC units, $0.001.
        assert (decision["amount"], decision["price_usd"]) == ("1000", 0.001), name
    # Only the proof with every local term right reaches the verifier.
    assert [call["body"]["signature"] for call in verified] == ["0xbad"]


def test_the_clock_window_is_300_seconds_on_both_sides() -> None:
    results, _ = _run(
        """
        results.past = await decide(proof({}, { timestamp: now - 299 }));
        results.future = await decide(proof({}, { timestamp: now + 299 }));
        """
    )

    assert results["past"]["authorized"] is True
    assert results["future"]["authorized"] is True
