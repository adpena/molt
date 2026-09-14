//! Scalarize immediately unpacked, nonescaping tuple values into SSA copies.
//!
//! The retired iterator-fusion lane mistook individual ForIter results for
//! whole iterators and emitted unconsumed backend tags. Tuple scalarization is
//! independent and remains the only live optimization in this module.

mod tuple_scalarize;

#[cfg(test)]
mod tests;

pub use tuple_scalarize::run_tuple_scalarize;
