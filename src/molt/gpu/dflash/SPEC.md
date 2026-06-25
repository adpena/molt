# DFlash fidelity SPEC — the F1–F6 invariants (executable-spec anchor)

<!--
authority: this file is the machine-checkable transcription of the DFlash
algorithm's NON-NEGOTIABLE fidelity invariants. doc 67 (Tinygrad + DFlash
Fidelity) §3.5.1 / Phase 5b. It is the source-of-truth the Phase 5a algorithm
work (drafter forward pass + KV injection) is checked against, and the spec the
fail-closed corpus (`tests/gpu/dflash/test_dflash_fidelity.py`) gates today.

constitution: CLAUDE.md "Top Priority: Tinygrad + DFlash Fidelity" (turn-blocking):
  "Exact DFlash algorithmic fidelity is non-negotiable when implementing DFlash
   support. Do not ship generic speculative decoding under a DFlash label."
  "If a model lacks a real trained DFlash drafter, say so explicitly and do not
   fake support."

scope: this SPEC pins the invariants and the fail-closed contract. It does NOT
implement the drafter algorithm (Phase 5a). F1/F2/F4/F5 are stated as the
obligations the future drafter + reference model must satisfy; F3 and F6 are
already structurally enforced by `contracts.py`/`adapters.py` and are gated by
the fail-closed corpus now.
-->

## Provenance (pinned source of truth)

| field | value |
|---|---|
| **paper** | Chen, Liang, and Liu, "DFlash: Block Diffusion for Flash Speculative Decoding" |
| **arXiv** | `arXiv:2602.06036v2` — <https://arxiv.org/abs/2602.06036> |
| **official project** | DFlash, Z-Lab — <https://z-lab.ai/projects/dflash/> |
| **molt contract layer** | `src/molt/gpu/dflash/{contracts,adapters,runtime}.py` |
| **molt mislabel guard** | `src/molt/stdlib/tinygrad/dflash.py` (fails closed: `tinygrad.dflash` raises `ImportError`) |

### Primary-source verification (arXiv:2602.06036v2, refreshed 2026-06-25)

Verified against the ACTUAL paper (not molt prose), per the project's online-research fidelity rule:
- **Authors:** Jian Chen, Yesheng Liang, Zhijian Liu. **Version:** v2 (submitted 2026-02-05, revised 2026-05-28).
- **Performance (abstract, verbatim):** "DFlash achieves over 6x **lossless** acceleration ... delivering up to 2.5x higher speedup than the state-of-the-art speculative decoding method EAGLE-3."
- **Section structure:** 1 Introduction · 2 Related Work · 3 Preliminaries · 4 Method · 5 Experiments · 6 Conclusion.
- **Verified section mapping (these SUPERSEDE the first-pass inline §-refs below, which were sourced from molt prose and were off by a section):**
  - F1 target-conditioning · F2 KV-injection · F4 block-diffusion → **§4.1 (Method — Inference)**.
  - F4 last-verified-token / anchor conditioning · F6 trained-drafter → **§4.2 (Method — Training)**.
  - F3 drafter/verifier separation · F5 speculative verification → **§3.1 (Preliminaries)**.
- **F2 core mechanism (§4.1, verbatim):** "we treat the fused target context feature as persistent contextual information and directly inject it into the Key and Value projections of every draft model layer."
- **F1 (§4.1, verbatim):** "the hidden representations of large autoregressive target models encode substantially more information than token-level logits."
- **F4 (§4.1, verbatim):** "DFlash predicts the next token block using a block-level diffusion process. All masked positions within a block are decoded in parallel in a single forward pass."
- **F5 caveat (honest):** the paper asserts losslessness as the standard accept-on-target-match speculative-decoding property; it states NO separate formal `output == greedy-decode` theorem. F5 transcribes that standard guarantee as the obligation the verification protocol must satisfy — it is not quoted as a verbatim paper theorem.

### Current ecosystem evidence (refreshed 2026-06-25)

This section records live implementation and follow-on-paper evidence so Molt's
DFlash contract stays aligned with the public source of truth instead of an
older prose snapshot.

- **Official Z-Lab implementation/model registry:** <https://github.com/z-lab/dflash>
  and <https://huggingface.co/collections/z-lab/dflash> publish model-paired
  DFlash draft artifacts. Public usage examples pass a target model plus a
  specific draft model id, for example vLLM `{"method":"dflash","model":...}`.
- **vLLM/speculators training path:** <https://docs.vllm.ai/projects/speculators/en/latest/user_guide/algorithms/dflash/>
  documents DFlash as a small diffusion-LLM draft model conditioned on target
  hidden states, with block-size, max-anchor, and target-layer-id training
  parameters.
- **vLLM runtime path:** <https://docs.vllm.ai/en/v0.23.0/api/vllm/v1/spec_decode/dflash/>
  exposes a `DFlashProposer` whose key differences include one draft forward
  pass, unpadded context states, query-only spec tokens, pre-inserted context
  KVs, and non-causal attention requirements.
- **SGLang/Modal/Z-Lab implementation work:** <https://www.lmsys.org/blog/2026-06-15-next-generation-speculative-decoding-dflash-v2/>
  emphasizes the same two base mechanisms Molt gates here: block-parallel
  diffusion drafting and target-feature/KV injection.
- **Follow-on papers are related, not interchangeable:** DDTree
  (<https://arxiv.org/abs/2604.12989>) builds draft trees from one-pass DFlash
  marginals; DFlare (<https://arxiv.org/abs/2606.02091>) changes the DFlash
  conditioning bottleneck with layer-wise fusion. Neither should be silently
  labeled "base DFlash" by a Molt adapter without explicit provenance.

This provenance is the same revision cited by `src/molt/gpu/dflash/contracts.py`
(module docstring) and `src/molt/stdlib/tinygrad/dflash.py`. DFlash is defined by
that source as **target-conditioned block-diffusion drafting**: target hidden
features are fused and injected into each draft layer's KV cache; drafting is
conditioned on that target context and the last verified token, and the verified
output stream is lossless against greedy target decoding. The invariants below
transcribe those mechanisms as named, individually-checkable obligations.

The distinction this SPEC exists to make unexpressible: **generic speculative
decoding** (an independent draft model proposing tokens with no target
conditioning) is NOT DFlash. Any path that produces draft tokens without
consuming `target_features`/`target_kv`/`position_ids`/`last_verified_token` is
generic speculative decoding and MUST NOT be labeled, imported, resolved, or run
under the DFlash name.

---

## The fidelity invariants

Each invariant has: a name (the obligation id), the citation it transcribes, the
precise checkable assertion, and its current gating status. An invariant marked
`fail-closed: GATED` is exercised by `tests/gpu/dflash/test_dflash_fidelity.py`
today. An invariant marked `algorithm: PENDING Phase 5a` is the obligation the
future drafter + `tests/gpu/dflash/reference/` model must satisfy; it is named
here so the algorithm cannot be declared done without satisfying it.

### F1 — `target_conditioning`: the drafter CONSUMES target features

- **Cite:** DFlash §3 (target-conditioned drafting); project page "hidden-feature
  conditioning". Paper mechanism: the drafter's forward pass fuses the target
  model's hidden features rather than running as an independent model.
- **Invariant:** the drafter forward pass must *consume* `DFlashConditioning.target_features`
  (hidden-feature fusion) — not merely receive and discard it. A drafter whose
  output is invariant to `target_features` is generic speculative decoding, not
  DFlash, and violates F1.
- **Checkable assertion (algorithm):** perturbing `target_features` must change
  the produced draft block for a fixed prefix/seed (the drafter is a function of
  the target context). Reference-model gradient/sensitivity check under
  `tests/gpu/dflash/reference/`.
- **Contract anchor (today):** `DFlashConditioning` requires non-`None`
  `target_features` (`contracts.py:56-57`); `require_dflash_conditioning`
  re-checks it at every boundary (`contracts.py:80-81`).
- **Status:** contract: GATED (fail-closed corpus). algorithm: PENDING Phase 5a.

### F2 — `kv_injection`: target features are INJECTED into each draft layer's KV cache

- **Cite:** DFlash §3 / §4 (KV injection); the paper's core efficiency mechanism —
  target hidden features are injected into the draft layers' KV cache so the
  drafter shares the target's context without recomputing it.
- **Invariant:** target hidden features must be injected into each draft layer's
  KV cache (`DFlashConditioning.target_kv` plus the fused `target_features`),
  verifiable by a KV-state assertion: after a draft step, the draft layers' KV
  state must reflect the injected target features, not a freshly-recomputed
  drafter-only KV.
- **Checkable assertion (algorithm):** a KV-state assertion on the reference
  drafter — the post-injection KV tensors equal the target-derived KV at the
  injected positions (bit/ULP-bounded per `docs/spec/tinygrad_pin.md` policy).
- **Contract anchor (today):** `DFlashConditioning` requires non-`None`
  `target_kv` (`contracts.py:58-59`); re-checked by `require_dflash_conditioning`
  (`contracts.py:82-83`).
- **Status:** contract: GATED. algorithm: PENDING Phase 5a.

### F3 — `verifier_drafter_separation`: target verification and draft logic are distinct, target-owns-conditioning

- **Cite:** DFlash §2/§3 (speculative-decoding structure: separate drafter and
  verifier); target-owned conditioning is the paper's invariant that the target
  model produces the conditioning the drafter consumes.
- **Invariant:** the verification logic (target model) and the draft logic
  (drafter) are distinct callables, and the conditioning is target-owned (refreshed
  by the verifier, consumed by the drafter). They may not be collapsed into one
  autoregressive path.
- **Checkable assertion (today):** `DFlashRuntime` requires *two distinct
  callables* `draft_step` and `verify_step`, each validated `callable`, and
  rejects the same callable supplied for both roles (`contracts.py`); the
  conditioned decode loop re-validates DFlash
  conditioning on every verifier-refreshed conditioning when the initial
  conditioning is DFlash (`runtime.py:209-215`). A non-callable for either, or a
  generic (non-`DFlashConditioning`) conditioning, raises.
- **Status:** STRUCTURALLY ENFORCED + GATED (fail-closed corpus asserts the
  non-callable and non-DFlash-conditioning rejections).

### F4 — `block_diffusion`: drafting produces a BLOCK via block-diffusion, conditioned on the last verified token

- **Cite:** DFlash title + §3 (block diffusion); the paper drafts a *block* of
  candidate tokens per step via a block-diffusion process — not autoregressive
  single-token drafting — conditioned on the last verified token.
- **Invariant:** a draft step produces a block of candidate tokens (size up to
  `max_block_size`) via the block-diffusion process, conditioned on
  `DFlashConditioning.last_verified_token`. Single-token-at-a-time autoregressive
  drafting is generic speculative decoding, not DFlash block diffusion.
- **Checkable assertion (algorithm):** the reference drafter emits a block of the
  requested size (1..`max_block_size`) per step; the block is a function of
  `last_verified_token` (perturbing it changes the block).
- **Contract anchor (today):** `DFlashConditioning` requires an integer
  `last_verified_token` (rejects `bool`; rejects non-integral)
  (`contracts.py:62-66`); `SpeculativeDraftRequest` carries `max_block_size`
  (`contracts.py:91-105`); the decode loop enforces
  `1 <= len(drafted) <= request_size` (`runtime.py:187-191`).
- **Status:** contract: GATED. algorithm: PENDING Phase 5a.

### F5 — `losslessness`: the verified output equals greedy target decoding

- **Cite:** DFlash §2 (speculative-decoding correctness guarantee); the verified
  token stream must equal the tokens greedy target-model decoding would produce —
  speculation changes *speed*, never *output*.
- **Invariant:** for any input, the token stream emitted by the DFlash
  verifier/drafter loop equals the stream produced by greedy decoding of the
  target model alone. Acceptance of a draft token is conditioned on it matching the
  target's argmax; on mismatch the target token is emitted and the block is
  re-anchored.
- **Checkable assertion (algorithm):** run the reference target model in pure
  greedy mode and in DFlash speculative mode; assert the emitted token streams are
  identical (exact integer-token equality).
- **Contract anchor (today):** `speculative_decode_greedy_conditioned` enforces the
  lossless verification protocol — `verify_step` must return `len(drafted)+1`
  target tokens (`runtime.py:205-208`); a draft token is accepted only when it
  equals the target token, else mismatch re-anchors (`runtime.py:217-241`).
- **Status:** protocol: ENFORCED (loop structure). end-to-end equality: PENDING
  Phase 5a reference model.

### F6 — `trained_drafter_required`: no DFlash runtime from an untrained/absent drafter (typed fail-closed)

- **Cite:** DFlash §3 (the drafter is a *trained* block-diffusion model conditioned
  on the target); CLAUDE.md mandate: "If a model lacks a real trained DFlash
  drafter, say so explicitly and do not fake support."
- **Invariant:** a `DFlashRuntime` cannot be constructed or resolved from an
  untrained or absent drafter. "No trained drafter present" is a first-class,
  typed, fail-closed state — never a silent fallback to a generic/faked drafter.
- **Checkable assertion (today):**
  - constructing `DFlashRuntime` with non-callable `draft_step`/`verify_step`
    raises `TypeError`, and constructing it with the same callable for both
    roles raises `TypeError`;
  - constructing `DFlashRuntime` / calling `require_dflash_conditioning` with a
    generic (non-`DFlashConditioning`) or incomplete conditioning raises
    `TypeError`/`ValueError` (`contracts.py:77-88`, `:157`);
  - resolving an adapter that is absent / unsupported returns `None`, and a
    *named* unavailable adapter raises `LookupError`
    (`adapters.py:97-141`, `:167-170`) — never a generic fallback runtime;
  - `import tinygrad.dflash` raises `ImportError` pointing at `molt.gpu.dflash`
    (`src/molt/stdlib/tinygrad/dflash.py:13-18`) — a generic helper cannot be
    imported under the DFlash name.
- **Status:** STRUCTURALLY ENFORCED + GATED (fail-closed corpus exercises every
  one of these typed refusals).

### F7 — `model_pair_provenance`: every adapter declares its target/draft pair and source

- **Cite:** the official Z-Lab model registry and current vLLM/SGLang usage all
  configure DFlash by pairing a target verifier model with a specific DFlash
  draft artifact. DFlash is not an anonymous "speculative mode".
- **Invariant:** a `DFlashAdapterSpec` must declare the exact target model id,
  draft model id, and provenance/source string for the adapter. Empty or
  non-string metadata is rejected before registration.
- **Checkable assertion (today):** `DFlashAdapterSpec` validates non-empty
  `target_model_id`, `draft_model_id`, and `provenance`; the fail-closed corpus
  asserts the typed refusals. Test fixtures use `test://...` ids and explicit
  test-only provenance so they cannot be mistaken for production model support.
- **Status:** STRUCTURALLY ENFORCED + GATED.

---

## Fail-closed contract summary (the part GATED today)

The following typed refusals are the executable encoding of "generic speculative
decoding mislabeled DFlash is unexpressible". They are asserted, with the exact
typed error, by `tests/gpu/dflash/test_dflash_fidelity.py`:

| guard | trigger | typed refusal (exact) | source |
|---|---|---|---|
| missing `target_features` | `DFlashConditioning(target_features=None, …)` | `ValueError("DFlashConditioning requires target_features")` | `contracts.py:56-57` |
| missing `target_kv` | `DFlashConditioning(target_kv=None, …)` | `ValueError("DFlashConditioning requires target_kv")` | `contracts.py:58-59` |
| missing `position_ids` | `DFlashConditioning(position_ids=None, …)` | `ValueError("DFlashConditioning requires position_ids")` | `contracts.py:60-61` |
| non-integer `last_verified_token` (bool) | `DFlashConditioning(last_verified_token=True, …)` | `TypeError("last_verified_token must be an integer token id")` | `contracts.py:62-63` |
| non-integral `last_verified_token` | `DFlashConditioning(last_verified_token=1.5, …)` | `TypeError("last_verified_token must be an integer token id")` | `contracts.py:64-66` |
| generic conditioning at boundary | `require_dflash_conditioning(SpeculativeConditioning(...))` | `TypeError("… must be DFlashConditioning")` | `contracts.py:78-79` |
| non-callable `draft_step` | `DFlashRuntime(draft_step=object(), …)` | `TypeError("DFlashRuntime draft_step must be callable")` | `contracts.py:153-154` |
| non-callable `verify_step` | `DFlashRuntime(verify_step=object(), …)` | `TypeError("DFlashRuntime verify_step must be callable")` | `contracts.py:155-156` |
| collapsed drafter/verifier | `DFlashRuntime(draft_step=f, verify_step=f, …)` | `TypeError("DFlashRuntime draft_step and verify_step must be distinct callables")` | `contracts.py` |
| generic conditioning into runtime | `DFlashRuntime(initial_conditioning=SpeculativeConditioning(...))` | `TypeError("initial_conditioning must be DFlashConditioning")` | `contracts.py:157` → `:78-79` |
| anonymous adapter target | `DFlashAdapterSpec(target_model_id="", …)` | `ValueError("dflash adapter target_model_id must be non-empty")` | `adapters.py` |
| anonymous adapter draft | `DFlashAdapterSpec(draft_model_id="   ", …)` | `ValueError("dflash adapter draft_model_id must be non-empty")` | `adapters.py` |
| missing adapter provenance | `DFlashAdapterSpec(provenance=None, …)` | `TypeError("dflash adapter provenance must be a string")` | `adapters.py` |
| non-spec adapter registration | `register_dflash_adapter(object())` | `TypeError("register_dflash_adapter expects DFlashAdapterSpec")` | `adapters.py:46-47` |
| generic adapter returns non-runtime | adapter `create_runtime` returns a non-`DFlashRuntime` | `TypeError("dflash adapter create_runtime() must return DFlashRuntime")` | `adapters.py:139-140` |
| named adapter unavailable | `build_dflash_runtime(…, dflash_adapter="x")` with no supporting adapter | `LookupError("dflash adapter 'x' is unavailable for this context")` | `adapters.py:167-170` |
| mislabel guard | `import tinygrad.dflash` | `ImportError("tinygrad.dflash is not available: …")` (points at `molt.gpu.dflash`, names `tinygrad.speculative` as the generic alternative) | `src/molt/stdlib/tinygrad/dflash.py:13-18` |

## Status legend

- **STRUCTURALLY ENFORCED + GATED** — the contract code enforces it AND the
  fail-closed corpus asserts the typed refusal. Drift would turn the corpus RED.
- **contract: GATED / algorithm: PENDING Phase 5a** — the *transport contract*
  (the conditioning field must be present) is gated today; the *consumption*
  (the drafter actually using it) is the Phase 5a obligation, checked by the
  future `tests/gpu/dflash/reference/` model. Phase 5a is build-heavy (compiled
  reference binary via `molt build`, run under `tools/safe_run.py`) and depends on
  the `gpu_op_contract` primitives (doc 67 Phase 1, LANDED).

## The gate

`pytest tests/gpu/dflash/` green (fail-closed corpus today; F1–F6 fidelity once
Phase 5a + the reference model land) is **release-blocking** for any change
touching `src/molt/gpu/dflash/**` or the `tinygrad` speculative modules
(`speculative`, `eagle`, `mirror_sd`, `tree_attention`, `flash_attention`,
`kv_cache`). A weakened guard turns the corpus RED — that is the mechanism that
makes "generic speculative decoding mislabeled DFlash" unexpressible.
