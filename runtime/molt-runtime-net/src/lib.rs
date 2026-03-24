//! molt-runtime-net: networking module group (socket, ssl, http, websocket)
//!
//! Extracted from molt-runtime to allow tree-shaking the networking stack
//! when not needed (e.g. WASM edge deploys without WASIX).

// TODO: migrate from molt-runtime/src/builtins/
// pub mod socket;
// pub mod ssl;
// pub mod http_client;
// pub mod ipaddress;
// pub mod select;
