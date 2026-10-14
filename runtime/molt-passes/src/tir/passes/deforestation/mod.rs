//! Scalarize immediately unpacked, nonescaping tuple values into SSA copies.
//!
//! The retired iterator-fusion lane mistook individual ForIter results for
//! whole iterators and emitted unconsumed backend tags. Tuple scalarization is
//! independent. Split-field read scalarization runs in the terminal lowering
//! phase, before lifetime insertion, using this same typed authority.

mod split_field;
mod tuple_scalarize;

#[cfg(test)]
mod tests;

pub use split_field::run_split_field_reads;
pub use tuple_scalarize::run_tuple_scalarize;
