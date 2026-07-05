use std::collections::{HashMap, HashSet};

use crate::tir::analysis::LoopForestResult;
use crate::tir::blocks::BlockId;

pub(super) struct LoopInfo {
    pub(super) preheader: Option<BlockId>,
    pub(super) body: HashSet<BlockId>,
}

pub(super) fn derive_loop_headers(
    loop_forest: &LoopForestResult,
    pred_map: &HashMap<BlockId, Vec<BlockId>>,
) -> HashMap<BlockId, LoopInfo> {
    let mut loop_headers = HashMap::new();

    for &header in &loop_forest.headers {
        let Some(body) = loop_forest.bodies.get(&header) else {
            continue;
        };
        let mut preheaders: Vec<BlockId> = pred_map
            .get(&header)
            .map(|preds| {
                preds
                    .iter()
                    .copied()
                    .filter(|pred| !body.contains(pred))
                    .collect()
            })
            .unwrap_or_default();
        preheaders.sort_unstable_by_key(|b| b.0);
        preheaders.dedup();
        let preheader = if preheaders.len() == 1 {
            Some(preheaders[0])
        } else {
            None
        };
        loop_headers.insert(
            header,
            LoopInfo {
                preheader,
                body: body.clone(),
            },
        );
    }

    loop_headers
}

pub(super) fn map_blocks_to_innermost_loop(
    loop_headers: &HashMap<BlockId, LoopInfo>,
) -> HashMap<BlockId, BlockId> {
    let mut block_to_header = HashMap::new();
    let mut ordered_loops: Vec<(BlockId, usize)> = loop_headers
        .iter()
        .map(|(&header, info)| (header, info.body.len()))
        .collect();
    ordered_loops.sort_unstable_by_key(|(header, body_len)| (*body_len, header.0));

    for (header, _) in ordered_loops {
        let Some(info) = loop_headers.get(&header) else {
            continue;
        };
        for &bid in &info.body {
            block_to_header.entry(bid).or_insert(header);
        }
    }

    block_to_header
}
