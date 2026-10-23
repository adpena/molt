//! Machine linkage carriers of each function body, reused by content.
//!
//! A carrier row (the parameter carriers and the inferred return type) is a
//! pure function of one `FunctionIR` contract and the backend code: TIR
//! lowering, value ranges and representation facts read nothing else. Every
//! native build lowered every defined body to TIR to freeze these rows, the
//! unchanged stdlib included: 5.6 s on one thread for a 91-line asyncio
//! program (3,612 functions, 3,592 of them stdlib; 0.24 s when reused).
//!
//! Rows live in the backend binary's content-addressed TIR cache namespace
//! (`tir::cache`), keyed by the function's contract digest, as Go's build
//! cache serves an unchanged package by its action ID. An unchanged body costs
//! one digest and one verified read; a changed body has a new key. Compilation
//! diagnostics bypass reuse, as they bypass every other backend cache.

use super::*;
use crate::tir::cache::{CompilationCache, CompilationCacheWriteError};
use crate::tir::pipeline_cache::request_parallel_map;
use molt_ir::backend_environment as backend_env;

const KEY_DOMAIN: &[u8] = b"molt-native-linkage-carriers-key-v1";
const MAGIC: &[u8] = b"MOLT:NATIVE-LINKAGE-CARRIERS:v1\0";

/// The TIR-derived half of a `NativeFunctionLinkageAbi`. The source signature
/// and parameter custody come from the `FunctionIR` itself on every build.
#[derive(Clone, Debug, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub(in crate::native_backend::simple_backend) struct LinkageCarriers {
    pub(in crate::native_backend::simple_backend) param_types: Vec<crate::tir::types::TirType>,
    pub(in crate::native_backend::simple_backend) return_type: crate::tir::types::TirType,
}

pub(in crate::native_backend::simple_backend) struct LinkageCarrierRows {
    /// One row per input function, in input order.
    pub(in crate::native_backend::simple_backend) rows: Vec<LinkageCarriers>,
    /// Rows served by the cache rather than computed.
    pub(in crate::native_backend::simple_backend) reused: usize,
}

impl LinkageCarriers {
    fn compute(function: &FunctionIR) -> Self {
        let tir = crate::tir::lower_from_simple::lower_to_tir(function);
        Self {
            param_types: crate::representation_plan::native_linkage_param_types(&tir),
            return_type: tir.return_type,
        }
    }

    fn encode(&self) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        serde_json::to_writer(&mut bytes, self).expect("linkage carrier rows serialize");
        bytes
    }

    fn decode(bytes: &[u8]) -> Option<Self> {
        serde_json::from_slice(bytes.strip_prefix(MAGIC)?).ok()
    }
}

fn cache_key(function: &FunctionIR) -> String {
    let mut key = molt_ir::content_digest::CompilationCacheKey::new(KEY_DOMAIN);
    key.digest_field(b"function-ir-contract", |writer| {
        molt_ir::ir::write_function_ir_contract(function, writer)
    })
    .expect("digest-backed FunctionIR contract serialization cannot fail");
    key.finish_hex()
}

/// Carrier rows for `functions`: verified cache rows for unchanged bodies,
/// rows computed on the request's compiler pool (and published) for the rest.
pub(in crate::native_backend::simple_backend) fn linkage_carriers(
    functions: &[&FunctionIR],
    cache: Option<&mut CompilationCache>,
) -> LinkageCarrierRows {
    let cache = cache.filter(|_| !backend_env::compilation_diagnostics_requested());
    let Some(cache) = cache else {
        return LinkageCarrierRows {
            rows: request_parallel_map(functions, |function| LinkageCarriers::compute(function)),
            reused: 0,
        };
    };
    let keys = request_parallel_map(functions, |function| cache_key(function));
    let mut rows: Vec<Option<LinkageCarriers>> = keys
        .iter()
        .map(|key| {
            cache
                .get(key)
                .and_then(|bytes| LinkageCarriers::decode(&bytes))
        })
        .collect();
    let misses: Vec<usize> = (0..rows.len())
        .filter(|index| rows[*index].is_none())
        .collect();
    let reused = rows.len() - misses.len();
    let computed =
        request_parallel_map(&misses, |index| LinkageCarriers::compute(functions[*index]));
    for (index, row) in misses.into_iter().zip(computed) {
        match cache.put(&keys[index], &row.encode()) {
            Ok(()) => {}
            Err(CompilationCacheWriteError::Integrity(error)) => panic!(
                "persistent TIR cache rejected linkage carriers for '{}': {error}",
                functions[index].name
            ),
            Err(CompilationCacheWriteError::Unavailable(error)) => eprintln!(
                "MOLT_CACHE: linkage carrier cache unavailable for '{}': {error}",
                functions[index].name
            ),
        }
        rows[index] = Some(row);
    }
    if let Err(error) = cache.trim_if_due() {
        eprintln!("MOLT_CACHE: persistent TIR cache trim failed: {error}");
    }
    LinkageCarrierRows {
        rows: rows
            .into_iter()
            .map(|row| row.expect("every function has a carrier row"))
            .collect(),
        reused,
    }
}
