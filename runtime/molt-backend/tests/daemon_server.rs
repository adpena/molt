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
/// The first launch of a freshly built backend binary can wait on macOS's scan
/// of the new executable for tens of seconds under load; a daemon that dies
/// fails at once below, so a long start deadline costs a passing run nothing.
const START_PATIENCE: Duration = Duration::from_secs(300);

struct Daemon {
    child: Child,
    socket: PathBuf,
    dir: PathBuf,
}

impl Daemon {
    fn start(name: &str) -> Self {
        // A Unix socket path holds at most 104 bytes on macOS, and a guarded
        // run's TMPDIR can be longer: like src/molt/backend_daemon_paths.py,
        // fall back to /tmp when the path would not fit.
        let leaf = format!("moltd-{name}-{}", std::process::id());
        let mut dir = std::env::temp_dir().join(&leaf);
        if dir.join("d.sock").as_os_str().len() >= 104 {
            dir = PathBuf::from("/tmp").join(&leaf);
        }
        std::fs::create_dir_all(&dir).expect("create daemon directory");
        let socket = dir.join("d.sock");
        let mut child = Command::new(env!("CARGO_BIN_EXE_molt-backend"))
            .arg("--daemon")
            .arg("--socket")
            .arg(&socket)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .expect("start daemon");
        let deadline = Instant::now() + START_PATIENCE;
        while UnixStream::connect(&socket).is_err() {
            if let Some(status) = child.try_wait().expect("poll daemon") {
                panic!("daemon exited before it listened: {status}");
            }
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
    let document = molt_ir::ir::BackendIrDocument::from_json_value(&serde_json::json!({
        "functions": [
            {"return_abi": "void", "name": "molt_main", "params": [], "ops": [
                {"kind": "call", "s_value": "helper"}, {"kind": "ret_void"}
            ]},
            {"return_abi": "void", "name": "helper", "params": [], "ops": [{"kind": "ret_void"}]}
        ],
        "profile": null
    }))
    .expect("valid compile IR");
    std::io::Write::write_all(
        &mut file,
        &molt_ir::addressed_program::encode_complete_lease(&document).expect("encode lease"),
    )
    .expect("write compile IR lease");

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

#[test]
fn daemon_origin_tracks_production_workers_and_original_cache_entry() {
    use sha2::{Digest, Sha256};
    let identity = |bytes: &[u8]| {
        serde_json::json!({
            "sha256": format!("{:x}", Sha256::digest(bytes)), "size": bytes.len()
        })
    };
    let daemon = Daemon::start("origins");
    for (key, kind, batch_size, batched) in [
        ("direct-object", "object", "100", false),
        ("direct-archive", "archive", "100", false),
        ("worker-archive", "archive", "1", true),
    ] {
        let output = daemon.dir.join(format!("{key}.artifact"));
        let mut request = native_compile_request(
            &output,
            key,
            serde_json::json!({
                "MOLT_BACKEND_BATCH_SIZE": batch_size,
                "MOLT_BACKEND_BATCH_OP_BUDGET": "8000"
            }),
        );
        request["jobs"][0]["native_output_kind"] = kind.into();
        let consumed = std::fs::read(output.with_extension("msgpack")).unwrap();
        let response = daemon.connect().request(request.clone());
        assert_eq!(response["jobs"][0]["ok"], true, "{response}");
        assert_eq!(response["jobs"][0]["cached"], false, "{response}");
        let origin = response["jobs"][0]["origin"].clone();
        assert_eq!(origin["schema"], "molt.daemon-job-origin.v2");
        assert_eq!(origin["daemon_pid"], daemon.child.id());
        assert_eq!(origin["input"]["schema"], "molt.addressed-program-input.v1");
        assert_eq!(origin["input"]["lease"], identity(&consumed));
        #[derive(serde::Deserialize)]
        struct ObservedLease<'a> {
            #[serde(borrow)]
            function_digests: &'a [u8],
        }
        let lease: ObservedLease<'_> = rmp_serde::from_slice(&consumed).unwrap();
        assert_eq!(lease.function_digests.len(), 64, "two ordered functions");
        let expected: String = lease
            .function_digests
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(origin["input"]["function_digests"], expected);
        let emitted = std::fs::read(&output).unwrap();
        assert_eq!(origin["output"], identity(&emitted));
        if batched {
            assert_eq!(origin["codegen"]["kind"], "native-workers");
            let workers = origin["codegen"]["workers"].as_array().unwrap();
            assert_eq!(workers.len(), 2, "actual subprocess fixture must batch");
            let archive = object::read::archive::ArchiveFile::parse(emitted.as_slice()).unwrap();
            let members: Vec<_> = archive.members().map(Result::unwrap).collect();
            assert_eq!(members.len(), workers.len());
            for (index, (member, worker)) in members.iter().zip(workers).enumerate() {
                assert_eq!(worker["schema"], "molt.native-worker-origin.v1");
                let pid = worker["process_id"].as_u64().unwrap();
                assert!(
                    pid > 0
                        && pid != u64::from(daemon.child.id())
                        && pid != u64::from(std::process::id())
                );
                assert_eq!(member.name(), format!("molt_{index:08}.o").as_bytes());
                assert_eq!(worker["output"], identity(member.data(&emitted).unwrap()));
            }
        } else {
            assert_eq!(
                origin["codegen"],
                serde_json::json!({"kind":"in-process", "process_id":daemon.child.id()})
            );
        }
        // These cache probes carry no IR. The original compiler processes have
        // exited for batched jobs; a serving daemon must not replace their origin.
        request["jobs"][0]
            .as_object_mut()
            .unwrap()
            .remove("ir_path");
        request["jobs"][0]["probe_cache_only"] = true.into();
        request["jobs"][0]["function_digests"] = expected.into();
        for tier in ["module", "function"] {
            let replay = daemon.dir.join(format!("{key}-{tier}.artifact"));
            request["jobs"][0]["output"] = replay.to_string_lossy().as_ref().into();
            if tier == "function" {
                request["jobs"][0]["cache_key"] = "".into();
            }
            let hit = daemon.connect().request(request.clone());
            assert_eq!(hit["jobs"][0]["ok"], true, "{hit}");
            assert_eq!(hit["jobs"][0]["cached"], true, "{hit}");
            assert_eq!(hit["jobs"][0]["cache_tier"], tier);
            assert_eq!(hit["jobs"][0]["origin"], origin);
            assert_eq!(std::fs::read(&replay).unwrap(), emitted);
            std::fs::remove_file(&replay).unwrap();
            let flag = format!("skip_{tier}_output_if_synced");
            request["jobs"][0][&flag] = true.into();
            let synced = daemon.connect().request(request.clone());
            assert_eq!(synced["jobs"][0]["ok"], true, "{synced}");
            assert_eq!(synced["jobs"][0]["output_written"], false);
            assert_eq!(synced["jobs"][0]["origin"], origin);
            assert!(!replay.exists());
            request["jobs"][0].as_object_mut().unwrap().remove(&flag);
        }
    }
}
