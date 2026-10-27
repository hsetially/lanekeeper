//! A hub running as a child process, with its output captured and real signals sent to it.
//!
//! Two programs are started this way: the real `hub` binary (the refusal tests: it cannot reach Google, so it must stop with
//! a typed message) and the `hub_dev` example (everything else: the same code with software edges, plan 18 G23). The child
//! gets an empty environment plus what the test passes, so nothing from the developer's shell reaches it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::io::{BufRead, BufReader};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// The four addresses a hub says it is listening on.
#[derive(Debug, Clone, Copy)]
pub struct Listening {
    pub web: SocketAddr,
    pub ops: SocketAddr,
    pub agent: SocketAddr,
    pub replica: SocketAddr,
}

/// Where the output is collected as it arrives.
#[derive(Default)]
struct Collected {
    lines: Vec<String>,
    listening: Option<Listening>,
}

pub struct Child {
    process: std::process::Child,
    collected: Arc<(Mutex<Collected>, Condvar)>,
    started: Instant,
}

/// The `hub` binary cargo built for this test.
pub fn real_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_hub"))
}

/// The `hub_dev` example. `cargo test --test x` does not build examples, so the first call builds it (once, under a lock),
/// into the target directory this test binary lives in.
pub fn example_binary() -> PathBuf {
    static BUILT: OnceLock<PathBuf> = OnceLock::new();
    BUILT
        .get_or_init(|| {
            let exe = std::env::current_exe().unwrap();
            // target/<profile>/deps/<test> -> target/<profile>
            let profile_dir = exe.parent().and_then(|deps| deps.parent()).unwrap().to_path_buf();
            let target_dir = profile_dir.parent().unwrap().to_path_buf();
            let profile = profile_dir.file_name().unwrap().to_string_lossy().into_owned();
            let path = profile_dir.join("examples").join("hub_dev");
            let mut build = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()));
            build
                .args(["build", "--quiet", "-p", "hub", "--example", "hub_dev"])
                .arg("--target-dir")
                .arg(&target_dir);
            match profile.as_str() {
                "debug" => {}
                "release" => {
                    build.arg("--release");
                }
                other => panic!("unsupported profile directory {other}"),
            }
            let status = build.status().unwrap();
            assert!(status.success(), "building the hub_dev example failed");
            assert!(path.is_file(), "no example at {}", path.display());
            path
        })
        .clone()
}

impl Child {
    /// Start `program` with `args` and exactly `env`.
    pub fn spawn(program: &std::path::Path, args: &[&str], env: &[(String, String)]) -> Self {
        let mut command = Command::new(program);
        command
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for (name, value) in env {
            command.env(name, value);
        }
        let started = Instant::now();
        let mut process = command.spawn().unwrap();
        let collected = Arc::new((Mutex::new(Collected::default()), Condvar::new()));
        for stream in [
            Box::new(process.stdout.take().unwrap()) as Box<dyn std::io::Read + Send>,
            Box::new(process.stderr.take().unwrap()),
        ] {
            let collected = Arc::clone(&collected);
            std::thread::spawn(move || {
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { return };
                    let (lock, changed) = &*collected;
                    let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
                    if let Some(addrs) = listening_line(&line) {
                        guard.listening = Some(addrs);
                    }
                    guard.lines.push(line);
                    changed.notify_all();
                }
            });
        }
        Self {
            process,
            collected,
            started,
        }
    }

    pub fn id(&self) -> u32 {
        self.process.id()
    }

    /// The web and operations addresses, once the child says it is listening.
    pub fn wait_listening(&self, within: Duration) -> (SocketAddr, SocketAddr) {
        let all = self.wait_listening_all(within);
        (all.web, all.ops)
    }

    /// All four addresses, once the child says it is listening.
    pub fn wait_listening_all(&self, within: Duration) -> Listening {
        let (lock, changed) = &*self.collected;
        let deadline = Instant::now() + within;
        let mut guard = lock.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            if let Some(addrs) = guard.listening {
                return addrs;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "the hub did not say it was listening within {within:?}. Output so far:\n{}",
                guard.lines.join("\n")
            );
            guard = changed
                .wait_timeout(guard, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Everything the child wrote to stdout and stderr so far.
    pub fn output(&self) -> String {
        let (lock, _) = &*self.collected;
        lock.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .lines
            .join("\n")
    }

    /// Send a signal by name (`TERM`, `INT`), as the kubelet does.
    pub fn signal(&self, name: &str) {
        let status = Command::new("kill")
            .arg(format!("-{name}"))
            .arg(self.process.id().to_string())
            .status()
            .unwrap();
        assert!(status.success(), "kill -{name} failed");
    }

    /// The exit status if the child ends within `within`; the output joins the panic message otherwise.
    pub fn wait_exit(&mut self, within: Duration) -> Option<ExitStatus> {
        let deadline = Instant::now() + within;
        loop {
            if let Some(status) = self.process.try_wait().unwrap() {
                // Let the reader threads take the last lines.
                std::thread::sleep(Duration::from_millis(100));
                return Some(status);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Like [`Child::wait_exit`], and a child that does not end is a failure that shows what it said.
    pub fn exit_within(&mut self, within: Duration) -> ExitStatus {
        match self.wait_exit(within) {
            Some(status) => status,
            None => panic!(
                "the hub did not exit within {within:?}. Output:\n{}",
                self.output()
            ),
        }
    }

    pub fn has_exited(&mut self) -> bool {
        self.process.try_wait().unwrap().is_some()
    }

    pub fn running_for(&self) -> Duration {
        self.started.elapsed()
    }
}

impl Drop for Child {
    fn drop(&mut self) {
        // A test that failed must not leave a hub behind.
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// `{"fields":{"message":"listening","web":"127.0.0.1:1","ops":"127.0.0.1:2","agent":...,"replica":...}, ...}` -> the addresses.
fn listening_line(line: &str) -> Option<Listening> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let fields = value.get("fields")?;
    if fields.get("message")?.as_str()? != "listening" {
        return None;
    }
    let address = |name: &str| fields.get(name)?.as_str()?.parse().ok();
    Some(Listening {
        web: address("web")?,
        ops: address("ops")?,
        agent: address("agent")?,
        replica: address("replica")?,
    })
}
