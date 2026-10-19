//! The compile daemon serves connections concurrently and retires on request:
//! it unlinks its socket, finishes the connections it accepted, and exits.
#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const PROTOCOL_VERSION: u32 = include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../src/molt/backend_daemon_protocol_version.txt"
));
const PATIENCE: Duration = Duration::from_secs(30);

struct Daemon {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Self {
        // Short: a Unix socket path must fit in about 100 bytes.
        let dir = std::env::temp_dir().join(format!("moltd-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create daemon directory");
        let socket = dir.join("d.sock");
        let child = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
            .arg("--daemon")
            .arg("--socket")
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("start daemon");
        let deadline = Instant::now() + PATIENCE;
        while UnixStream::connect(&socket).is_err() {
            assert!(Instant::now() < deadline, "daemon never listened");
            std::thread::sleep(Duration::from_millis(10));
        }
        Self { child, socket, dir }
    }

    fn connect(&self) -> Client {
        let stream = UnixStream::connect(&self.socket).expect("connect");
        stream
            .set_read_timeout(Some(PATIENCE))
            .expect("read timeout");
        Client {
            reader: BufReader::new(stream.try_clone().expect("clone stream")),
            stream,
        }
    }

    fn wait_for_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll daemon") {
                return status;
            }
            assert!(Instant::now() < deadline, "retired daemon never exited");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Daemon {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct Client {
    stream: UnixStream,
    reader: BufReader<UnixStream>,
}

impl Client {
    fn request(&mut self, request: serde_json::Value) -> serde_json::Value {
        let mut line = serde_json::to_vec(&request).expect("encode request");
        line.push(b'\n');
        self.stream.write_all(&line).expect("send request");
        let mut response = String::new();
        self.reader.read_line(&mut response).expect("read response");
        serde_json::from_str(&response).expect("decode response")
    }

    fn ping(&mut self) -> serde_json::Value {
        self.request(serde_json::json!({"version": PROTOCOL_VERSION, "ping": true}))
    }
}

#[test]
fn an_open_connection_does_not_block_other_clients() {
    let daemon = Daemon::start("concurrent");
    // One build holds a connection open without finishing its request.
    let _waiting_build = daemon.connect();
    let rejected = daemon.connect().request(serde_json::json!({
        "version": PROTOCOL_VERSION - 1, "ping": true,
    }));
    assert_eq!(rejected["ok"], false, "{rejected}");
    assert!(
        rejected["error"]
            .as_str()
            .unwrap()
            .contains("unsupported protocol version")
    );
    let response = daemon.connect().ping();
    assert_eq!(response["pong"], true, "{response}");
    assert_eq!(response["health"]["active_jobs"], 0, "{response}");
}

#[test]
fn retirement_unlinks_the_socket_finishes_open_connections_and_exits() {
    let mut daemon = Daemon::start("retire");
    let mut open_build = daemon.connect();

    let response = daemon
        .connect()
        .request(serde_json::json!({"version": PROTOCOL_VERSION, "retire": true}));
    assert_eq!(response["ok"], true, "{response}");
    assert_eq!(response["health"]["retiring"], true, "{response}");

    // A new build finds no socket and starts a fresh daemon.
    assert!(!daemon.socket.exists(), "retirement kept its socket");
    assert!(UnixStream::connect(&daemon.socket).is_err());

    // A connection the daemon already accepted is still served.
    let response = open_build.ping();
    assert_eq!(response["pong"], true, "{response}");
    assert_eq!(response["health"]["retiring"], true, "{response}");
    drop(open_build);

    let status = daemon.wait_for_exit();
    assert!(status.success(), "retired daemon exited with {status}");
}

/// A native compile request with its own settings.
fn native_compile_request(
    output: &std::path::Path,
    key: &str,
    env: serde_json::Value,
) -> serde_json::Value {
    let lease = output.with_extension("msgpack");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lease)
        .expect("create compile IR lease");
    rmp_serde::encode::write_named(
        &mut file,
        &serde_json::json!({
            "functions": [
                {"return_abi": "void", "name": "molt_main", "params": [], "ops": [
                    {"kind": "call", "s_value": "helper"}, {"kind": "ret_void"}
                ]},
                {"return_abi": "void", "name": "helper", "params": [], "ops": [{"kind": "ret_void"}]}
            ],
            "profile": null
        }),
    )
    .expect("encode compile IR lease");

    serde_json::json!({
        "version": PROTOCOL_VERSION,
        "config_digest": "daemon-server-test",
        "env": env,
        "jobs": [{
            "id": key,
            "is_wasm": false,
            "native_output_kind": "archive",
            "output": output.to_string_lossy(),
            "cache_key": key,
            "function_cache_key": format!("{key}-functions"),
            "ir_path": lease.to_string_lossy()
        }]
    })
}

#[test]
fn concurrent_compiles_run_under_their_own_request_settings() {
    // The daemon reads settings only from request scopes, so a compile phase
    // that missed its scope (a worker thread, a pool, a child) would panic
    // and fail its job.
    let daemon = Daemon::start("compile");
    let builds: Vec<_> = [
        ("module-a", serde_json::json!({"RAYON_NUM_THREADS": "2"})),
        (
            "module-b",
            serde_json::json!({"MOLT_DISABLE_RC_COALESCE": ""}),
        ),
    ]
    .into_iter()
    .map(|(key, env)| {
        let output = daemon.dir.join(format!("{key}.a"));
        let mut client = daemon.connect();
        std::thread::spawn(move || {
            let response = client.request(native_compile_request(&output, key, env));
            (response, output)
        })
    })
    .collect();
    for build in builds {
        let (response, output) = build.join().expect("build thread");
        assert_eq!(response["ok"], true, "{response}");
        assert_eq!(response["jobs"][0]["ok"], true, "{response}");
        assert!(output.exists(), "missing {}", output.display());
    }
}
