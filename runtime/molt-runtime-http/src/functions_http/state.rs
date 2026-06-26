use super::*;

// ---------------------------------------------------------------------------

pub(super) struct MoltUrllibResponse {
    pub(super) body: Vec<u8>,
    pub(super) pos: usize,
    pub(super) closed: bool,
    pub(super) url: String,
    pub(super) code: i64,
    pub(super) reason: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) header_joined: HashMap<String, String>,
    pub(super) headers_dict_cache: Option<u64>,
    pub(super) headers_list_cache: Option<u64>,
}

pub(super) struct UrllibHttpRequest {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) path: String,
    pub(super) method: String,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
    pub(super) timeout: Option<f64>,
    /// When `Some(server_name)`, the request is sent over TLS using rustls
    /// with the given SNI server name. Required for `https://` URLs.
    pub(super) tls_server_name: Option<String>,
}

#[derive(Clone)]
pub(super) struct MoltHttpClientConnection {
    pub(super) host: String,
    pub(super) port: u16,
    pub(super) timeout: Option<f64>,
    pub(super) method: Option<String>,
    pub(super) url: Option<String>,
    pub(super) headers: Vec<(String, String)>,
    pub(super) body: Vec<u8>,
    pub(super) buffer: Vec<Vec<u8>>,
    pub(super) skip_host: bool,
    pub(super) skip_accept_encoding: bool,
    /// Set when the connection was created via `molt_http_client_connection_new_https`
    /// (i.e. backing an `http.client.HTTPSConnection`). Causes request execution
    /// to negotiate TLS via rustls.
    pub(super) use_tls: bool,
}

pub(super) struct MoltHttpClientConnectionRuntime {
    pub(super) next_handle: u64,
    pub(super) connections: HashMap<u64, MoltHttpClientConnection>,
}

#[derive(Clone, Default)]
pub(super) struct MoltHttpMessage {
    pub(super) headers: Vec<(String, String)>,
    pub(super) index: HashMap<String, Vec<usize>>,
    pub(super) items_list_cache: Option<u64>,
}

pub(super) struct MoltHttpMessageRuntime {
    pub(super) next_handle: u64,
    pub(super) messages: HashMap<u64, MoltHttpMessage>,
}

#[derive(Clone)]
pub(super) struct MoltCookieEntry {
    pub(super) name: String,
    pub(super) value: String,
    pub(super) domain: String,
    pub(super) path: String,
}

#[derive(Clone, Default)]
pub(super) struct MoltCookieJar {
    pub(super) cookies: Vec<MoltCookieEntry>,
}

pub(super) struct MoltSocketServerPending {
    pub(super) request: Vec<u8>,
    pub(super) response: Option<Vec<u8>>,
}

pub(super) struct MoltSocketServerRuntime {
    pub(super) next_request_id: u64,
    pub(super) pending_by_server: HashMap<u64, VecDeque<u64>>,
    pub(super) pending_requests: HashMap<u64, MoltSocketServerPending>,
    pub(super) request_server: HashMap<u64, u64>,
    pub(super) closed_servers: HashSet<u64>,
}

pub(super) static URLLIB_RESPONSE_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltUrllibResponse>>> =
    OnceLock::new();
pub(super) static URLLIB_RESPONSE_NEXT: AtomicU64 = AtomicU64::new(1);
pub(super) static HTTP_CLIENT_CONNECTION_RUNTIME: OnceLock<Mutex<MoltHttpClientConnectionRuntime>> =
    OnceLock::new();
pub(super) static HTTP_MESSAGE_RUNTIME: OnceLock<Mutex<MoltHttpMessageRuntime>> = OnceLock::new();
pub(super) static COOKIEJAR_REGISTRY: OnceLock<Mutex<HashMap<u64, MoltCookieJar>>> =
    OnceLock::new();
pub(super) static COOKIEJAR_NEXT: AtomicU64 = AtomicU64::new(1);
pub(super) static SOCKETSERVER_RUNTIME: OnceLock<Mutex<MoltSocketServerRuntime>> = OnceLock::new();
