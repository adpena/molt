# Molt experiments

This directory preserves non-shipping implementation experiments that are useful
as design evidence but must not masquerade as live runtime capability. Nothing in
this directory is compiled, packaged, or added to Molt module roots.

`runtime_string_repr.rs` is the retired Project TITAN string-layout exploration.
The live runtime continues to use its flat UTF-8 string storage authority.
