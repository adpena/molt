#[cfg(any(unix, test))]
use std::cmp::Reverse;
#[cfg(any(unix, test))]
use std::collections::{BinaryHeap, HashMap};
use std::env;
use std::fs::File;
#[cfg(unix)]
use std::io::BufRead;
use std::io::Write;
use std::io::{self, Read};
#[cfg(unix)]
use std::os::fd::AsRawFd;
#[cfg(all(feature = "native-backend", windows))]
use std::os::windows::io::AsRawHandle;
use std::path::Path;
#[cfg(feature = "native-backend")]
use std::path::PathBuf;
#[cfg(any(unix, test))]
use std::sync::Arc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[cfg(feature = "native-backend")]
use molt_backend::SimpleBackend;
use molt_backend::SimpleIR;
#[cfg(any(unix, test))]
use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};
#[cfg(feature = "wasm-backend")]
use molt_backend::{WasmBackend, WasmCompileOptions};
#[cfg(any(unix, test))]
use serde_json::Value as JsonValue;
#[cfg(feature = "native-backend")]
use sha2::{Digest, Sha256};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::Storage::FileSystem::{LOCKFILE_EXCLUSIVE_LOCK, LockFileEx, UnlockFileEx};
#[cfg(all(feature = "native-backend", windows))]
use windows_sys::Win32::System::IO::OVERLAPPED;

mod config;
mod daemon;
mod io_limits;
#[cfg(feature = "native-backend")]
mod native_batch;
#[cfg(feature = "native-backend")]
mod shared_stdlib_cache;

pub(crate) use config::*;
pub(crate) use daemon::*;
pub(crate) use io_limits::*;
#[cfg(feature = "native-backend")]
pub(crate) use native_batch::*;
#[cfg(feature = "native-backend")]
pub(crate) use shared_stdlib_cache::*;
