use super::*;

#[path = "text/encoding.rs"]
mod encoding;
pub(super) use encoding::*;
#[path = "text/newline.rs"]
mod newline;
pub(super) use newline::*;
#[path = "text/backend.rs"]
mod backend;
pub(super) use backend::*;
#[path = "text/line_reader.rs"]
mod line_reader;
pub(super) use line_reader::*;
