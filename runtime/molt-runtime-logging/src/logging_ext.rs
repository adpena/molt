//! Intrinsics for the `logging` stdlib module.
//!
//! Implements handler and logger handles and level utilities for `logging`.
//! Python LogRecord and Formatter objects own mutable record and format state.
//!
//! Handle registries are scoped to the owning Molt `RuntimeState` through the
//! runtime extension-state bridge, so nested runtimes do not share or leak
//! logging state.

use crate::bridge::{alloc_string, raise_exception, string_obj_to_owned, to_i64};
use molt_runtime_core::prelude::*;
use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, Ordering};

// ── Standard logging levels (CPython) ────────────────────────────────────────

const CRITICAL: i64 = 50;
const ERROR: i64 = 40;
const WARNING: i64 = 30;
const INFO: i64 = 20;
const DEBUG: i64 = 10;
const NOTSET: i64 = 0;

// ── Handle counters ──────────────────────────────────────────────────────────

fn next_handler_handle() -> i64 {
    logging_state()
        .next_handler_handle
        .fetch_add(1, Ordering::Relaxed)
}

fn next_logger_handle() -> i64 {
    logging_state()
        .next_logger_handle
        .fetch_add(1, Ordering::Relaxed)
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn return_str(_py: &PyToken, s: &str) -> u64 {
    let ptr = alloc_string(_py, s.as_bytes());
    if ptr.is_null() {
        return raise_exception::<u64>(_py, BuiltinExc::MemoryError, "out of memory");
    }
    MoltObject::from_ptr(ptr).bits()
}

fn handle_from_bits(_py: &PyToken, handle_bits: u64, kind: &str) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let msg = format!("{kind} handle must be an int");
        let _ = raise_exception::<u64>(_py, BuiltinExc::TypeError, &msg);
        return None;
    };
    Some(id)
}

// ── Level name mapping ───────────────────────────────────────────────────────

struct LevelNames {
    level_to_name: HashMap<i64, String>,
    name_to_level: HashMap<String, i64>,
}

impl LevelNames {
    fn new() -> Self {
        let mut level_to_name = HashMap::new();
        let mut name_to_level = HashMap::new();
        let defaults = [
            (CRITICAL, "CRITICAL"),
            (ERROR, "ERROR"),
            (WARNING, "WARNING"),
            (INFO, "INFO"),
            (DEBUG, "DEBUG"),
            (NOTSET, "NOTSET"),
        ];
        for &(level, name) in &defaults {
            level_to_name.insert(level, name.to_string());
            name_to_level.insert(name.to_string(), level);
        }
        // CPython also maps "FATAL" -> CRITICAL and "WARN" -> WARNING.
        name_to_level.insert("FATAL".to_string(), CRITICAL);
        name_to_level.insert("WARN".to_string(), WARNING);
        Self {
            level_to_name,
            name_to_level,
        }
    }

    fn get_name(&self, level: i64) -> String {
        self.level_to_name
            .get(&level)
            .cloned()
            .unwrap_or_else(|| format!("Level {level}"))
    }

    fn get_level(&self, name: &str) -> Option<i64> {
        self.name_to_level.get(name).copied()
    }

    fn add(&mut self, level: i64, name: String) {
        self.level_to_name.insert(level, name.clone());
        self.name_to_level.insert(name, level);
    }
}

// ── Handler ──────────────────────────────────────────────────────────────────

struct HandlerState {
    level: i64,
    closed: bool,
}

impl HandlerState {
    fn new_base(level: i64) -> Self {
        Self {
            level,
            closed: false,
        }
    }
}

// ── Handler intrinsics ───────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_new(level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let handler = HandlerState::new_base(level);
        let id = next_handler_handle();
        logging_state()
            .handler_registry
            .lock()
            .unwrap()
            .insert(id, handler);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_set_level(handler_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let mut registry = logging_state().handler_registry.lock().unwrap();
        if let Some(handler) = registry.get_mut(&h_id) {
            handler.level = level;
        } else {
            return raise_exception::<u64>(_py, BuiltinExc::ValueError, "invalid Handler handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_flush(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let registry = logging_state().handler_registry.lock().unwrap();
        if !registry.contains_key(&h_id) {
            return raise_exception::<u64>(_py, BuiltinExc::ValueError, "invalid Handler handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_close(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let mut registry = logging_state().handler_registry.lock().unwrap();
        if let Some(handler) = registry.get_mut(&h_id) {
            handler.closed = true;
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_handler_drop(handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(h_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        logging_state()
            .handler_registry
            .lock()
            .unwrap()
            .remove(&h_id);
        MoltObject::none().bits()
    })
}

// ── Logger ───────────────────────────────────────────────────────────────────

struct LoggerState {
    level: i64,
    handlers: Vec<i64>,
    disabled: bool,
    parent: Option<i64>,
}

impl LoggerState {
    fn new(level: i64) -> Self {
        Self {
            level,
            handlers: Vec::new(),
            disabled: false,
            parent: None,
        }
    }

    fn get_effective_level(&self, registry: &HashMap<i64, LoggerState>) -> i64 {
        if self.level != NOTSET {
            return self.level;
        }
        if let Some(parent_id) = self.parent
            && let Some(parent) = registry.get(&parent_id)
        {
            return parent.get_effective_level(registry);
        }
        NOTSET
    }

    fn is_enabled_for(&self, level: i64, registry: &HashMap<i64, LoggerState>) -> bool {
        if self.disabled {
            return false;
        }
        level >= self.get_effective_level(registry)
    }
}

struct LoggingRuntimeState {
    next_handler_handle: AtomicI64,
    next_logger_handle: AtomicI64,
    level_names: Mutex<LevelNames>,
    handler_registry: Mutex<HashMap<i64, HandlerState>>,
    logger_registry: Mutex<HashMap<i64, LoggerState>>,
    logger_cache: Mutex<HashMap<String, i64>>,
}

impl LoggingRuntimeState {
    fn new() -> Self {
        Self {
            next_handler_handle: AtomicI64::new(1),
            next_logger_handle: AtomicI64::new(1),
            level_names: Mutex::new(LevelNames::new()),
            handler_registry: Mutex::new(HashMap::new()),
            logger_registry: Mutex::new(HashMap::new()),
            logger_cache: Mutex::new(HashMap::new()),
        }
    }

    fn clear(&self) {
        *self.level_names.lock().unwrap() = LevelNames::new();
        self.handler_registry.lock().unwrap().clear();
        self.logger_registry.lock().unwrap().clear();
        self.logger_cache.lock().unwrap().clear();
    }
}

unsafe extern "C" fn logging_runtime_state_init() -> *mut u8 {
    Box::into_raw(Box::new(LoggingRuntimeState::new())) as *mut u8
}

unsafe extern "C" fn logging_runtime_state_clear(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        (&*(ptr as *const LoggingRuntimeState)).clear();
    }
}

unsafe extern "C" fn logging_runtime_state_drop(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    unsafe {
        drop(Box::from_raw(ptr as *mut LoggingRuntimeState));
    }
}

fn logging_state() -> &'static LoggingRuntimeState {
    let ptr = crate::bridge::runtime_state_get_or_init(
        b"molt-runtime-logging/v1",
        logging_runtime_state_init,
        logging_runtime_state_clear,
        logging_runtime_state_drop,
    );
    assert!(
        !ptr.is_null(),
        "molt logging runtime state initialization failed"
    );
    unsafe { &*(ptr as *const LoggingRuntimeState) }
}

// ── Logger intrinsics ────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_new(name_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        if string_obj_to_owned(obj_from_bits(name_bits)).is_none() {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "Logger name must be str");
        }
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let logger = LoggerState::new(level);
        let id = next_logger_handle();
        logging_state()
            .logger_registry
            .lock()
            .unwrap()
            .insert(id, logger);
        MoltObject::from_int(id).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_set_level(handle_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let mut registry = logging_state().logger_registry.lock().unwrap();
        if let Some(logger) = registry.get_mut(&id) {
            logger.level = level;
        } else {
            return raise_exception::<u64>(_py, BuiltinExc::ValueError, "invalid Logger handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_add_handler(logger_bits: u64, handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let Some(handler_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let mut registry = logging_state().logger_registry.lock().unwrap();
        if let Some(logger) = registry.get_mut(&logger_id) {
            if !logger.handlers.contains(&handler_id) {
                logger.handlers.push(handler_id);
            }
        } else {
            return raise_exception::<u64>(_py, BuiltinExc::ValueError, "invalid Logger handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_remove_handler(logger_bits: u64, handler_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        let Some(handler_id) = handle_from_bits(_py, handler_bits, "Handler") else {
            return MoltObject::none().bits();
        };
        let mut registry = logging_state().logger_registry.lock().unwrap();
        if let Some(logger) = registry.get_mut(&logger_id) {
            logger.handlers.retain(|&h| h != handler_id);
        } else {
            return raise_exception::<u64>(_py, BuiltinExc::ValueError, "invalid Logger handle");
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_is_enabled_for(logger_bits: u64, level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::from_bool(false).bits();
        };
        let level = to_i64(obj_from_bits(level_bits)).unwrap_or(NOTSET);
        let registry = logging_state().logger_registry.lock().unwrap();
        let Some(logger) = registry.get(&logger_id) else {
            return MoltObject::from_bool(false).bits();
        };
        let enabled = logger.is_enabled_for(level, &registry);
        MoltObject::from_bool(enabled).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_get_effective_level(logger_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(logger_id) = handle_from_bits(_py, logger_bits, "Logger") else {
            return MoltObject::from_int(NOTSET).bits();
        };
        let registry = logging_state().logger_registry.lock().unwrap();
        let Some(logger) = registry.get(&logger_id) else {
            return MoltObject::from_int(NOTSET).bits();
        };
        let effective = logger.get_effective_level(&registry);
        MoltObject::from_int(effective).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_logger_drop(handle_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(id) = handle_from_bits(_py, handle_bits, "Logger") else {
            return MoltObject::none().bits();
        };
        logging_state().logger_registry.lock().unwrap().remove(&id);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_shutdown() -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        for handler in logging_state()
            .handler_registry
            .lock()
            .unwrap()
            .values_mut()
        {
            handler.closed = true;
        }
        MoltObject::none().bits()
    })
}

// ── Level utilities ──────────────────────────────────────────────────────────

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_get_level_name(level_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(level) = to_i64(obj_from_bits(level_bits)) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "level must be an int");
        };
        let name = logging_state().level_names.lock().unwrap().get_name(level);
        return_str(_py, &name)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_add_level_name(level_bits: u64, name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(level) = to_i64(obj_from_bits(level_bits)) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "level must be an int");
        };
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "level name must be str");
        };
        logging_state().level_names.lock().unwrap().add(level, name);
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_logging_level_to_int(name_bits: u64) -> u64 {
    molt_runtime_core::with_core_gil!(_py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            return raise_exception::<u64>(_py, BuiltinExc::TypeError, "level name must be str");
        };
        let level_names = logging_state().level_names.lock().unwrap();
        if let Some(level) = level_names.get_level(&name) {
            MoltObject::from_int(level).bits()
        } else {
            // CPython returns the string itself when unknown; in the intrinsic
            // layer we raise ValueError for type safety.
            let msg = format!("Unknown level: '{name}'");
            raise_exception::<u64>(_py, BuiltinExc::ValueError, &msg)
        }
    })
}
