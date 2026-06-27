use molt_obj_model::MoltObject;
use molt_runtime_core::obj_from_bits;
use molt_runtime_core::prelude::*;
use num_bigint::BigInt;
use num_traits::One;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crate::bridge::{
    alloc_bytes, alloc_dict_with_pairs, alloc_list_with_capacity, alloc_string, alloc_tuple,
    attr_name_bits_from_bytes, bytes_like_slice, call_callable0, call_callable1, call_callable2,
    call_class_init_with_args, clear_exception, dec_ref_bits, env_state_get, exception_kind_bits,
    exception_pending, inc_ref_bits, index_bigint_from_obj, int_bits_from_bigint, is_truthy,
    maybe_ptr_from_bits, missing_bits, molt_exception_last, molt_float_from_obj,
    molt_getattr_builtin, molt_is_callable, molt_iter, molt_iter_next, molt_list_insert,
    object_type_id, raise_exception, seq_vec_ref, string_obj_to_owned, to_f64, to_i64,
};

#[path = "functions_http/client_core.rs"]
mod client_core;
#[path = "functions_http/common.rs"]
mod common;
#[path = "functions_http/ctypes.rs"]
mod ctypes;
#[path = "functions_http/message_client_ffi.rs"]
mod message_client_ffi;
#[path = "functions_http/request_response_ffi.rs"]
mod request_response_ffi;
#[path = "functions_http/server_core.rs"]
mod server_core;
#[path = "functions_http/server_ffi.rs"]
mod server_ffi;
#[path = "functions_http/state.rs"]
mod state;
#[path = "functions_http/url_cookie_error_ffi.rs"]
mod url_cookie_error_ffi;
#[path = "functions_http/url_parse.rs"]
mod url_parse;

#[allow(unused_imports)]
use client_core::*;
#[allow(unused_imports)]
use common::*;
#[allow(unused_imports)]
use ctypes::*;
#[allow(unused_imports)]
use message_client_ffi::*;
#[allow(unused_imports)]
use request_response_ffi::*;
#[allow(unused_imports)]
use server_core::*;
#[allow(unused_imports)]
use server_ffi::*;
#[allow(unused_imports)]
use state::*;
#[allow(unused_imports)]
use url_cookie_error_ffi::*;
#[allow(unused_imports)]
use url_parse::*;

pub(crate) use crate::bridge::{alloc_string_bits, attr_optional};
pub use ctypes::*;
pub use message_client_ffi::*;
pub use request_response_ffi::*;
pub use server_ffi::*;
pub use url_cookie_error_ffi::*;
