"""Intrinsic-backed helpers for :mod:`statistics` (CPython 3.12+ surface)."""

from _intrinsics import require_intrinsic as _require_intrinsic

__all__ = ["_normal_dist_inv_cdf"]

_MOLT_STATISTICS_NORMAL_DIST_INV_CDF = _require_intrinsic(
    "molt_statistics_normal_dist_inv_cdf"
)


def _normal_dist_inv_cdf(
    p,
    mu,
    sigma,
    _normal_dist_inv_cdf_intrinsic=_MOLT_STATISTICS_NORMAL_DIST_INV_CDF,
):
    return float(_normal_dist_inv_cdf_intrinsic(p, mu, sigma))


del _MOLT_STATISTICS_NORMAL_DIST_INV_CDF
