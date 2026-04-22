"""DFlash is not a generic speculative-decoding helper.

Provenance:
- Chen, Liang, and Liu, "DFlash: Block Diffusion for Flash Speculative
  Decoding", arXiv:2602.06036, https://arxiv.org/abs/2602.06036
- Official DFlash project, https://z-lab.ai/projects/dflash/

Paper-faithful DFlash support lives behind the explicit adapter contract in
``molt.gpu.dflash``. This module intentionally fails closed so generic
speculative helpers cannot be imported under the DFlash name.
"""

raise ImportError(
    "tinygrad.dflash is not available: DFlash requires a target-conditioned "
    "block-diffusion drafter/verifier adapter. Use molt.gpu.dflash for the "
    "paper-faithful adapter contract, or tinygrad.speculative for generic "
    "speculative-decoding helpers."
)
