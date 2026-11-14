//! The control endpoint: lets tools (agents through `ponko-mcp`, the
//! inspector, scripts) look inside a running game and drive it.
//!
//! Off by default. Turned on with [`Game::control_endpoint`](crate::game::Game::control_endpoint),
//! or by the `PONKO_CONTROL=1` environment variable in builds that honor it
//! (debug builds, and release builds with the `control-env` feature), so
//! tools can enable it for a game under development without code changes. A
//! release build of a game that did neither never opens a port.
//!
//! Protocol: TCP on `127.0.0.1`, one JSON object per line each way.
//! Request: `{"id": 1, "token": "...", "method": "entity.get", "params": {...}}`.
//! Reply: `{"id": 1, "result": ...}` or `{"id": 1, "error": "..."}`.
//! The token is in the instance's registry file, readable only by its user.
//!
//! Requests are applied by the game loop between frames (or between ticks
//! when headless), so they never race the simulation or each other.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use glam::Vec2;
use ponko_ecs::Entity;
use serde_json::{Value, json};

use crate::ctx::Ctx;
// `crate::units::Health` → `Health`, for archetype listings.
use crate::diagnostics::short_name as short;
use crate::reflect::WorldReflect;

mod capabilities;
mod editor;
mod inspector;
mod query;
use query::{Condition, FieldPath, FieldStats};

/// Environment variable that turns the endpoint on (`1`).
pub(crate) const ENV_CONTROL: &str = "PONKO_CONTROL";
/// Environment variable that lets tools start input scenarios (`1`).
pub(crate) const ENV_INPUT_INJECTION: &str = "PONKO_INPUT_INJECTION";
/// Environment variable that starts the simulation paused (`1`).
pub(crate) const ENV_START_PAUSED: &str = "PONKO_START_PAUSED";
/// Environment variable overriding where instances register themselves.
pub(crate) const ENV_INSTANCES_DIR: &str = "PONKO_INSTANCES_DIR";
/// Environment variable a launching tool sets; copied into the registry file.
pub(crate) const ENV_LAUNCH_ID: &str = "PONKO_LAUNCH_ID";

/// How long a connection waits for the game to answer before giving up
/// (a game stuck in a long frame, or paused in a debugger).
const REPLY_TIMEOUT: Duration = Duration::from_secs(30);

/// How long a game that is exiting waits for the replies it already owes
/// to be written to their sockets, so the client that asked it to quit
/// hears that it did. A client that stopped reading costs at most this.
const EXIT_FLUSH_TIMEOUT: Duration = Duration::from_secs(2);

/// How often the accept loop checks whether the game has shut down.
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// The longest request line a connection may send. Reading stops there,
/// before the token is checked, so a stranger can't make the game buffer
/// an endless line.
const MAX_LINE: usize = 4 * 1024 * 1024;

/// How long a new connection may take to send its first request with the
/// right token. Until then it holds one of the [`MAX_CONNECTIONS`] slots, so
/// connections that never say anything can't lock tools out. Afterwards a
/// connection may stay idle (one bound to an input scenario, say).
const AUTH_TIMEOUT: Duration = Duration::from_secs(10);

/// How many connections may be open at once. Each has a thread; past this
/// a new connection is told why and closed.
const MAX_CONNECTIONS: usize = 32;

/// Upper bound for `entities.list`, so one request can't produce gigabytes.
const MAX_LIST: usize = 10_000;

pub(crate) fn env_flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| v == "1" || v.eq_ignore_ascii_case("true"))
}

/// Whether this build lets [`ENV_CONTROL`] open the endpoint: debug builds,
/// and release builds with the `control-env` feature (which the MCP `launch`
/// tool turns on). A shipped release build without it ignores the variable,
/// so players can't open a port into a game that didn't ask for one.
pub(crate) const ENV_CONTROL_HONORED: bool = cfg!(any(debug_assertions, feature = "control-env"));

/// Whether `PONKO_CONTROL=1` turns the endpoint on in this build. Says so on
/// stderr when the variable is set but ignored, so a tool waiting for the
/// endpoint can tell why it never opens.
pub(crate) fn env_control() -> bool {
    env_control_with(env_flag(ENV_CONTROL), ENV_CONTROL_HONORED)
}

fn env_control_with(requested: bool, honored: bool) -> bool {
    if requested && !honored {
        eprintln!("{ENV_CONTROL_IGNORED}");
    }
    requested && honored
}

/// What a release build without `control-env` prints when `PONKO_CONTROL` is
/// set. `ponko-mcp` looks for the start of it in a launched game's log.
pub(crate) const ENV_CONTROL_IGNORED: &str = "ponko: PONKO_CONTROL is ignored: this release build opens the control endpoint only with the `control-env` feature or `Game::control_endpoint(true)`";

/// The directory where running instances register: `$PONKO_INSTANCES_DIR`,
/// else `~/.ponko/instances` (`%LOCALAPPDATA%\ponko\instances` on Windows).
pub(crate) fn instances_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os(ENV_INSTANCES_DIR) {
        return Some(PathBuf::from(dir));
    }
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("ponko").join("instances"))
    } else {
        std::env::var_os("HOME").map(|d| PathBuf::from(d).join(".ponko").join("instances"))
    }
}

/// A random 128-bit token, hex, from the operating system's secure random
/// source. Any local program that can read the registry file may connect;
/// the token keeps out everything else (other users, web pages talking to
/// localhost), so it must not be guessable.
#[cfg(any(unix, windows))]
fn random_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|e| std::io::Error::other(format!("no secure random source: {e}")))?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// No TCP endpoint on other platforms (the web), so no token either.
#[cfg(not(any(unix, windows)))]
fn random_token() -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the control endpoint needs a secure random source",
    ))
}

struct Request {
    method: String,
    params: Value,
    reply: Sender<Result<Value, String>>,
}

/// How many requests have been handed to the game whose replies are not on
/// their sockets yet. The connection threads count; the game waits for zero
/// before it exits.
#[derive(Default)]
struct Owed {
    count: std::sync::Mutex<usize>,
    none: std::sync::Condvar,
}

impl Owed {
    fn count(&self) -> std::sync::MutexGuard<'_, usize> {
        self.count.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// One more reply is owed until the returned guard is dropped.
    fn one_more(self: &Arc<Self>) -> OwedReply {
        *self.count() += 1;
        OwedReply(self.clone())
    }

    /// Waits until nothing is owed, for at most `timeout`. Calls `idle`
    /// between looks (to refuse requests that arrive meanwhile).
    fn wait_for_none(&self, timeout: Duration, mut idle: impl FnMut()) {
        let deadline = std::time::Instant::now() + timeout;
        let mut count = self.count();
        while *count > 0 {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            if left.is_zero() {
                return;
            }
            drop(count);
            idle();
            count = self.count();
            if *count == 0 {
                return;
            }
            count = self
                .none
                .wait_timeout(count, left.min(Duration::from_millis(5)))
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}

/// A reply the game owes; dropped once it was written (or could not be).
struct OwedReply(Arc<Owed>);

impl Drop for OwedReply {
    fn drop(&mut self) {
        let mut count = self.0.count();
        *count -= 1;
        if *count == 0 {
            self.0.none.notify_all();
        }
    }
}

/// A running endpoint. Dropping it stops accepting connections and removes
/// the registry file.
pub(crate) struct Control {
    requests: Receiver<Request>,
    addr: SocketAddr,
    registry_file: Option<PathBuf>,
    shutdown: Arc<AtomicBool>,
    owed: Arc<Owed>,
    #[cfg(test)]
    token: Arc<str>,
    /// Keeps stdout and stderr for `logs.get` while the endpoint is open.
    _logs: Option<crate::logs::Capture>,
}

impl Control {
    /// Opens the endpoint on a free localhost port and registers the instance.
    pub(crate) fn start(name: &str, headless: bool) -> std::io::Result<Self> {
        Self::start_with(name, headless, true)
    }

    /// [`start`](Self::start), with or without the capture of stdout and
    /// stderr for `logs.get`. The capture is one per process, so tests that
    /// open endpoints side by side leave it off.
    fn start_with(name: &str, headless: bool, capture_logs: bool) -> std::io::Result<Self> {
        Self::start_with_auth_timeout(name, headless, capture_logs, AUTH_TIMEOUT)
    }

    /// [`start_with`](Self::start_with), with another [`AUTH_TIMEOUT`].
    fn start_with_auth_timeout(
        name: &str,
        headless: bool,
        capture_logs: bool,
        auth_timeout: Duration,
    ) -> std::io::Result<Self> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        listener.set_nonblocking(true)?;
        let addr = listener.local_addr()?;
        let token: Arc<str> = random_token()?.into();
        let (tx, requests) = mpsc::channel();
        let shutdown = Arc::new(AtomicBool::new(false));

        let registry_file = instances_dir().and_then(|dir| {
            let info = json!({
                "name": name,
                "pid": std::process::id(),
                "port": addr.port(),
                "token": &*token,
                "headless": headless,
                "cwd": std::env::current_dir().ok(),
                "exe": std::env::current_exe().ok(),
                "started": SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs()),
                "engine_version": env!("CARGO_PKG_VERSION"),
                // Set by tools that launch games, to find the instance they started.
                "launch_id": std::env::var(ENV_LAUNCH_ID).ok(),
            });
            match write_registry(&dir, addr.port(), &info) {
                Ok(path) => Some(path),
                Err(e) => {
                    eprintln!("ponko: control endpoint could not register in {}: {e}", dir.display());
                    None
                }
            }
        });

        let stop = shutdown.clone();
        #[cfg(test)]
        let kept_token = token.clone();
        let owed = Arc::new(Owed::default());
        let counted = owed.clone();
        thread::Builder::new()
            .name("ponko-control".into())
            .spawn(move || accept_loop(listener, token, tx, stop, counted, auth_timeout))?;
        let logs = capture_logs.then(crate::logs::Capture::start).flatten();
        if capture_logs {
            eprintln!("ponko: control endpoint on {addr}");
        }
        Ok(Self {
            requests,
            addr,
            registry_file,
            shutdown,
            owed,
            #[cfg(test)]
            token: kept_token,
            _logs: logs,
        })
    }

    pub(crate) fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Handles every request that has arrived. Called between frames/ticks.
    pub(crate) fn poll(&self, ctx: &mut Ctx) {
        ctx.events.collect_feed(&ctx.world);
        while let Ok(req) = self.requests.try_recv() {
            answer(ctx, req);
        }
    }

    /// Waits up to `timeout` for a request, then handles everything queued.
    /// Used by a paused headless game, which has nothing else to do.
    pub(crate) fn wait(&self, ctx: &mut Ctx, timeout: Duration) {
        ctx.events.collect_feed(&ctx.world);
        match self.requests.recv_timeout(timeout) {
            Ok(req) => answer(ctx, req),
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
        }
        self.poll(ctx);
    }
}

impl Control {
    /// The game is exiting (often because a client asked it to): the
    /// connection threads die with the process, so this waits until every
    /// reply already owed has been written to its socket, for at most
    /// [`EXIT_FLUSH_TIMEOUT`]. Requests that were queued but not handled, or
    /// that still arrive, are refused instead of left waiting.
    fn flush_replies(&self) {
        self.owed.wait_for_none(EXIT_FLUSH_TIMEOUT, || {
            while let Ok(req) = self.requests.try_recv() {
                let _ = req.reply.send(Err("the game is shutting down".into()));
            }
        });
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        if let Some(path) = &self.registry_file {
            let _ = std::fs::remove_file(path);
        }
        self.flush_replies();
    }
}

fn write_registry(dir: &Path, port: u16, info: &Value) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}-{port}.json", std::process::id()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        options.mode(0o600);
        let _ = std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700));
    }
    let mut file = options.open(&path)?;
    file.write_all(serde_json::to_string_pretty(info)?.as_bytes())?;
    Ok(path)
}

fn accept_loop(
    listener: TcpListener,
    token: Arc<str>,
    tx: Sender<Request>,
    shutdown: Arc<AtomicBool>,
    owed: Arc<Owed>,
    auth_timeout: Duration,
) {
    let open = Arc::new(AtomicUsize::new(0));
    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((mut stream, _)) => {
                let Some(slot) = ConnectionSlot::take(&open) else {
                    let _ = stream.set_nonblocking(false);
                    let _ = writeln!(
                        stream,
                        "{}",
                        json!({ "id": null, "error": format!("too many connections (at most {MAX_CONNECTIONS}); close one and retry") })
                    );
                    // Not drained here, so the accept loop never waits.
                    let _ = stream.shutdown(std::net::Shutdown::Write);
                    continue;
                };
                let (token, tx, owed) = (token.clone(), tx.clone(), owed.clone());
                let _ = thread::Builder::new().name("ponko-control-conn".into()).spawn(move || {
                    let _slot = slot;
                    serve_connection(stream, &token, &tx, &owed, auth_timeout);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            Err(_) => thread::sleep(ACCEPT_POLL),
        }
    }
}

/// One of the [`MAX_CONNECTIONS`] open connections; frees it when dropped.
struct ConnectionSlot(Arc<AtomicUsize>);

impl ConnectionSlot {
    fn take(open: &Arc<AtomicUsize>) -> Option<Self> {
        // Counted in first; past the cap, dropping it counts it out again.
        let slot = Self(open.clone());
        if open.fetch_add(1, Ordering::AcqRel) < MAX_CONNECTIONS {
            Some(slot)
        } else {
            None
        }
    }
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Reads one request line of at most [`MAX_LINE`] bytes, without the
/// newline. `Ok(None)`: the client closed the connection.
fn read_request_line(reader: &mut impl BufRead) -> std::io::Result<Option<String>> {
    let mut buf = Vec::new();
    let read = reader.take(MAX_LINE as u64 + 1).read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > MAX_LINE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("request line longer than {MAX_LINE} bytes"),
        ));
    }
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    String::from_utf8(buf)
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// A connection's reading side: until `deadline` (while the client hasn't
/// sent the right token yet), every read waits only for the time left, so
/// a client trickling bytes can't hold its slot either.
struct DeadlineReader {
    stream: TcpStream,
    deadline: Option<std::time::Instant>,
}

impl Read for DeadlineReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self.deadline {
            Some(deadline) => read_before(&self.stream, buf, deadline),
            None => self.stream.read(buf),
        }
    }
}

/// How often a read with a deadline looks for input.
const DEADLINE_POLL: Duration = Duration::from_millis(2);

/// Reads what has arrived, waiting for input until `deadline` (then
/// `TimedOut`). It never lets a blocking read time out (`SO_RCVTIMEO`): on
/// Windows that leaves the connection in an undefined state, and the reply
/// written after it is lost (the client sees the connection aborted). So the
/// socket is nonblocking while it waits, and blocking again afterwards.
fn read_before(stream: &TcpStream, buf: &mut [u8], deadline: std::time::Instant) -> std::io::Result<usize> {
    stream.set_nonblocking(true)?;
    let mut reading = stream;
    let read = loop {
        match reading.read(buf) {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                let left = deadline.saturating_duration_since(std::time::Instant::now());
                if left.is_zero() {
                    break Err(std::io::ErrorKind::TimedOut.into());
                }
                thread::sleep(left.min(DEADLINE_POLL));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            read => break read,
        }
    };
    stream.set_nonblocking(false)?;
    read
}

impl DeadlineReader {
    /// The client proved it has the token: no deadlines from now on (the
    /// write timeout is the socket's, shared with the writing handle).
    fn trust(&mut self) {
        if self.deadline.take().is_some() {
            let _ = self.stream.set_write_timeout(None);
        }
    }

    fn trusted(&self) -> bool {
        self.deadline.is_none()
    }
}

/// Ends a connection we refuse after the last reply: says we're done
/// writing, then reads (and drops) what the client still sends for a moment.
/// Closing a socket with unread input resets the connection, and a reset
/// can throw away the reply before the client read it (Windows does).
fn hang_up(stream: &TcpStream) {
    const DRAIN_FOR: Duration = Duration::from_millis(200);
    const DRAIN_AT_MOST: usize = 1024 * 1024;
    if stream.shutdown(std::net::Shutdown::Write).is_err() {
        return;
    }
    let until = std::time::Instant::now() + DRAIN_FOR;
    let (mut drained, mut buf) = (0, [0u8; 8192]);
    while drained < DRAIN_AT_MOST {
        match read_before(stream, &mut buf, until) {
            Ok(0) | Err(_) => return,
            Ok(n) => drained += n,
        }
    }
}

fn serve_connection(stream: TcpStream, token: &str, tx: &Sender<Request>, owed: &Arc<Owed>, auth_timeout: Duration) {
    static NEXT_CONNECTION: AtomicU64 = AtomicU64::new(1);
    let connection = NEXT_CONNECTION.fetch_add(1, Ordering::Relaxed);
    let mut bound_scenario = false;
    serve_lines(stream, token, tx, connection, &mut bound_scenario, owed, auth_timeout);
    if bound_scenario {
        // The client that started the scenario is gone: let go of its input.
        let (reply, _) = mpsc::channel();
        let _ = tx.send(Request {
            method: "input.scenario.cancel".into(),
            params: json!({ "reason": "the client that started it disconnected", "connection": connection }),
            reply,
        });
    }
}

fn serve_lines(
    stream: TcpStream,
    token: &str,
    tx: &Sender<Request>,
    connection: u64,
    bound_scenario: &mut bool,
    owed: &Arc<Owed>,
    auth_timeout: Duration,
) {
    let _ = stream.set_nonblocking(false);
    // A client that doesn't read its replies can't hold the connection
    // either: until it sends the token, a blocked write gives up too.
    let _ = stream.set_write_timeout(Some(auth_timeout));
    let Ok(mut writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(DeadlineReader {
        stream,
        deadline: Some(std::time::Instant::now() + auth_timeout),
    });
    loop {
        let line = match read_request_line(&mut reader) {
            Ok(Some(line)) => line,
            Ok(None) => return,
            Err(e) => {
                // Too long, not UTF-8, or no token in time: say why, then
                // hang up (the rest of an over-long line can't be told apart
                // from a new request).
                let error = if matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock) {
                    format!("no request with the right token within {auth_timeout:?}; closing")
                } else {
                    e.to_string()
                };
                let _ = writeln!(writer, "{}", json!({ "id": null, "error": error }));
                hang_up(&writer);
                return;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        // Owed from before the game can see the request until the reply is
        // written: a game that exits in between waits for this.
        let owed_reply = owed.one_more();
        let (reply, trusted) = handle_line(&line, token, tx, connection);
        if trusted {
            reader.get_mut().trust();
        } else if !reader.get_ref().trusted() {
            // The first request must carry the token: say why, then hang
            // up, so strangers get one reply and no more.
            drop(owed_reply);
            let _ = writeln!(writer, "{reply}").and_then(|()| writer.flush());
            hang_up(&writer);
            return;
        }
        if reply["result"]["bound_to_connection"] == true {
            *bound_scenario = true;
        }
        let written = writeln!(writer, "{reply}").and_then(|()| writer.flush());
        drop(owed_reply);
        if written.is_err() {
            return;
        }
    }
}

/// Whether `sent` is `token`, in time that doesn't depend on where they
/// differ, so the token can't be guessed a byte at a time.
fn token_matches(sent: Option<&str>, token: &str) -> bool {
    let Some(sent) = sent else {
        return false;
    };
    let (a, b) = (sent.as_bytes(), token.as_bytes());
    let mut diff = u8::from(a.len() != b.len());
    for (i, &t) in b.iter().enumerate() {
        diff |= t ^ a.get(i).copied().unwrap_or(!t);
    }
    std::hint::black_box(diff) == 0
}

/// The reply to one request line, and whether it carried the right token.
fn handle_line(line: &str, token: &str, tx: &Sender<Request>, connection: u64) -> (Value, bool) {
    let msg: Value = match serde_json::from_str(line) {
        Ok(v) => v,
        Err(e) => return (json!({ "id": null, "error": format!("invalid JSON: {e}") }), false),
    };
    if !token_matches(msg.get("token").and_then(Value::as_str), token) {
        let id = msg.get("id").cloned().unwrap_or(Value::Null);
        let error = "missing or wrong token (see the instance's registry file)";
        return (json!({ "id": id, "error": error }), false);
    }
    (handle_request(&msg, tx, connection), true)
}

/// A request with the right token.
fn handle_request(msg: &Value, tx: &Sender<Request>, connection: u64) -> Value {
    let id = msg.get("id").cloned().unwrap_or(Value::Null);
    let reply = |r: Result<Value, String>| match r {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(error) => json!({ "id": id, "error": error }),
    };
    let Some(method) = msg.get("method").and_then(Value::as_str) else {
        return reply(Err("missing `method`".into()));
    };
    if method == "logs.get" {
        // Answered right here, not by the game loop: logs matter most when
        // the game is stuck in a long frame.
        return reply(logs_get(msg.get("params").unwrap_or(&Value::Null)));
    }
    let (reply_tx, reply_rx) = mpsc::channel();
    let mut params = msg.get("params").cloned().unwrap_or(Value::Null);
    if let Some(map) = params.as_object_mut() {
        // Set by the server, never by the client: which connection is asking.
        map.insert("_connection".into(), json!(connection));
    }
    let req = Request {
        method: method.to_string(),
        params,
        reply: reply_tx,
    };
    if tx.send(req).is_err() {
        return reply(Err("the game is shutting down".into()));
    }
    reply(match reply_rx.recv_timeout(REPLY_TIMEOUT) {
        Ok(r) => r,
        Err(RecvTimeoutError::Timeout) => Err("the game did not answer in time".into()),
        // The game let go of the request without answering: it is exiting.
        Err(RecvTimeoutError::Disconnected) => Err("the game is shutting down".into()),
    })
}

/// The JSON of the components an entity was matched on.
#[derive(Clone, Copy)]
struct Found<'a> {
    names: &'a [String],
    jsons: &'a [Value],
}

impl<'a> Found<'a> {
    fn get(&self, component: &str) -> Option<&'a Value> {
        self.names.iter().position(|n| n == component).map(|i| &self.jsons[i])
    }
}

/// Calls `f` for every entity that has the `with` components, the ones the
/// `fields` name, and passes every `where` condition. `f` gets the entity,
/// its archetype's component names, and a lookup of the JSON of the
/// components named in conditions and fields.
fn for_each_matching(
    world: &ponko_ecs::World,
    params: &Value,
    fields: &[FieldPath],
    mut f: impl FnMut(Entity, &[&str], Found) -> Result<(), String>,
) -> Result<(), String> {
    let mut with: Vec<String> = match params.get("with") {
        None | Some(Value::Null) => Vec::new(),
        Some(v) => serde_json::from_value(v.clone()).map_err(|_| "`with` must be a list of component names")?,
    };
    let conditions = query::list(params, "where", Condition::parse)?;
    // The components whose JSON is needed, each once.
    let mut needed: Vec<String> = Vec::new();
    for c in conditions.iter().map(|c| &c.path).chain(fields) {
        if !needed.contains(&c.component) {
            needed.push(c.component.clone());
        }
    }
    let reflected = world.reflected_components();
    if let Some(n) = needed.iter().find(|n| !reflected.contains(&n.as_str())) {
        return Err(format!(
            "`{n}` is not a reflected component (reflected: {})",
            reflected.join(", ")
        ));
    }
    with.extend(needed.iter().cloned());
    let mut jsons: Vec<Value> = Vec::with_capacity(needed.len());
    for a in world.raw_ref().archetypes().iter().filter(|a| !a.is_empty()) {
        let names: Vec<&str> = a
            .component_ids()
            .iter()
            .map(|&id| short(world.component_info(id).name()))
            .collect();
        if !with.iter().all(|w| names.contains(&w.as_str())) {
            continue;
        }
        for &e in a.entities() {
            jsons.clear();
            for n in &needed {
                jsons.push(world.get_json(e, n).map_err(|e| e.to_string())?);
            }
            let found = Found {
                names: &needed,
                jsons: &jsons,
            };
            if conditions
                .iter()
                .all(|c| found.get(&c.path.component).is_some_and(|j| c.holds(j)))
            {
                f(e, &names, found)?;
            }
        }
    }
    Ok(())
}

fn logs_get(params: &Value) -> Result<Value, String> {
    let since = params.get("since").and_then(Value::as_u64);
    let max = params.get("lines").and_then(Value::as_u64).unwrap_or(100) as usize;
    crate::logs::get(since, max)
}

fn answer(ctx: &mut Ctx, req: Request) {
    if req.method == "screenshot" && !ctx.headless {
        // Answered after the next frame is drawn.
        match screenshot_path(&req.params) {
            Ok(claimed) => {
                let max_size = req.params.get("max_size").and_then(Value::as_u64).unwrap_or(0) as u32;
                ctx.gfx.queue_claimed_screenshot(claimed, max_size, req.reply);
            }
            Err(e) => {
                let _ = req.reply.send(Err(e));
            }
        }
        return;
    }
    let result = handle(ctx, &req.method, &req.params);
    let _ = req.reply.send(result);
}

fn camera_json(ctx: &Ctx) -> Value {
    let c = &ctx.camera;
    let r = c.visible_rect();
    json!({
        "position": [c.position.x, c.position.y],
        "zoom": c.zoom,
        "center": [c.center().x, c.center().y],
        "visible": { "min": [r.min.x, r.min.y], "max": [r.max.x, r.max.y] },
    })
}

/// The `path` param (a new `.png` in the game's folder, see [`new_game_file`]),
/// or a new file in the temp dir. Either way the file is made here, empty,
/// so the name is ours before anything is written to it.
fn screenshot_path(params: &Value) -> Result<ClaimedFile, String> {
    match params.get("path") {
        None | Some(Value::Null) => new_temp_file("screenshot", "png"),
        Some(Value::String(p)) => new_game_file(p, "png"),
        Some(_) => Err("`path` must be a string".into()),
    }
}

/// Names Windows keeps for devices, with any extension (`nul.json` is the
/// null device there).
fn is_reserved_name(name: &str) -> bool {
    let stem = name.split('.').next().unwrap_or(name).trim_end().to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "CONIN$" | "CONOUT$")
        || ["COM", "LPT"].iter().any(|p| {
            stem.strip_prefix(p).is_some_and(|n| {
                let mut digits = n.chars();
                // Windows counts 0 and the superscript digits too.
                matches!(
                    (digits.next(), digits.next()),
                    (Some('0'..='9' | '\u{b9}' | '\u{b2}' | '\u{b3}'), None)
                )
            })
        })
}

/// Checks a file path a tool sent: relative, made only of plain names (no
/// `..`, `.`, root or drive, no hidden names starting with `.`, no Windows
/// device names), ending in `.{extension}`. So a tool can only name files
/// of that type below the folder the path is taken from.
fn check_tool_path(path: &str, extension: &str) -> Result<(), String> {
    use std::path::Component;
    let p = Path::new(path);
    let plain = !path.is_empty()
        && p.components().all(|c| match c {
            Component::Normal(name) => name
                .to_str()
                .is_some_and(|n| !n.starts_with('.') && !is_reserved_name(n)),
            _ => false,
        });
    // `\` separates folders on Windows only; refuse it everywhere, so a path
    // means the same on every system.
    if !plain || path.contains('\\') || path.contains(':') {
        return Err(format!(
            "`path` must be relative, below the game's folder, without `..`, hidden or device names (got {path:?})"
        ));
    }
    if p.extension().and_then(|e| e.to_str()) != Some(extension) {
        return Err(format!("`path` must end in .{extension} (got {path:?})"));
    }
    Ok(())
}

/// Fails unless `file`, with links followed as far as it exists, is below
/// `root`: a link inside the game's folder must not lead a tool outside it.
fn check_below(root: &Path, file: &Path) -> Result<(), String> {
    let root = root
        .canonicalize()
        .map_err(|e| format!("can't resolve {}: {e}", root.display()))?;
    // The nearest part of the path that exists (a dangling link counts, and
    // then fails to resolve).
    let mut existing = file;
    while existing.symlink_metadata().is_err() {
        match existing.parent() {
            Some(parent) => existing = parent,
            None => return Err(format!("{} is not below {}", file.display(), root.display())),
        }
    }
    let real = existing
        .canonicalize()
        .map_err(|e| format!("can't resolve {}: {e}", existing.display()))?;
    if real.starts_with(&root) {
        Ok(())
    } else {
        Err(format!(
            "`path` leads outside {} (through a link to {})",
            root.display(),
            real.display()
        ))
    }
}

/// The game's working folder, where tools' files go (the `cwd` of its
/// registry entry).
fn game_folder() -> Result<PathBuf, String> {
    std::env::current_dir().map_err(|e| format!("the game has no working folder: {e}"))
}

/// A file a tool names, in the game's working folder. See
/// [`check_tool_path`] and [`check_below`].
fn game_file(path: &str, extension: &str) -> Result<PathBuf, String> {
    check_tool_path(path, extension)?;
    let cwd = game_folder()?;
    let file = cwd.join(path);
    check_below(&cwd, &file)?;
    Ok(file)
}

/// Makes the folders `file` (below `root`) needs with `create`, checking
/// before and after that no link leads out of `root`: a folder swapped for
/// a link in between is caught by the second check.
fn make_folders(root: &Path, file: &Path, create: impl FnOnce(&Path) -> std::io::Result<()>) -> Result<(), String> {
    check_below(root, file)?;
    if let Some(dir) = file.parent() {
        create(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    check_below(root, file)
}

/// Writes `bytes` to a [`game_file`], making missing folders: only to a new
/// file unless `overwrite`, and never through a link.
fn write_game_file(path: &str, extension: &str, bytes: &[u8], overwrite: bool) -> Result<PathBuf, String> {
    let file = game_file(path, extension)?;
    make_folders(&game_folder()?, &file, |dir| std::fs::create_dir_all(dir))?;
    if !overwrite {
        let mut out = claim_new_file(&file, "pass overwrite: true")?;
        return match out.write_all(bytes) {
            Ok(()) => Ok(file),
            Err(e) => {
                let _ = std::fs::remove_file(&file);
                Err(format!("could not write {}: {e}", file.display()))
            }
        };
    }
    if file.symlink_metadata().is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(format!(
            "{} is a link; overwrite replaces only plain files",
            file.display()
        ));
    }
    let mut out = open_to_overwrite(&file)?;
    out.write_all(bytes)
        .map_err(|e| format!("could not write {}: {e}", file.display()))?;
    Ok(file)
}

/// Opens `file` to replace it, emptied, if it is a plain file of its own:
/// not a link (on unix the open itself refuses one, `O_NOFOLLOW`, also one
/// made after [`write_game_file`] checked), not a pipe or device (opened
/// without waiting, `O_NONBLOCK`, and refused), not a file with other
/// names (hard links; unix only), and not a file a screenshot or trace has
/// claimed (on Windows recognized by its full path only, see [`FileId`]).
/// It is emptied only once all that is checked.
fn open_to_overwrite(file: &Path) -> Result<std::fs::File, String> {
    let fail = |why: String| format!("could not write {}: {why}", file.display());
    let busy = || fail("a screenshot or trace will be written there; name another file, or wait for it".into());
    if is_claimed_path(file) {
        return Err(busy());
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::custom_flags(&mut options, libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let out = options.open(file).map_err(|e| fail(e.to_string()))?;
    let meta = out.metadata().map_err(|e| fail(e.to_string()))?;
    if !meta.is_file() {
        return Err(fail("not a plain file".into()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if meta.nlink() > 1 {
            return Err(fail(
                "the file has other names (hard links); overwrite replaces only plain files".into(),
            ));
        }
        // Claimed between the check above and the open.
        if ClaimedFile::is_claimed(&(meta.dev(), meta.ino())) {
            return Err(busy());
        }
    }
    out.set_len(0).map_err(|e| fail(e.to_string()))?;
    Ok(out)
}

/// Makes `file` new and empty (never one that exists, or a link) and
/// returns it open for writing, so what is written can't end up anywhere
/// else. `instead` says what to do when it exists.
fn claim_new_file(file: &Path, instead: &str) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(file)
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                format!("{} already exists; name a new file, or {instead}", file.display())
            } else {
                format!("could not create {}: {e}", file.display())
            }
        })
}

/// A new file a tool will have written later (screenshots, traces),
/// claimed when it asked: written through [`file`](Self::file), never
/// reopened by name. Removed when dropped unless [`keep`](Self::keep) was
/// called, so a game that shuts down (or a request that fails) before
/// writing it leaves no empty file behind. While claimed, `world.save`
/// won't overwrite it, and it is removed only if the path still names it.
pub(crate) struct ClaimedFile {
    pub(crate) path: PathBuf,
    file: Option<std::fs::File>,
    /// Which file it is, while it is claimed.
    id: Option<FileId>,
}

/// Which file a path or handle is: its device and inode on unix; elsewhere
/// its full path, in lower case (Windows' names ignore case).
#[cfg(unix)]
type FileId = (u64, u64);
#[cfg(not(unix))]
type FileId = PathBuf;

/// The [`FileId`] of what `path` names (not following a link), if anything.
fn file_id(path: &Path) -> Option<FileId> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let meta = path.symlink_metadata().ok()?;
        Some((meta.dev(), meta.ino()))
    }
    #[cfg(not(unix))]
    {
        let full = path.canonicalize().ok()?;
        Some(PathBuf::from(full.to_string_lossy().to_lowercase()))
    }
}

/// Whether `path` names a file a screenshot or trace has claimed and not
/// written yet: other writers leave it alone (writing over it would lose
/// what they wrote when the claim is given up).
pub(crate) fn is_claimed_path(path: &Path) -> bool {
    file_id(path).is_some_and(|id| ClaimedFile::is_claimed(&id))
}

/// Files claimed and neither kept nor given up yet.
static CLAIMED: std::sync::Mutex<Vec<FileId>> = std::sync::Mutex::new(Vec::new());

fn claimed_files() -> std::sync::MutexGuard<'static, Vec<FileId>> {
    CLAIMED.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl ClaimedFile {
    fn new(path: PathBuf, file: std::fs::File) -> Self {
        #[cfg(unix)]
        let id = {
            use std::os::unix::fs::MetadataExt;
            file.metadata().ok().map(|m| (m.dev(), m.ino()))
        };
        #[cfg(not(unix))]
        let id = file_id(&path);
        claimed_files().extend(id.iter().cloned());
        Self {
            path,
            file: Some(file),
            id,
        }
    }

    fn is_claimed(id: &FileId) -> bool {
        claimed_files().contains(id)
    }

    fn unclaim(&mut self) -> Option<FileId> {
        let id = self.id.take()?;
        let mut claimed = claimed_files();
        if let Some(at) = claimed.iter().position(|c| *c == id) {
            claimed.swap_remove(at);
        }
        Some(id)
    }

    /// The claimed file, to write it.
    pub(crate) fn file(&mut self) -> &mut std::fs::File {
        self.file
            .as_mut()
            .expect("a claimed file is open until it is kept or dropped")
    }

    /// The file is written: keep it.
    pub(crate) fn keep(mut self) {
        // Closed here; with no handle left, the drop removes nothing.
        let _ = self.file.take();
        self.unclaim();
    }

    /// Gives the file up unwritten (or half written): removes it, so the
    /// same path can be asked for again. The same as dropping it.
    pub(crate) fn abandon(self) {}
}

impl Drop for ClaimedFile {
    fn drop(&mut self) {
        let id = self.unclaim();
        // Closed first: Windows can't remove an open file.
        if self.file.take().is_none() {
            return;
        }
        // Only if the path still names the claimed file: not one that
        // replaced it since.
        if id.is_none_or(|id| file_id(&self.path) == Some(id)) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// [`game_file`] for tools that only look (screenshots, traces): a new
/// file, with its folders made and the file claimed here, never one that
/// exists.
fn new_game_file(path: &str, extension: &str) -> Result<ClaimedFile, String> {
    let file = game_file(path, extension)?;
    make_folders(&game_folder()?, &file, |dir| std::fs::create_dir_all(dir))?;
    let claimed = claim_new_file(&file, "leave out `path` for a temp file")?;
    Ok(ClaimedFile::new(file, claimed))
}

/// A new file in the temp dir, with a random name, claimed here.
fn new_temp_file(kind: &str, extension: &str) -> Result<ClaimedFile, String> {
    let random = random_token().map_err(|e| e.to_string())?;
    let file = std::env::temp_dir().join(format!(
        "ponko-{kind}-{}-{}.{extension}",
        std::process::id(),
        &random[..16]
    ));
    let claimed = claim_new_file(&file, "try again")?;
    Ok(ClaimedFile::new(file, claimed))
}

/// The `path` param, a `.json` file in `assets/` (scenes and prefabs). See
/// [`check_asset_path`].
fn asset_path_param(params: &Value) -> Result<&str, String> {
    let path = str_param(params, "path")?;
    #[cfg(not(target_arch = "wasm32"))]
    let roots = crate::assets::asset_roots();
    #[cfg(target_arch = "wasm32")]
    let roots: Vec<PathBuf> = Vec::new();
    check_asset_path(path, &roots)?;
    Ok(path)
}

/// Checks an asset path a tool sent: [`check_tool_path`], and no link
/// inside an `assets/` folder (`roots`) leads out of it. A folder that is
/// itself a link (a shared `assets/` folder, say) is trusted: the check
/// starts where it leads.
fn check_asset_path(path: &str, roots: &[PathBuf]) -> Result<(), String> {
    check_tool_path(path, "json")?;
    for root in roots {
        if root.is_dir() {
            check_below(root, &root.join(path))?;
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ methods

/// Every method the endpoint understands, with a one-line description.
/// Returned by `methods`, so tools can discover them.
pub(crate) const METHODS: &[(&str, &str)] = &[
    ("methods", "list the methods this endpoint understands"),
    (
        "capabilities",
        "what this instance can do: engine version, headless or windowed, compiled features, subsystems (renderer, physics, audio, networking...) with why one is off, each method's availability with a reason when refused, and reflected components/resources with their schema (or `opaque` when not described)",
    ),
    (
        "logs.get",
        "the game's recent stdout and stderr lines (answered even mid-frame); params: lines? (100), since? (the `next` of a previous read: only newer lines)",
    ),
    (
        "info",
        "name, pid, tick, sim time, paused, speed, fps, entity count, render stats (sprites, draw calls, CPU and GPU ms)",
    ),
    (
        "world.summary",
        "entity count, allocated bytes, reflected components, archetypes with their sizes and bytes per entity",
    ),
    (
        "entities.list",
        "entities with their reflected components as JSON and the names of the others; params: with? (component names), where? (like [\"Health.current < 5\"]), limit? (100), offset?",
    ),
    (
        "entities.stats",
        "count, min, max, mean of reflected fields (per axis for vectors; value counts for names and enums); params: fields (like [\"Health.current\", \"Team\"]), with?, where?",
    ),
    ("entity.get", "one entity's reflected components; params: entity"),
    ("entity.spawn", "spawn from {component: value}; params: components"),
    ("entity.despawn", "params: entity"),
    (
        "component.set",
        "write a component (object fields merge); params: entity, component, value",
    ),
    ("component.remove", "params: entity, component"),
    (
        "resources.list",
        "every reflected resource with its value as JSON (null when it's not in the world)",
    ),
    ("resource.get", "one reflected resource as JSON; params: resource"),
    (
        "resource.set",
        "write a reflected resource (object fields merge; inserted if missing); params: resource, value",
    ),
    (
        "world.save",
        "save every entity with reflected components, and reflected resources, to a JSON file; params: path (a .json file relative to the game's folder, no `..`), overwrite? (false: an existing file is refused). Returns counts",
    ),
    (
        "world.load",
        "replace every entity with a save file's (same ids); params: path (a .json file relative to the game's folder, no `..`). Checked first: on error nothing changes",
    ),
    (
        "scene.save",
        "save the world as a scene in assets/ (what ctx.load_scene and the editor read); params: path (a .json file relative to assets/, no `..`)",
    ),
    (
        "scene.load",
        "replace the world with a scene from assets/, loading the images its sprites name; params: path (a .json file relative to assets/)",
    ),
    (
        "editor.pick",
        "the entity under a window point in 3D (the nearest mesh bounds the mouse ray enters) and where that ray meets the ground (y = 0); params: at [x, y] in window pixels. `entity` is null when nothing is there",
    ),
    (
        "editor.transform",
        "move, turn and scale entities as the editor's gizmo does, as one undo step; attached ones keep their parent, and what is attached to them follows; params: entities [ids], translate? [x, y, z], rotate? [x, y, z] (degrees about each axis, about the entities' middle), scale? (a factor or [x, y, z], along each entity's own axes), space? (\"world\" or \"local\": the first entity's axes), snap? (true, or { step, angle (degrees), scale })",
    ),
    (
        "editor.drag",
        "drag a handle of the gizmo from one window point to another, as the mouse does in the editor, as one undo step; params: entities [ids], mode (translate, rotate or scale), handle (x, y, z, xy, xz, yz or center), from [x, y], to [x, y], space? (world or local), snap? (as editor.transform)",
    ),
    (
        "editor.parent",
        "make an entity follow another in the hierarchy (Attached3D in 3D, Attached in 2D) without moving it, or follow nothing; params: entity, parent (an id, or null to detach). Refuses loops",
    ),
    (
        "editor.hierarchy",
        "every entity as a tree: each node's parent and how it hangs off it (attached, attached3d, joint, skin, prefab_part), depth, children, whether it is a prefab instance's root, and its component names; roots first, each followed by its subtree",
    ),
    (
        "editor.assets",
        "what the editor's asset browser lists: glTF meshes, prefabs and scenes in assets/, and the materials of the mesh files loaded so far (mesh, index, name)",
    ),
    (
        "editor.place",
        "drop an asset into the scene, as one undo step: a glTF mesh (a model) or a prefab; params: path (.glb, .gltf or a prefab .json in assets/), at? [x, y, z] (or [x, y] for a 2D prefab), screen? [x, y] (where that window point's ray meets the ground; default the middle of the view), snap? (true or { step }: put it on the grid). Returns the entity and its prefab parts",
    ),
    (
        "editor.material",
        "paint a model with a material of a loaded mesh file (a ModelMaterial), as one undo step; params: entity, mesh (the file), material (its name, or its number in the file)",
    ),
    (
        "editor.delete",
        "despawn entities with everything that hangs off them (prefab parts, attachments), as one undo step; params: entities [ids]",
    ),
    (
        "editor.duplicate",
        "copy entities with everything that hangs off them, as one undo step; params: entities [ids], offset? ([x, y] in 2D, [x, y, z] in 3D). Returns the copies",
    ),
    (
        "editor.undo",
        "undo the last editor change (made here or in the inspector)",
    ),
    ("editor.redo", "redo the last undone editor change"),
    (
        "editor.save",
        "save the world back to the scene file it was loaded from (or last saved to); params: path? (a .json file relative to assets/)",
    ),
    (
        "editor.camera",
        "read or move the 3D camera as the editor's fly camera does; params: position? [x, y, z], look_at? [x, y, z], fov? (degrees), turn? [dx, dy] (mouse pixels), fly? [right, up, forward] (world units), focus? (an entity to frame)",
    ),
    (
        "prefab.spawn",
        "spawn a prefab from assets/ at a point; params: path (a .json file relative to assets/), at [x, y]. Returns the root entity and its parts",
    ),
    (
        "prefab.despawn",
        "despawn a prefab's root and the parts spawned with it; params: entity (the root). Returns how many entities went",
    ),
    (
        "prefab.save",
        "save an entity's reflected components (without its position) as a prefab in assets/; params: entity, path (a .json file relative to assets/)",
    ),
    ("sim.pause", "pause simulation systems"),
    ("sim.resume", "resume simulation systems"),
    ("sim.step", "run extra simulation ticks; params: ticks? (default 1)"),
    ("sim.speed", "set simulation speed; params: speed"),
    (
        "input.scenario.load",
        "start a tick-indexed input scenario (versioned JSON: input steps fed through the real input path, and checks), replacing a running one; its tick 0 is the next simulation tick; params: scenario (the object), bind_connection? (cancel it when this connection closes), keep_input_when_done? (by default the keys it holds are released when it ends; with this they stay held, owned by the scenario's owner until cancelled), max_ticks? (refuse a longer scenario before touching the running one), queue_ticks? (paused games: queue the scenario's own ticks as steps; a failed check or a cancel takes back only those that haven't run). Refused unless the game allows input injection (PONKO_INPUT_INJECTION=1 or Game::input_injection). Returns the report",
    ),
    (
        "input.scenario.status",
        "the latest input scenario's report (status pending/running/passed/failed/cancelled, ticks run, checks passed, the failing step and tick with expected and actual), or null",
    ),
    (
        "input.scenario.cancel",
        "end the running input scenario and release the keys, buttons and axes it holds; params: reason? Returns the report",
    ),
    ("systems.timings", "each system's last and average time in ms"),
    (
        "ecs.diagnostics",
        "the ECS as stable, versioned data, built only when asked: entities and allocated bytes; archetypes (index, full component names, entities, bytes_per_entity, allocated_bytes); components (name, short_name, size, align, archetypes, entities, bytes); systems in run order (name, phase, order, sim, every, offset, access, last_ms, avg_ms). params: with? (component names: adds `matching`, the archetypes and entities a query for them visits), limit? (archetypes listed, non-empty ones in world order, default 100; `archetypes_total` counts all)",
    ),
    (
        "events.list",
        "reflected event types (ctx.events.reflect, or C events whose type is a reflected component) and how many are kept now",
    ),
    (
        "events.subscribe",
        "start collecting new events, between frames, newest kept; params: types? (names, default all), capacity? (1000)",
    ),
    (
        "events.take",
        "events collected since the last take, oldest first: [{tick, type, data}], plus how many were dropped for space",
    ),
    ("events.unsubscribe", "stop collecting events"),
    (
        "camera.get",
        "the camera: position (world point at the window's top-left), zoom, center, visible rect",
    ),
    (
        "camera.set",
        "move the camera; params: position? [x, y], zoom?, look_at? [x, y] (applied after zoom). Returns camera.get",
    ),
    (
        "screenshot",
        "save the next frame as a PNG (windowed games only); params: path? (a new .png file relative to the game's folder; default: a temp file), max_size? (scale down to fit, 0 = full size). Returns path, width, height",
    ),
    (
        "shaders.status",
        "the game's custom shaders (sprite shaders, material shaders, screen effects, compute passes): each one's last compile or hot reload result (compiled, reloaded, failed, reload_failed: the last good version stays), reload count, frame, and its errors with line and column in the game's own WGSL, cause and GPU variant",
    ),
    (
        "perf.get",
        "frame timing over the last few seconds (fps; avg, p50, p95, p99, max ms; CPU split into systems, render and waiting on the display; GPU ms), GPU ms per pass, what was drawn (draw calls; 3D models and triangles for the camera and the shadows; render CPU split into collect, record, submit), the slowest systems, live asset handles by class with estimated bytes (`assets`: textures, meshes, materials, sounds, ...; with what the GPU holds), residency budgets per asset kind (`residency`: budget, resident and pinned counts and bytes, loads, reloads, evictions, evicted bytes), streaming (`streaming`: the upload queue's budget, pending count, bytes and uploads last frame and in total, oversized steps, decode waits, uploads held by a budget, failures, last and longest step ms; and `assets`, every streamed or resident asset with kind, name, state (loading, uploading, streaming, resident, evicted, failed), id and GPU size); params: meshes? (list the n meshes drawing the most triangles, default 0), profiling? (true turns on scope timing and allocation counting, false turns it off), history? (list the last n drawn frames in render_inspector.history, at most 120, default 0). While profiling is on, the answer also has `report`: frame percentiles, every system and profile_scope with time and allocations per frame, the slowest frames and what was in them, and warnings with advice. Always has `render_inspector.quality_autotune` (null when off; otherwise active choices, pins, frozen state, reason and the last 16 decisions) and `render_inspector` (schema 1, also when headless): the last frame's CPU time split into extract, prepare, encode, submit and other ms; GPU ms per frame and per pass; draw calls, models, triangles, occluded models, plain models outside the camera's view, pipelines built, and counts.state_changes (model-pass pipeline, bind group, vertex/index buffer changes and recorded draws); upload bytes; memory_estimate_bytes (what the renderer's tables hold for the game's assets, not its own frame targets or shadow maps); the GPU adapter and tier; with history, per frame CPU ms, smoothed GPU ms, the newest single-frame GPU ms with its age, counts and uploads; the active graphics quality preset and settings. A metric that can't be measured (no timestamp queries, headless, not counted) is null and `unavailable` gives the reason; GPU times are never guessed",
    ),
    (
        "perf.trace",
        "record the next frames as a Chrome/Perfetto trace (systems, render, display wait, GPU passes; windowed games only); params: path? (a new .json file relative to the game's folder; default: a temp file), frames? (120). Returns the path; it's written when the frames are done",
    ),
    (
        "perf.hud",
        "show or hide the on-screen performance overlay; params: on (bool)",
    ),
    (
        "quality.set",
        "switch the 3D graphics quality preset; params: level (low, medium, high, ultra or recommended)",
    ),
    ("game.quit", "ask the game to exit"),
];

/// `perf.get`'s `render_inspector`: the last frame's rendering cost in a
/// versioned schema (see `control/inspector.rs`).
fn render_inspector_json(ctx: &Ctx, history: usize) -> Value {
    let source = match (ctx.headless, ctx.perf.last_render_detail) {
        (true, _) => inspector::Source::Headless,
        (false, None) => inspector::Source::NotDrawn,
        (false, Some(detail)) => inspector::Source::Drawn(detail),
    };
    #[cfg(feature = "3d")]
    let quality = Some(inspector::quality_json(&ctx.scene3d));
    #[cfg(not(feature = "3d"))]
    let quality = None;
    // The newest `history` frames, oldest first.
    let frames: Vec<crate::perf::RenderFrame> = {
        let all = &ctx.perf.render_history;
        all.iter().skip(all.len().saturating_sub(history)).copied().collect()
    };
    #[cfg(feature = "3d")]
    let frustum_culled = ctx.gfx.frame3d.frustum_culled_models().ok_or(inspector::FRUSTUM_STALE);
    #[cfg(not(feature = "3d"))]
    let frustum_culled = Err(NO_3D_MODELS);
    let mut report = inspector::render_inspector(&inspector::Inputs {
        source,
        stats: ctx.perf.render_stats(),
        passes: ctx.perf.gpu_passes(),
        quality,
        pipelines: ctx.gfx.pipeline_stats(),
        memory: ctx.gfx.renderer.as_ref().and_then(crate::render::Renderer::residency),
        adapter: ctx.gfx.gpu_capabilities(),
        history: &frames,
        frustum_culled,
    });
    #[cfg(feature = "3d")]
    let (autotune, off) = (
        json!(ctx.quality_autotune_status()),
        "automatic quality tuning is off (SceneSettings::set_quality_autotune turns it on)",
    );
    #[cfg(not(feature = "3d"))]
    let (autotune, off) = (Value::Null, "automatic quality tuning needs the `3d` feature");
    if autotune.is_null() {
        report["unavailable"]["quality_autotune"] = json!(off);
    }
    report["quality_autotune"] = autotune;
    report
}

/// `quality.set`: switches to a graphics quality preset, fitted to what the
/// GPU can do, and answers with the settings before and after, so a tool
/// can compare `perf.get` across the change.
#[cfg(feature = "3d")]
fn quality_set(ctx: &mut Ctx, params: &Value) -> Result<Value, String> {
    use crate::render::GraphicsQuality;
    let level = str_param(params, "level")?;
    let caps = ctx.gfx.gpu_capabilities().cloned();
    let wanted = match level {
        "low" => Some(GraphicsQuality::Low),
        "medium" => Some(GraphicsQuality::Medium),
        "high" => Some(GraphicsQuality::High),
        "ultra" => Some(GraphicsQuality::Ultra),
        "recommended" if caps.is_some() => None,
        "recommended" if ctx.headless => return Err("`recommended` needs a GPU: this game runs headless".into()),
        "recommended" => {
            return Err(
                "`recommended` needs a GPU: the renderer hasn't opened one yet; try again after the first frame".into(),
            );
        }
        other => {
            return Err(format!(
                "unknown quality level `{other}` (low, medium, high, ultra or recommended)"
            ));
        }
    };
    let previous = inspector::quality_json(&ctx.scene3d);
    // Nothing the GPU lacks is asked for.
    let fitted_to_gpu = match (wanted, &caps) {
        (Some(quality), caps) => {
            ctx.scene3d.set_quality(quality);
            let preset = inspector::quality_json(&ctx.scene3d);
            if let Some(caps) = caps {
                ctx.scene3d.fit_quality_to(caps);
            }
            inspector::quality_json(&ctx.scene3d) != preset
        }
        (None, Some(caps)) => {
            ctx.scene3d.set_recommended_quality(caps);
            false
        }
        (None, None) => unreachable!("`recommended` without a GPU returned above"),
    };
    Ok(json!({
        "requested": level,
        "fitted_to_gpu": fitted_to_gpu,
        "previous": previous,
        "quality": inspector::quality_json(&ctx.scene3d),
    }))
}

#[cfg(not(feature = "3d"))]
fn quality_set(_: &mut Ctx, _: &Value) -> Result<Value, String> {
    Err(NO_QUALITY.into())
}

/// Why `counts.frustum_culled_models` is null without the `3d` feature.
#[cfg(not(feature = "3d"))]
const NO_3D_MODELS: &str = "3D models need the `3d` feature";

/// Why `quality.set` isn't there without the `3d` feature.
#[cfg(not(feature = "3d"))]
pub(crate) const NO_QUALITY: &str = "graphics quality settings need the `3d` feature";

/// `perf.get`: the profiler's numbers as JSON.
fn perf_json(ctx: &Ctx, meshes: usize, history: usize) -> Value {
    if ctx.headless {
        let slowest = slowest_systems(ctx);
        return json!({ "headless": true, "systems": slowest, "residency": residency_json(ctx), "streaming": streaming_json(ctx), "render_inspector": render_inspector_json(ctx, history) });
    }
    let f = ctx.perf.frame_stats();
    let r = ctx.perf.render_stats();
    let passes: Vec<Value> = {
        let mut p: Vec<_> = ctx.perf.gpu_passes().to_vec();
        p.sort_by(|a, b| b.ms.total_cmp(&a.ms));
        p.iter().map(|p| json!({ "name": p.name, "ms": p.ms })).collect()
    };
    #[allow(unused_mut)]
    let mut out = json!({
        "frames": f.frames,
        "fps": f.fps,
        "frame_ms": { "avg": f.avg_ms, "p50": f.p50_ms, "p95": f.p95_ms, "p99": f.p99_ms, "max": f.max_ms },
        "cpu_ms": { "systems": f.systems_ms, "render": f.render_cpu_ms, "display_wait": f.display_wait_ms },
        "gpu_ms": f.gpu_ms,
        "gpu_passes": passes,
        "render": {
            "draw_calls": r.draw_calls,
            "sprites": r.sprites,
            "collect_ms": r.collect_ms,
            "record_ms": r.record_ms,
            "submit_ms": r.submit_ms,
            "models": r.instances,
            "triangles": r.triangles,
            "shadow_models": r.shadow_instances,
            "shadow_triangles": r.shadow_triangles,
            "uploads": r.uploads.json(),
        },
        "systems": slowest_systems(ctx),
        "residency": residency_json(ctx),
        "streaming": streaming_json(ctx),
        "render_inspector": render_inspector_json(ctx, history),
        "hint": "frame time is about max(CPU, GPU): when display_wait is large the GPU (or vsync) is the limit; gpu_passes says which pass",
    });
    #[cfg(feature = "3d")]
    if meshes > 0 {
        out["meshes"] = ctx
            .gfx
            .mesh_costs()
            .iter()
            .take(meshes)
            .map(|c| {
                json!({
                    "name": c.name,
                    "models": c.models,
                    "triangles": c.triangles,
                    "shadow_models": c.shadow_models,
                    "shadow_triangles": c.shadow_triangles,
                })
            })
            .collect();
    }
    #[cfg(not(feature = "3d"))]
    let _ = meshes;
    out
}

/// `Graphics::shader_status` as JSON.
pub(crate) fn shader_status_json(gfx: &crate::gfx::Graphics) -> Value {
    use crate::render::ShaderKind;
    use crate::render::ShaderOutcome;
    let shaders: Vec<Value> = gfx
        .shader_status()
        .iter()
        .map(|s| {
            let kind = match s.kind {
                ShaderKind::Material => "material",
                ShaderKind::Effect => "effect",
                ShaderKind::Sprite => "sprite",
                _ => "compute",
            };
            let outcome = match s.outcome {
                ShaderOutcome::Compiled => "compiled",
                ShaderOutcome::Reloaded => "reloaded",
                ShaderOutcome::Failed => "failed",
                _ => "reload_failed",
            };
            let diagnostics: Vec<Value> = s
                .diagnostics
                .iter()
                .map(|d| {
                    json!({
                        "line": d.line,
                        "column": d.column,
                        "in_engine_code": d.in_engine_code,
                        "message": d.message,
                        "label": d.label,
                        "notes": d.notes,
                        "source_line": d.source_line,
                        "variant": d.variant,
                        "text": d.to_string(),
                    })
                })
                .collect();
            json!({
                "shader": s.shader,
                "kind": kind,
                "outcome": outcome,
                "ok": s.is_ok(),
                "reloads": s.reloads,
                "frame": s.frame,
                "diagnostics": diagnostics,
            })
        })
        .collect();
    json!({ "shaders": shaders })
}

/// Most pairs `ecs.diagnostics` lists in `schedule.dependencies`: the
/// pairs grow with the square of the declared systems in a phase.
const MAX_DEPENDENCIES: usize = 1000;

/// `ecs.diagnostics`: [`Ctx::ecs_diagnostics`] as JSON, with the
/// archetype and dependency lists capped and an optional query match count.
fn ecs_diagnostics_json(ctx: &Ctx, params: &Value) -> Result<Value, String> {
    let limit = usize_param(params, "limit", 100)?.min(MAX_LIST);
    let with: Option<Vec<String>> = match params.get("with") {
        None | Some(Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|_| "`with` must be a list of component names")?),
    };
    let mut d = ctx.ecs_diagnostics();
    let matching = match &with {
        Some(with) => Some(d.matching(&with.iter().map(String::as_str).collect::<Vec<_>>())?),
        None => None,
    };
    let total = d.archetypes.len();
    d.archetypes.retain(|a| a.entities > 0);
    d.archetypes.truncate(limit);
    let dependencies = d.schedule.dependencies.len();
    d.schedule.dependencies.truncate(MAX_DEPENDENCIES);
    let mut out = serde_json::to_value(&d).map_err(|e| e.to_string())?;
    out["archetypes_total"] = json!(total);
    out["schedule"]["dependencies_total"] = json!(dependencies);
    if let (Some(with), Some(m)) = (with, matching) {
        out["matching"] = json!({ "with": with, "archetypes": m.archetypes, "entities": m.entities });
    }
    Ok(out)
}

/// The upload queue and every streamed asset's state.
fn streaming_json(ctx: &Ctx) -> Value {
    let u = ctx.gfx.upload_stats();
    let assets: Vec<Value> = ctx
        .gfx
        .streamed_assets()
        .into_iter()
        .map(|a| {
            json!({
                "kind": a.kind.name(),
                "name": a.name,
                "state": a.state.name(),
                "id": a.id,
                "size": a.size,
            })
        })
        .collect();
    json!({
        "uploads": {
            "budget": u.budget,
            "pending": u.pending,
            "frame_bytes": u.frame_bytes,
            "frame_uploads": u.frame_uploads,
            "total_bytes": u.total_bytes,
            "total_uploads": u.total_uploads,
            "oversized": u.oversized,
            "decode_waits": u.decode_waits,
            "held_by_budget": u.held_by_budget,
            "failures": u.failures,
            "step_ms": u.step_ms,
            "max_step_ms": u.max_step_ms,
        },
        "assets": assets,
    })
}

/// Residency budgets and counters, by kind name.
fn residency_json(ctx: &Ctx) -> Value {
    crate::streaming::ResidencyKind::ALL
        .iter()
        .map(|&kind| {
            let r = ctx.gfx.residency(kind);
            let value = json!({
                "budget": r.budget,
                "resident": r.resident,
                "resident_bytes": r.resident_bytes,
                "over_budget_bytes": r.over_budget_bytes(),
                "pinned": r.pinned,
                "pinned_bytes": r.pinned_bytes,
                "loads": r.loads,
                "reloads": r.reloads,
                "evictions": r.evictions,
                "evicted_bytes": r.evicted_bytes,
            });
            (kind.name().to_string(), value)
        })
        .collect::<serde_json::Map<_, _>>()
        .into()
}

/// The 10 systems with the most average time.
fn slowest_systems(ctx: &Ctx) -> Value {
    let mut t: Vec<_> = ctx
        .perf
        .system_timings()
        .iter()
        .filter(|t| t.sim || !ctx.headless)
        .collect();
    t.sort_by(|a, b| b.avg_ms.total_cmp(&a.avg_ms));
    t.iter()
        .take(10)
        .map(|t| json!({ "name": t.name, "avg_ms": t.avg_ms }))
        .collect()
}

fn param<'a>(params: &'a Value, name: &str) -> Result<&'a Value, String> {
    params.get(name).ok_or_else(|| format!("missing param `{name}`"))
}

fn entity_param(params: &Value) -> Result<Entity, String> {
    param(params, "entity")?
        .as_u64()
        .map(Entity::from_bits)
        .ok_or_else(|| "`entity` must be an entity id (a non-negative integer)".into())
}

fn str_param<'a>(params: &'a Value, name: &str) -> Result<&'a str, String> {
    param(params, name)?
        .as_str()
        .ok_or_else(|| format!("`{name}` must be a string"))
}

fn usize_param(params: &Value, name: &str, default: usize) -> Result<usize, String> {
    match params.get(name) {
        None | Some(Value::Null) => Ok(default),
        Some(v) => v
            .as_u64()
            .map(|n| n as usize)
            .ok_or_else(|| format!("`{name}` must be a non-negative integer")),
    }
}

/// The `capabilities` answer as JSON text (see `Ctx::capabilities_json`).
pub(crate) fn capabilities_json(ctx: &Ctx) -> String {
    capabilities::report(ctx).to_string()
}

pub(crate) fn handle(ctx: &mut Ctx, method: &str, params: &Value) -> Result<Value, String> {
    let err = |e: crate::reflect::ReflectError| e.to_string();
    if let Some(reason) = capabilities::unavailable(ctx, method) {
        return Err(reason);
    }
    Ok(match method {
        "capabilities" => capabilities::report(ctx),
        "methods" => Value::Array(
            METHODS
                .iter()
                .map(|(name, about)| json!({ "name": name, "about": about }))
                .collect(),
        ),
        "info" => json!({
            "pid": std::process::id(),
            "headless": ctx.headless,
            "tick": ctx.time.sim_tick(),
            "sim_time": ctx.time.sim_time(),
            "paused": ctx.time.is_paused(),
            "speed": ctx.time.speed(),
            "fixed_dt": ctx.time.fixed_dt(),
            // Headless games have no frames, so no frame rate.
            "fps": if ctx.headless { Value::Null } else { json!(ctx.time.fps()) },
            "frame": if ctx.headless { Value::Null } else { json!(ctx.time.frame()) },
            "entities": ctx.world.len(),
            "render": if ctx.headless { Value::Null } else {
                let r = ctx.perf.render_stats();
                json!({ "sprites": r.sprites, "draw_calls": r.draw_calls, "cpu_ms": r.cpu_ms, "gpu_ms": r.gpu_ms })
            },
        }),
        "logs.get" => logs_get(params)?,
        "world.summary" => {
            let world = &ctx.world;
            let archetypes: Vec<Value> = world
                .raw_ref()
                .archetypes()
                .iter()
                .filter(|a| !a.is_empty())
                .map(|a| {
                    let names: Vec<&str> = a
                        .component_ids()
                        .iter()
                        .map(|&id| short(world.component_info(id).name()))
                        .collect();
                    json!({ "components": names, "entities": a.len(), "bytes_per_entity": a.bytes_per_entity() })
                })
                .collect();
            json!({
                "entities": world.len(),
                "allocated_bytes": world.allocated_bytes(),
                "reflected_components": world.reflected_components(),
                "reflected_resources": world.reflected_resources(),
                "reflected_events": ctx.events.reflected_json(world),
                "archetypes": archetypes,
            })
        }
        "entities.list" => {
            let limit = usize_param(params, "limit", 100)?.min(MAX_LIST);
            let offset = usize_param(params, "offset", 0)?;
            let world = &ctx.world;
            let reflected = world.reflected_components();
            let (mut total, mut out) = (0, Vec::new());
            for_each_matching(world, params, &[], |e, names, _| {
                if total >= offset && out.len() < limit {
                    let components = world.entity_json(e).map_err(err)?;
                    let other: Vec<&str> = names.iter().copied().filter(|n| !reflected.contains(n)).collect();
                    out.push(json!({ "entity": e.to_bits(), "components": components, "other_components": other }));
                }
                total += 1;
                Ok(())
            })?;
            json!({ "total": total, "offset": offset, "entities": out })
        }
        "entities.stats" => {
            let fields = query::list(params, "fields", FieldPath::parse)?;
            if fields.is_empty() {
                return Err("missing param `fields` (like [\"Health.current\", \"Team\"])".into());
            }
            let mut stats: Vec<FieldStats> = fields.iter().map(|_| FieldStats::default()).collect();
            let mut count = 0u64;
            for_each_matching(&ctx.world, params, &fields, |_, _, found| {
                count += 1;
                for (f, s) in fields.iter().zip(&mut stats) {
                    if let Some(v) = found.get(&f.component).and_then(|c| f.get(c)) {
                        s.add(v);
                    }
                }
                Ok(())
            })?;
            let by_field: serde_json::Map<String, Value> = fields
                .iter()
                .zip(&stats)
                .map(|(f, s)| (f.text().to_string(), s.to_json()))
                .collect();
            json!({ "entities": count, "fields": by_field })
        }
        "entity.get" => ctx.world.entity_json(entity_param(params)?).map_err(err)?,
        "entity.spawn" => {
            let components = param(params, "components")?.clone();
            ctx.load_scene_assets(&components)?;
            let e = ctx.world.spawn_json(components).map_err(err)?;
            json!({ "entity": e.to_bits() })
        }
        "entity.despawn" => json!(ctx.world.despawn(entity_param(params)?)),
        "component.set" => {
            let e = entity_param(params)?;
            let name = str_param(params, "component")?;
            let value = param(params, "value")?.clone();
            ctx.load_scene_assets(&json!({ name: &value }))?;
            ctx.world.set_json(e, name, value).map_err(err)?;
            ctx.world.get_json(e, name).map_err(err)?
        }
        "component.remove" => {
            let e = entity_param(params)?;
            json!(ctx.world.remove_json(e, str_param(params, "component")?).map_err(err)?)
        }
        "resources.list" => {
            let world = &ctx.world;
            let mut out = serde_json::Map::new();
            for name in world.reflected_resources() {
                let value = match world.resource_json(name) {
                    Ok(v) => v,
                    Err(crate::reflect::ReflectError::MissingResource(_)) => Value::Null,
                    Err(e) => return Err(err(e)),
                };
                out.insert(name.to_string(), value);
            }
            Value::Object(out)
        }
        "resource.get" => ctx.world.resource_json(str_param(params, "resource")?).map_err(err)?,
        "resource.set" => {
            let name = str_param(params, "resource")?;
            ctx.world
                .set_resource_json(name, param(params, "value")?.clone())
                .map_err(err)?;
            ctx.world.resource_json(name).map_err(err)?
        }
        "world.save" => {
            let path = str_param(params, "path")?;
            let overwrite = match params.get("overwrite") {
                None | Some(Value::Null) => false,
                Some(v) => v.as_bool().ok_or("`overwrite` must be true or false")?,
            };
            // Checked before the world is serialized, which can be slow.
            check_tool_path(path, "json")?;
            let save = ctx.world.save();
            let text = serde_json::to_string(&save).map_err(|e| e.to_string())?;
            let path = write_game_file(path, "json", text.as_bytes(), overwrite)?;
            json!({
                "path": path,
                "entities": save["entities"].as_array().map_or(0, Vec::len),
                "resources": save["resources"].as_object().map_or(0, |r| r.len()),
                "bytes": text.len(),
                "not_saved": save["not_saved"],
                "tick": ctx.time.sim_tick(),
            })
        }
        "scene.save" => {
            let path = asset_path_param(params)?;
            ctx.assets.save_scene(&ctx.world, path)?;
            ctx.scene_file = Some(path.to_string());
            json!({ "path": path, "entities": ctx.world.len() })
        }
        "scene.load" => {
            let path = asset_path_param(params)?;
            ctx.load_scene(path)?;
            json!({ "path": path, "entities": ctx.world.len() })
        }
        "prefab.spawn" => {
            let path = asset_path_param(params)?;
            let at: [f32; 2] = serde_json::from_value(param(params, "at")?.clone())
                .map_err(|e| format!("`at` must be [x, y]: {e}"))?;
            let e = ctx.spawn_prefab(path, glam::vec2(at[0], at[1]))?;
            let parts: Vec<u64> = ctx
                .world
                .get::<crate::scene::PrefabParts>(e)
                .map(|p| p.0.iter().map(|p| p.to_bits()).collect())
                .unwrap_or_default();
            json!({ "entity": e.to_bits(), "parts": parts })
        }
        "prefab.despawn" => json!(ctx.despawn_prefab(entity_param(params)?)),
        "editor.pick" => editor::pick(ctx, params)?,
        "editor.transform" => editor::transform(ctx, params)?,
        "editor.drag" => editor::drag(ctx, params)?,
        "editor.parent" => editor::parent(ctx, params)?,
        "editor.hierarchy" => editor::hierarchy(ctx),
        "editor.assets" => editor::assets(ctx),
        "editor.place" => editor::place(ctx, params)?,
        "editor.material" => editor::material(ctx, params)?,
        "editor.delete" => editor::delete(ctx, params)?,
        "editor.duplicate" => editor::duplicate(ctx, params)?,
        "editor.undo" => editor::undo(ctx, false)?,
        "editor.redo" => editor::undo(ctx, true)?,
        "editor.save" => editor::save(ctx, params)?,
        "editor.camera" => editor::camera(ctx, params)?,
        "prefab.save" => {
            let e = entity_param(params)?;
            let path = asset_path_param(params)?;
            ctx.assets.save_prefab(&ctx.world, e, path)?;
            json!({ "path": path })
        }
        "world.load" => {
            let path = game_file(str_param(params, "path")?, "json")?;
            let text = std::fs::read_to_string(&path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
            let save: Value =
                serde_json::from_str(&text).map_err(|e| format!("{} is not JSON: {e}", path.display()))?;
            ctx.replace_world(&save)?;
            json!({ "path": path, "entities": ctx.world.len() })
        }
        "sim.pause" => {
            ctx.time.pause();
            json!({ "paused": true, "tick": ctx.time.sim_tick() })
        }
        "sim.resume" => {
            ctx.time.resume();
            json!({ "paused": false, "tick": ctx.time.sim_tick() })
        }
        "sim.step" => {
            let ticks = usize_param(params, "ticks", 1)?;
            let ticks = u32::try_from(ticks).map_err(|_| "`ticks` is too large")?;
            for _ in 0..ticks {
                ctx.time.step();
            }
            json!({ "queued_ticks": ticks, "tick": ctx.time.sim_tick() })
        }
        "sim.speed" => {
            let speed = param(params, "speed")?
                .as_f64()
                .filter(|s| s.is_finite() && *s >= 0.0)
                .ok_or("`speed` must be a finite number >= 0")?;
            ctx.time.set_speed(speed as f32);
            json!({ "speed": ctx.time.speed() })
        }
        "input.scenario.load" => {
            let mut scenario =
                crate::scenario::InputScenario::from_value(param(params, "scenario")?).map_err(|e| e.to_string())?;
            let flag = |name: &str| -> Result<bool, String> {
                match params.get(name) {
                    None | Some(Value::Null) => Ok(false),
                    Some(v) => v.as_bool().ok_or_else(|| format!("`{name}` must be true or false")),
                }
            };
            let (bind, keep, queue) = (
                flag("bind_connection")?,
                flag("keep_input_when_done")?,
                flag("queue_ticks")?,
            );
            // Checked before anything is installed: a refused scenario leaves a running one alone.
            let ticks = scenario.ticks();
            if ticks == 0 {
                return Err("the scenario has no steps, so there is nothing to run".into());
            }
            if let Some(max) = params.get("max_ticks").filter(|v| !v.is_null()) {
                let max = max.as_u64().ok_or("`max_ticks` must be a whole number")?;
                if ticks > max {
                    return Err(format!(
                        "the scenario runs {ticks} ticks, more than `max_ticks` ({max})"
                    ));
                }
            }
            if queue && !ctx.time.is_paused() {
                return Err("`queue_ticks` needs a paused game (call sim.pause first)".into());
            }
            // A tool's scenario lets go of its input when it ends, unless told to keep it.
            scenario.release_unless_kept(keep);
            let owner = params.get("_connection").and_then(Value::as_u64).filter(|_| bind);
            let start = ctx.time.sim_tick();
            ctx.input.install_scenario(
                &mut ctx.world,
                &mut ctx.time,
                scenario,
                start,
                crate::scenario::RunOrigin {
                    owner,
                    tool: true,
                    queue_ticks: queue,
                },
            );
            json!({ "report": ctx.input.scenario_report(&ctx.world), "bound_to_connection": owner.is_some() })
        }
        "input.scenario.status" => json!(ctx.input.scenario_report(&ctx.world)),
        "input.scenario.cancel" => {
            let reason = params
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("cancelled by a tool");
            let owner = params.get("connection").and_then(Value::as_u64);
            let cancelled = ctx
                .input
                .cancel_scenario_for(&mut ctx.world, &mut ctx.time, reason, owner);
            json!({ "cancelled": cancelled, "report": ctx.input.scenario_report(&ctx.world) })
        }
        "systems.timings" => Value::Array(
            ctx.perf
                .system_timings()
                .iter()
                // Headless games run only simulation systems.
                .filter(|t| t.sim || !ctx.headless)
                .map(|t| json!({ "name": t.name, "sim": t.sim, "last_ms": t.last_ms, "avg_ms": t.avg_ms }))
                .collect(),
        ),
        "ecs.diagnostics" => ecs_diagnostics_json(ctx, params)?,
        "perf.get" => {
            if let Some(on) = params.get("profiling") {
                ctx.perf
                    .set_profiling(on.as_bool().ok_or("`profiling` must be true or false")?);
            }
            let history = usize_param(params, "history", 0)?.min(crate::perf::RENDER_HISTORY);
            let mut out = perf_json(ctx, usize_param(params, "meshes", 0)?, history);
            out["assets"] = serde_json::to_value(ctx.asset_inventory()).unwrap_or_default();
            if ctx.perf.profiling() {
                let report = ctx.perf.report();
                out["report"] = serde_json::to_value(&report).unwrap_or_default();
                out["report_text"] = json!(report.to_string());
            }
            out
        }
        "perf.trace" => {
            let frames = usize_param(params, "frames", 120)?.clamp(1, 10_000);
            // Checked before any file or folder is made.
            if ctx.perf.headless {
                return Err(crate::perf::HEADLESS_TRACE.into());
            }
            let claimed = match params.get("path") {
                None | Some(Value::Null) => new_temp_file("trace", "json")?,
                Some(Value::String(p)) => new_game_file(p, "json")?,
                Some(_) => return Err("`path` must be a string".into()),
            };
            let path = claimed.path.clone();
            ctx.perf.record_claimed_trace(claimed, frames)?;
            json!({ "path": path, "frames": frames, "written_after_frame": ctx.time.frame() + frames as u64 })
        }
        "perf.hud" => {
            let on = param(params, "on")?.as_bool().ok_or("`on` must be true or false")?;
            ctx.perf.set_hud(on);
            json!(on)
        }
        "quality.set" => quality_set(ctx, params)?,
        "game.quit" => {
            ctx.quit();
            json!(true)
        }
        "events.list" => Value::Array(ctx.events.reflected_json(&ctx.world)),
        "events.subscribe" => {
            let types: Vec<String> = match params.get("types") {
                None | Some(Value::Null) => Vec::new(),
                Some(v) => {
                    serde_json::from_value(v.clone()).map_err(|_| "`types` must be a list of event type names")?
                }
            };
            let capacity = match params.get("capacity") {
                None | Some(Value::Null) => 1000,
                Some(v) => v.as_u64().ok_or("`capacity` must be a whole number")?.min(100_000) as usize,
            };
            ctx.events.subscribe(&ctx.world, types.clone(), capacity);
            json!({ "types": types, "capacity": capacity })
        }
        "events.take" => {
            let feed = ctx
                .events
                .feed
                .as_mut()
                .ok_or("not subscribed: call events.subscribe first")?;
            let events: Vec<Value> = feed.items.drain(..).collect();
            json!({ "events": events, "dropped": std::mem::take(&mut feed.dropped) })
        }
        "events.unsubscribe" => json!(ctx.events.feed.take().is_some()),
        "camera.get" => camera_json(ctx),
        "shaders.status" => shader_status_json(&ctx.gfx),
        "camera.set" => {
            let vec = |name: &str| -> Result<Option<Vec2>, String> {
                match params.get(name) {
                    None | Some(Value::Null) => Ok(None),
                    Some(v) => serde_json::from_value::<[f32; 2]>(v.clone())
                        .map(|[x, y]| Some(Vec2::new(x, y)))
                        .map_err(|_| format!("`{name}` must be [x, y]")),
                }
            };
            let (position, look_at) = (vec("position")?, vec("look_at")?);
            let zoom = match params.get("zoom") {
                None | Some(Value::Null) => None,
                Some(v) => Some(
                    v.as_f64()
                        .filter(|z| *z > 0.0)
                        .ok_or("`zoom` must be a positive number")? as f32,
                ),
            };
            if let Some(p) = position {
                ctx.camera.position = p;
            }
            if let Some(z) = zoom {
                let (lo, hi) = crate::d2::Camera::ZOOM_LIMITS;
                ctx.camera.zoom = z.clamp(lo, hi);
            }
            if let Some(p) = look_at {
                ctx.camera.look_at(p);
            }
            camera_json(ctx)
        }
        // Windowed games answer it after the next frame (see `answer`);
        // headless ones were refused above.
        "screenshot" => return Err("screenshots are taken after the next frame".into()),
        other => return Err(format!("unknown method `{other}` (call `methods` for the list)")),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::d2::Transform as Transform2D;
    use glam::vec2;
    use ponko_ecs::Component;

    fn ctx() -> Ctx {
        Ctx::new(1.0 / 60.0, glam::Vec2::ZERO)
    }

    /// A game that is asked to quit drops its endpoint and exits; the
    /// connection threads die with the process. So once the endpoint's exit
    /// flush returns, the reply must have been written: nothing is owed.
    #[test]
    fn the_reply_to_quit_is_written_before_the_endpoint_is_gone() {
        for round in 0..200 {
            let control = Control::start_with("quit-reply", true, false).unwrap();
            let mut client = TcpStream::connect(control.addr()).unwrap();
            let line = json!({ "id": round, "token": &*control.token, "method": "game.quit", "params": {} });
            writeln!(client, "{line}").unwrap();
            // The game loop: answer what arrives, and leave once told to.
            let mut ctx = ctx();
            while !ctx.quit_requested() {
                control.wait(&mut ctx, Duration::from_secs(5));
            }
            // What dropping the endpoint does first. Without it the game
            // would be gone while the connection thread still holds the
            // reply.
            control.flush_replies();
            assert_eq!(
                *control.owed.count(),
                0,
                "round {round}: the reply to game.quit was not written when the game left"
            );
            drop(control);
            client.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut reply = String::new();
            BufReader::new(&client).read_line(&mut reply).unwrap();
            let reply: Value = serde_json::from_str(&reply).unwrap_or_else(|e| panic!("round {round}: {reply:?}: {e}"));
            assert_eq!(reply, json!({ "id": round, "result": true }), "round {round}");
        }
    }

    /// A request the game never got to is refused when the game exits,
    /// instead of leaving its client waiting for an answer.
    #[test]
    fn a_request_still_queued_when_the_game_exits_is_refused_not_left_waiting() {
        let control = Control::start_with("queued", true, false).unwrap();
        let mut client = TcpStream::connect(control.addr()).unwrap();
        let line = json!({ "id": 1, "token": &*control.token, "method": "sim.status", "params": {} });
        writeln!(client, "{line}").unwrap();
        // Until the connection thread has handed the request over.
        while *control.owed.count() == 0 {
            thread::yield_now();
        }
        let started = std::time::Instant::now();
        drop(control);
        assert!(started.elapsed() < EXIT_FLUSH_TIMEOUT, "the exit waited out its limit");
        let mut reply = String::new();
        BufReader::new(client).read_line(&mut reply).unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply, json!({ "id": 1, "error": "the game is shutting down" }));
    }

    /// A windowed screenshot is answered after the next frame; a game that
    /// exits first drops the request with its graphics. The client is told
    /// at once, and the exit doesn't wait for an answer that can't come.
    #[test]
    fn a_request_the_game_let_go_of_is_refused_at_once_and_does_not_hold_up_the_exit() {
        let control = Control::start_with("dropped", true, false).unwrap();
        let mut client = TcpStream::connect(control.addr()).unwrap();
        let line = json!({ "id": 4, "token": &*control.token, "method": "screenshot", "params": {} });
        writeln!(client, "{line}").unwrap();
        // The game takes the request and keeps it for later, then exits:
        // whatever held it is dropped before the endpoint.
        let held = control.requests.recv_timeout(Duration::from_secs(5)).unwrap();
        let started = std::time::Instant::now();
        drop(held);
        drop(control);
        assert!(
            started.elapsed() < EXIT_FLUSH_TIMEOUT / 2,
            "the exit waited for a reply that could not come"
        );
        let mut reply = String::new();
        BufReader::new(client).read_line(&mut reply).unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert_eq!(reply, json!({ "id": 4, "error": "the game is shutting down" }));
    }

    #[test]
    fn every_listed_method_is_handled() {
        let mut ctx = ctx();
        for (name, _) in METHODS {
            if *name == "game.quit" || *name == "screenshot" {
                continue;
            }
            let r = handle(&mut ctx, name, &json!({}));
            if let Err(e) = r {
                let uncaptured = *name == "logs.get" && e.contains("captured");
                let refused = *name == "input.scenario.load" && e.contains("input injection is off");
                let no_3d = (*name == "quality.set" || name.starts_with("editor.")) && e.contains("`3d` feature");
                assert!(
                    e.starts_with("missing param") || uncaptured || refused || no_3d,
                    "{name}: {e}"
                );
            }
        }
        assert!(
            handle(&mut ctx, "nope", &json!({}))
                .unwrap_err()
                .contains("unknown method")
        );
    }

    #[cfg(all(feature = "3d", feature = "window"))]
    #[test]
    fn shader_status_reports_errors_in_the_games_lines() {
        let mut ctx = ctx();
        assert_eq!(
            handle(&mut ctx, "shaders.status", &json!({})).unwrap()["shaders"],
            json!([])
        );
        let _ = ctx.gfx.try_create_shader(
            "red.wgsl",
            "// red\nfn surface(s: Surface) -> Surface {\n    return 1.0;\n}",
        );
        let r = handle(&mut ctx, "shaders.status", &json!({})).unwrap();
        let s = &r["shaders"][0];
        assert_eq!(
            (s["shader"].clone(), s["kind"].clone()),
            (json!("red.wgsl"), json!("material"))
        );
        assert_eq!((s["outcome"].clone(), s["ok"].clone()), (json!("failed"), json!(false)));
        let d = &s["diagnostics"][0];
        assert_eq!((d["line"].clone(), d["column"].clone()), (json!(3), json!(12)), "{d}");
        assert!(d["text"].as_str().unwrap().starts_with("red.wgsl:3:12: error:"), "{d}");
    }

    #[test]
    fn entity_methods_round_trip() {
        let mut ctx = ctx();
        let id = handle(
            &mut ctx,
            "entity.spawn",
            &json!({ "components": { "Transform2D": { "position": [1.0, 2.0] } } }),
        )
        .unwrap()["entity"]
            .as_u64()
            .unwrap();
        let set = handle(
            &mut ctx,
            "component.set",
            &json!({ "entity": id, "component": "Transform2D", "value": { "rotation": 2.0 } }),
        )
        .unwrap();
        assert_eq!(set["position"], json!([1.0, 2.0]));
        assert_eq!(set["rotation"], json!(2.0));
        let list = handle(&mut ctx, "entities.list", &json!({ "with": ["Transform2D"] })).unwrap();
        assert_eq!(list["total"], 1);
        assert_eq!(
            handle(&mut ctx, "entity.despawn", &json!({ "entity": id })).unwrap(),
            json!(true)
        );
        assert!(handle(&mut ctx, "entity.get", &json!({ "entity": id })).is_err());
    }

    #[test]
    fn list_pages_and_filters() {
        let mut ctx = ctx();
        #[derive(Component)]
        struct Hidden;
        for i in 0..10 {
            ctx.world.spawn((Transform2D::at(vec2(i as f32, 0.0)),));
        }
        ctx.world.spawn((Hidden, Transform2D::default()));
        let page = handle(&mut ctx, "entities.list", &json!({ "limit": 3, "offset": 9 })).unwrap();
        assert_eq!(page["total"], 11);
        let got = page["entities"].as_array().unwrap();
        assert_eq!(got.len(), 2, "pages cross archetypes");
        assert_eq!(got[1]["other_components"], json!(["Hidden"]));
        assert!(got[1]["components"].get("Transform2D").is_some());
        let hidden = handle(&mut ctx, "entities.list", &json!({ "with": ["Hidden"] })).unwrap();
        assert_eq!(hidden["total"], 1, "filters accept unreflected names too");
        let none = handle(&mut ctx, "entities.list", &json!({ "with": ["Nope"] })).unwrap();
        assert_eq!(none["total"], 0);
    }

    #[test]
    fn sim_methods_drive_the_clock() {
        let mut ctx = ctx();
        handle(&mut ctx, "sim.pause", &json!({})).unwrap();
        assert!(ctx.time.is_paused());
        handle(&mut ctx, "sim.step", &json!({ "ticks": 3 })).unwrap();
        assert_eq!(ctx.take_pending_steps(), 3);
        handle(&mut ctx, "sim.speed", &json!({ "speed": 4.0 })).unwrap();
        assert_eq!(ctx.time.speed(), 4.0);
        assert!(handle(&mut ctx, "sim.speed", &json!({ "speed": -1.0 })).is_err());
    }

    #[test]
    fn perf_reports_list_the_frames_gpu_uploads_and_reallocations() {
        let mut ctx = ctx();
        ctx.perf.last_render_stats.uploads = crate::render::GpuUploads {
            instances: 640,
            uniforms: 80,
            buffers_created: 3,
            reallocations: 2,
            buffer_bytes_created: 4096,
            ..Default::default()
        };
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        let u = &perf["render"]["uploads"];
        assert_eq!(u["bytes"], 720, "{u}");
        assert_eq!(u["instances"], 640, "{u}");
        assert_eq!(u["buffers_created"], 3, "{u}");
        assert_eq!(u["reallocations"], 2, "{u}");
        assert_eq!(u["buffer_bytes_created"], 4096, "{u}");
        // Every count, by the name it has in Rust.
        assert_eq!(u.as_object().unwrap().len(), 14, "{u}");
    }

    #[test]
    fn perf_reports_put_the_slowest_systems_first() {
        let mut ctx = ctx();
        ctx.perf.timings = ["fast", "slow", "middle"]
            .iter()
            .zip([0.1, 3.0, 1.0])
            .map(|(&name, avg_ms)| crate::perf::SystemTiming {
                name,
                sim: false,
                last_ms: avg_ms,
                avg_ms,
            })
            .collect();
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        let names: Vec<_> = perf["systems"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["name"].clone())
            .collect();
        assert_eq!(names, [json!("slow"), json!("middle"), json!("fast")]);
        for key in [
            "fps",
            "frame_ms",
            "cpu_ms",
            "gpu_passes",
            "render",
            "assets",
            "render_inspector",
        ] {
            assert!(perf.get(key).is_some(), "{key} missing from {perf}");
        }
        // No renderer has drawn a frame: no made-up zeros.
        let inspector = &perf["render_inspector"];
        assert_eq!(inspector["schema"], 1);
        assert!(
            inspector["gpu_ms"].is_null() && inspector["unavailable"]["gpu_ms"].is_string(),
            "{inspector}"
        );
        assert!(handle(&mut ctx, "perf.hud", &json!({ "on": true })).unwrap() == json!(true));
        assert!(ctx.perf.hud());
        assert!(handle(&mut ctx, "perf.hud", &json!({})).is_err(), "`on` is required");
        assert!(perf.get("report").is_none(), "the deep report needs profiling on");
        let deep = handle(&mut ctx, "perf.get", &json!({ "profiling": true })).unwrap();
        assert!(ctx.perf.profiling() && deep["report"].is_object(), "{deep}");
        assert!(handle(&mut ctx, "perf.get", &json!({ "profiling": 3 })).is_err());
        ctx.perf.set_profiling(false);
    }

    #[cfg(feature = "3d")]
    #[test]
    fn render_inspector_adds_autotune_choices_reasons_and_history() {
        use crate::render::QualityAutotune;
        use crate::three::autotune::Sample;
        let mut ctx = ctx();
        let report = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert!(report["render_inspector"].get("quality_autotune").unwrap().is_null());
        ctx.scene3d.set_quality_autotune(Some(QualityAutotune::default()));
        for i in 1..=30 {
            ctx.update_quality_autotune(
                Some(Sample {
                    sequence: i,
                    ms: 30.0,
                    fresh: true,
                }),
                false,
            );
        }
        let report = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        let inspector = &report["render_inspector"];
        let auto = &inspector["quality_autotune"];
        assert_eq!(inspector["schema"], 1);
        assert_eq!(auto["changes"], 1);
        assert_eq!(auto["history"][0]["reason"], "over_budget");
        assert_eq!(auto["history"][0]["gpu_ms"], 30.0);
        assert_eq!(auto["render_scale"], inspector["quality"]["render_scale"]);
        ctx.set_headless(true);
        for _ in 0..400 {
            ctx.update_quality_autotune(None, false);
        }
        let report = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert_eq!(report["render_inspector"]["quality_autotune"]["reason"], "frozen");
        assert_eq!(report["render_inspector"]["quality_autotune"]["changes"], 1);
    }

    #[test]
    fn perf_get_lists_as_many_recent_frames_as_asked() {
        let mut ctx = ctx();
        for frame in 0..(crate::perf::RENDER_HISTORY as u64 + 5) {
            ctx.perf.last_render_stats.draw_calls = frame as usize;
            ctx.perf.push_render_frame(frame);
        }
        // Not listed unless asked for.
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert_eq!(perf["render_inspector"]["history"], json!([]));
        let perf = handle(&mut ctx, "perf.get", &json!({ "history": 3 })).unwrap();
        let frames: Vec<u64> = perf["render_inspector"]["history"]
            .as_array()
            .unwrap()
            .iter()
            .map(|f| f["frame"].as_u64().unwrap())
            .collect();
        let newest = crate::perf::RENDER_HISTORY as u64 + 4;
        assert_eq!(frames, [newest - 2, newest - 1, newest], "the newest, oldest first");
        assert_eq!(perf["render_inspector"]["history"][2]["draw_calls"], newest);
        // More than is kept gives what is kept: the ring doesn't grow.
        let all = handle(&mut ctx, "perf.get", &json!({ "history": 100_000 })).unwrap();
        let all = all["render_inspector"]["history"].as_array().unwrap().len();
        assert_eq!(all, crate::perf::RENDER_HISTORY);
        assert!(ctx.perf.render_history.capacity() >= crate::perf::RENDER_HISTORY);
        assert!(handle(&mut ctx, "perf.get", &json!({ "history": "all" })).is_err());
    }

    /// The null-with-reason rule on `perf.get`'s real answer, in every
    /// state a game can be asked in: headless, windowed before its first
    /// frame, after a drawn frame, with automatic quality tuning on.
    #[test]
    fn every_null_in_perf_gets_render_inspector_has_a_reason() {
        let check = |ctx: &mut Ctx, state: &str| {
            let perf = handle(ctx, "perf.get", &json!({ "history": 4 })).unwrap();
            let report = &perf["render_inspector"];
            assert!(report.is_object(), "{state}: {perf}");
            inspector::assert_nulls_explained(report);
            report.clone()
        };
        let mut headless = ctx();
        headless.set_headless(true);
        let r = check(&mut headless, "headless");
        assert!(r["quality_autotune"].is_null() && r["unavailable"]["quality_autotune"].is_string());

        let mut windowed = ctx();
        check(&mut windowed, "not drawn yet");
        // As `Ctx::render_frame` leaves it after a drawn frame (without
        // timestamp queries, so GPU time is unknown).
        windowed.perf.last_render_detail = Some(crate::render::RenderDetail::default());
        windowed.perf.push_render_frame(1);
        let r = check(&mut windowed, "drawn");
        assert_eq!(r["history"].as_array().map(Vec::len), Some(1));
        #[cfg(feature = "3d")]
        {
            // A world with nothing to cull: counted, 0.
            assert_eq!(r["counts"]["frustum_culled_models"], 0);
            windowed
                .scene3d
                .set_quality_autotune(Some(crate::render::QualityAutotune::default()));
            let r = check(&mut windowed, "autotune on");
            assert!(r["quality_autotune"].is_object());
            assert!(r["unavailable"].get("quality_autotune").is_none());
        }
        #[cfg(not(feature = "3d"))]
        {
            let why = &r["unavailable"]["counts.frustum_culled_models"];
            assert!(why.as_str().unwrap().contains("`3d` feature"), "{r}");
        }
    }

    #[test]
    fn a_game_without_a_renderer_explains_its_missing_memory_and_adapter() {
        let mut ctx = ctx();
        ctx.set_headless(true);
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        let inspector = &perf["render_inspector"];
        for path in ["memory_estimate_bytes", "adapter"] {
            assert!(inspector[path].is_null(), "{path}: {inspector}");
            assert!(inspector["unavailable"][path].is_string(), "{path}: {inspector}");
        }
    }

    #[cfg(feature = "3d")]
    #[test]
    fn quality_set_switches_presets_and_answers_before_and_after() {
        let mut ctx = ctx();
        let low = handle(&mut ctx, "quality.set", &json!({ "level": "low" })).unwrap();
        assert_eq!(low["requested"], "low");
        assert_eq!(low["quality"]["level"], "low");
        assert_eq!(low["fitted_to_gpu"], false, "no GPU to fit to: {low}");
        let ultra = handle(&mut ctx, "quality.set", &json!({ "level": "ultra" })).unwrap();
        assert_eq!(
            ultra["previous"], low["quality"],
            "the answer carries the settings it replaced"
        );
        assert_eq!(ultra["quality"]["level"], "ultra");
        // `perf.get` reports the same settings.
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert_eq!(perf["render_inspector"]["quality"], ultra["quality"]);

        let err = handle(&mut ctx, "quality.set", &json!({ "level": "extreme" })).unwrap_err();
        assert!(err.contains("extreme") && err.contains("recommended"), "{err}");
        assert!(
            handle(&mut ctx, "quality.set", &json!({})).is_err(),
            "`level` is required"
        );
        // Without a GPU there is nothing to recommend for; nothing changes.
        // A windowed game before its first frame will have one.
        let err = handle(&mut ctx, "quality.set", &json!({ "level": "recommended" })).unwrap_err();
        assert!(err.contains("hasn't opened"), "{err}");
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert_eq!(perf["render_inspector"]["quality"]["level"], "ultra");
        // A headless game never will.
        let mut headless = Ctx::new(1.0 / 60.0, glam::Vec2::ZERO);
        headless.set_headless(true);
        let err = handle(&mut headless, "quality.set", &json!({ "level": "recommended" })).unwrap_err();
        assert!(err.contains("headless"), "{err}");
    }

    #[cfg(not(feature = "3d"))]
    #[test]
    fn quality_set_needs_the_3d_feature() {
        let mut ctx = ctx();
        let err = handle(&mut ctx, "quality.set", &json!({ "level": "low" })).unwrap_err();
        assert!(err.contains("3d"), "{err}");
    }

    #[test]
    fn headless_games_cant_trace_frames() {
        let mut ctx = ctx();
        ctx.set_headless(true);
        let err = handle(&mut ctx, "perf.trace", &json!({})).unwrap_err();
        assert!(err.contains("headless"), "{err}");
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert_eq!(perf["headless"], true);
        let inspector = &perf["render_inspector"];
        assert!(inspector["cpu_ms"].is_null(), "{inspector}");
        assert!(
            inspector["unavailable"]["cpu_ms"]
                .as_str()
                .unwrap()
                .contains("headless"),
            "{inspector}"
        );
        assert_eq!(inspector["quality"].is_object(), cfg!(feature = "3d"), "{inspector}");
        ctx.gfx
            .set_residency_budget(crate::streaming::ResidencyKind::Mesh, Some(1024));
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        assert_eq!(perf["residency"]["mesh"]["budget"], 1024, "{perf}");
        assert_eq!(perf["residency"]["mesh"]["evictions"], 0, "{perf}");
        assert_eq!(perf["residency"]["texture"]["evictions"], 0, "{perf}");
        assert_eq!(perf["streaming"]["uploads"]["pending"], 0, "{perf}");
        ctx.gfx.stream_texture("missing.png");
        let perf = handle(&mut ctx, "perf.get", &json!({})).unwrap();
        let asset = &perf["streaming"]["assets"][0];
        assert_eq!(
            (asset["kind"].as_str(), asset["state"].as_str()),
            (Some("texture"), Some("loading")),
            "{perf}"
        );
        assert_eq!(perf["streaming"]["uploads"]["pending"], 1, "{perf}");
    }

    #[test]
    fn headless_games_report_no_frames_and_only_sim_timings() {
        let mut ctx = ctx();
        ctx.set_headless(true);
        ctx.perf.timings = vec![
            crate::perf::SystemTiming {
                name: "sim",
                sim: true,
                last_ms: 1.0,
                avg_ms: 1.0,
            },
            crate::perf::SystemTiming {
                name: "draw",
                sim: false,
                last_ms: 0.0,
                avg_ms: 0.0,
            },
        ];
        let info = handle(&mut ctx, "info", &json!({})).unwrap();
        assert_eq!((info["fps"].clone(), info["frame"].clone()), (Value::Null, Value::Null));
        let timings = handle(&mut ctx, "systems.timings", &json!({})).unwrap();
        assert_eq!(timings.as_array().unwrap().len(), 1);
        assert_eq!(timings[0]["name"], "sim");
        ctx.set_headless(false);
        assert!(handle(&mut ctx, "info", &json!({})).unwrap()["fps"].is_number());
        assert_eq!(
            handle(&mut ctx, "systems.timings", &json!({}))
                .unwrap()
                .as_array()
                .unwrap()
                .len(),
            2
        );
    }

    #[test]
    fn ecs_diagnostics_list_archetypes_and_count_matches() {
        let mut ctx = ctx();
        ctx.world.spawn((Transform2D::at(vec2(1.0, 2.0)),));
        #[derive(ponko_ecs::Component)]
        struct Tag;
        ctx.world.spawn((Transform2D::default(), Tag));
        let d = handle(&mut ctx, "ecs.diagnostics", &json!({ "with": ["Transform2D"] })).unwrap();
        assert_eq!(d["version"], crate::diagnostics::ECS_DIAGNOSTICS_VERSION);
        assert_eq!(d["entities"], 2);
        assert_eq!(
            d["matching"],
            json!({ "with": ["Transform2D"], "archetypes": 2, "entities": 2 })
        );
        assert!(d["archetypes"].as_array().unwrap().iter().all(|a| a["entities"] != 0));
        assert!(d["archetypes_total"].as_u64().unwrap() >= 2);
        assert_eq!(d["schedule"]["dependencies_total"], 0);
        assert_eq!(d["schedule"]["parallel"], false);
        assert_eq!(d["query_cache"]["enabled"], true);
        let one = handle(&mut ctx, "ecs.diagnostics", &json!({ "limit": 1 })).unwrap();
        assert_eq!(one["archetypes"].as_array().unwrap().len(), 1);
        assert!(one.get("matching").is_none());
        let err = handle(&mut ctx, "ecs.diagnostics", &json!({ "with": ["Nope"] })).unwrap_err();
        assert!(err.contains("unknown component `Nope`"), "{err}");
        assert!(handle(&mut ctx, "ecs.diagnostics", &json!({ "with": 3 })).is_err());
    }

    #[test]
    fn tokens_are_128_random_bits() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.bytes().all(|c| c.is_ascii_hexdigit()), "{a}");
        assert_ne!(a, b);
    }

    #[test]
    fn ponko_control_is_ignored_where_the_build_does_not_honor_it() {
        assert!(env_control_with(true, true));
        assert!(!env_control_with(true, false), "a shipped release build opened a port");
        assert!(!env_control_with(false, true));
        assert!(!env_control_with(false, false));
        assert_eq!(
            ENV_CONTROL_HONORED,
            cfg!(any(debug_assertions, feature = "control-env"))
        );
    }

    #[test]
    fn tool_paths_stay_below_their_folder() {
        for ok in ["save.json", "saves/slot 1.json", "a/b/c.json"] {
            assert_eq!(check_tool_path(ok, "json"), Ok(()), "{ok}");
        }
        for bad in [
            "",
            "/etc/passwd.json",
            "../up.json",
            "saves/../../up.json",
            "./save.json",
            "saves\\..\\up.json",
            "C:\\up.json",
            "save.txt",
            "save",
            ".hidden.json",
            "saves/.git/x.json",
            "c:up.json",
            "nul.json",
            "saves/CON.json",
            "com1.json",
            "LPT9.backup.json",
            "aux .json",
            "CONIN$.json",
            "conout$.json",
            "COM\u{b9}.json",
            "lpt\u{b3}.json",
            "com0.json",
            "LPT0.json",
        ] {
            assert!(check_tool_path(bad, "json").is_err(), "{bad:?} was accepted");
        }
        for ok in [
            "console.json",
            "com10.json",
            "lpt.json",
            "saves/nullable.json",
            "com\u{b9}\u{b9}.json",
            "conin.json",
        ] {
            assert_eq!(check_tool_path(ok, "json"), Ok(()), "{ok}");
        }
        assert!(check_tool_path("shot.png", "json").is_err());
        assert_eq!(check_tool_path("shot.png", "png"), Ok(()));
    }

    #[test]
    fn prefab_spawn_returns_the_parts_and_prefab_despawn_removes_them() {
        let path = format!("ponko-test-control-tree-{}.json", std::process::id());
        let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join(&path);
        let mut ctx = ctx();
        let root = ctx.world.spawn((Transform2D::default(),));
        let arm = ctx
            .world
            .spawn((Transform2D::default(), crate::attach::Attached::to(root)));
        let saved = handle(
            &mut ctx,
            "prefab.save",
            &json!({ "entity": root.to_bits(), "path": path }),
        );
        ctx.world.despawn(root);
        ctx.world.despawn(arm);
        let spawned = saved.and_then(|_| handle(&mut ctx, "prefab.spawn", &json!({ "path": path, "at": [1.0, 2.0] })));
        let _ = std::fs::remove_file(&file);
        let spawned = spawned.unwrap();
        let parts = spawned["parts"].as_array().unwrap();
        assert_eq!(parts.len(), 1);
        let part = Entity::from_bits(parts[0].as_u64().unwrap());
        let new_root = Entity::from_bits(spawned["entity"].as_u64().unwrap());
        assert_eq!(ctx.world.get::<crate::attach::Attached>(part).unwrap().to, new_root);
        let gone = handle(&mut ctx, "prefab.despawn", &json!({ "entity": new_root.to_bits() })).unwrap();
        assert_eq!(gone, 2);
        assert_eq!(ctx.world.len(), 0);
    }

    #[test]
    fn file_methods_refuse_paths_outside_the_game() {
        let mut ctx = ctx();
        let outside = std::env::temp_dir().join(format!("ponko-escape-{}.json", std::process::id()));
        let absolute = outside.to_str().unwrap();
        for method in ["world.save", "world.load", "scene.save", "scene.load", "prefab.spawn"] {
            for path in [absolute, "../escape.json", "saves/../../escape.json"] {
                let params = json!({ "path": path, "at": [0.0, 0.0] });
                let err = handle(&mut ctx, method, &params).unwrap_err();
                assert!(err.contains("relative"), "{method} {path}: {err}");
            }
        }
        let e = ctx.world.spawn((Transform2D::default(),));
        let err = handle(
            &mut ctx,
            "prefab.save",
            &json!({ "entity": e.to_bits(), "path": "../p.json" }),
        )
        .unwrap_err();
        assert!(err.contains("relative"), "{err}");
        for method in ["screenshot", "perf.trace"] {
            assert!(
                handle(&mut ctx, method, &json!({ "path": absolute })).is_err(),
                "{method}"
            );
        }
        assert!(!outside.exists());
    }

    #[test]
    fn tokens_are_compared_whole() {
        let token = "0123456789abcdef0123456789abcdef";
        assert!(token_matches(Some(token), token));
        for wrong in [
            "",
            "0",
            "0123456789abcdef0123456789abcde",
            "0123456789abcdef0123456789abcdeg",
            "0123456789abcdef0123456789abcdef0",
        ] {
            assert!(!token_matches(Some(wrong), token), "{wrong:?}");
        }
        assert!(!token_matches(None, token));
    }

    /// A folder of the test's own, removed when dropped (for the link
    /// tests, which are unix only).
    #[cfg(unix)]
    struct TempDir(PathBuf);
    #[cfg(unix)]
    impl TempDir {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("ponko-{name}-{}", random_token().unwrap()));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir.canonicalize().unwrap())
        }
    }
    #[cfg(unix)]
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[cfg(unix)]
    #[test]
    fn links_inside_the_folder_must_not_lead_out_of_it() {
        let root = TempDir::new("root");
        let outside = TempDir::new("outside");
        let root = &root.0;
        std::fs::create_dir(root.join("saves")).unwrap();
        std::os::unix::fs::symlink(&outside.0, root.join("escape")).unwrap();
        std::os::unix::fs::symlink(outside.0.join("file.json"), root.join("dangling.json")).unwrap();
        std::os::unix::fs::symlink(root.join("saves"), root.join("inside")).unwrap();
        assert_eq!(check_below(root, &root.join("saves/new/a.json")), Ok(()));
        assert_eq!(
            check_below(root, &root.join("inside/a.json")),
            Ok(()),
            "a link that stays inside is fine"
        );
        let err = check_below(root, &root.join("escape/a.json")).unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
        assert!(check_below(root, &root.join("escape/deeper/a.json")).is_err());
        assert!(
            check_below(root, &root.join("dangling.json")).is_err(),
            "a dangling link out"
        );
        // Asset paths: a link inside an assets/ folder that leads out is
        // refused; the folder itself may be a link (shared assets).
        let assets = TempDir::new("assets");
        std::os::unix::fs::symlink(&outside.0, assets.0.join("shared")).unwrap();
        std::fs::create_dir(assets.0.join("levels")).unwrap();
        let roots = [assets.0.clone()];
        assert_eq!(check_asset_path("levels/a.json", &roots), Ok(()));
        let err = check_asset_path("shared/a.json", &roots).unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
        let linked_root = TempDir::new("linked-root");
        std::os::unix::fs::symlink(&assets.0, linked_root.0.join("assets")).unwrap();
        assert_eq!(
            check_asset_path("levels/a.json", &[linked_root.0.join("assets")]),
            Ok(()),
            "a linked assets/ folder is trusted"
        );
        // A folder that turns into a link while the folders are made (a
        // race) is caught by the check after making them.
        let outside_path = outside.0.clone();
        let err = make_folders(root, &root.join("made/a.json"), |dir| {
            std::os::unix::fs::symlink(&outside_path, dir)
        })
        .unwrap_err();
        assert!(err.contains("leads outside"), "{err}");
    }

    #[test]
    fn world_save_makes_new_files_unless_told_to_overwrite() {
        let mut ctx = ctx();
        ctx.world.spawn((Transform2D::default(),));
        // Relative to the working folder (the crate's, under cargo test).
        let dir = format!("ponko-test-saves-{}", random_token().unwrap());
        let path = format!("{dir}/slot.json");
        let save = |ctx: &mut Ctx, params: Value| handle(ctx, "world.save", &params);
        let first = save(&mut ctx, json!({ "path": path }));
        let again = save(&mut ctx, json!({ "path": path }));
        let overwritten = save(&mut ctx, json!({ "path": path, "overwrite": true }));
        let bad = save(&mut ctx, json!({ "path": path, "overwrite": "yes" }));
        let loaded = handle(&mut ctx, "world.load", &json!({ "path": path }));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(first.unwrap()["entities"], 1);
        assert!(again.unwrap_err().contains("already exists"));
        assert_eq!(overwritten.unwrap()["entities"], 1);
        assert!(bad.is_err());
        assert_eq!(loaded.unwrap()["entities"], 1);
    }

    #[test]
    fn claimed_files_are_removed_unless_kept() {
        let dir = format!("ponko-test-claims-{}", random_token().unwrap());
        let dropped = new_game_file(&format!("{dir}/dropped.png"), "png").unwrap();
        let dropped_path = dropped.path.clone();
        // A queued screenshot or unfinished trace when the game shuts down.
        let made = dropped_path.exists();
        {
            let _quit = dropped;
        }
        let dropped_left = dropped_path.exists();
        let mut kept = new_game_file(&format!("{dir}/kept.json"), "json").unwrap();
        std::io::Write::write_all(kept.file(), b"{}").unwrap();
        let kept_path = kept.path.clone();
        kept.keep();
        let kept_contents = std::fs::read(&kept_path);
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(made);
        assert!(!dropped_left, "an unwritten claimed file was left behind");
        assert_eq!(kept_contents.unwrap(), b"{}");
    }

    /// `world.save` with `overwrite` into a file a trace claimed would be
    /// removed with the claim when the game shut down before the trace was
    /// written: it is refused, and the claim removes only its own file.
    #[test]
    fn overwrite_leaves_claimed_files_alone() {
        let dir = format!("ponko-test-claimed-{}", random_token().unwrap());
        let claimed = new_game_file(&format!("{dir}/x.json"), "json").unwrap();
        let saved = write_game_file(&format!("{dir}/x.json"), "json", b"{\"world\":1}", true);
        let claimed_path = claimed.path.clone();
        drop(claimed);
        let gone = !claimed_path.exists();
        // Once the claim is gone, the path is free again.
        let later = write_game_file(&format!("{dir}/x.json"), "json", b"{}", true);
        let later_kept = std::fs::read(&claimed_path).ok();
        // A file that replaced the claimed one by name is left alone.
        #[cfg(unix)]
        let replaced_kept = {
            let claimed = new_game_file(&format!("{dir}/y.json"), "json").unwrap();
            std::fs::write(format!("{dir}/other.json"), "{}").unwrap();
            std::fs::rename(format!("{dir}/other.json"), &claimed.path).unwrap();
            let path = claimed.path.clone();
            drop(claimed);
            std::fs::read(path).ok()
        };
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(
            saved
                .unwrap_err()
                .contains("a screenshot or trace will be written there")
        );
        assert!(gone, "the unwritten claim is removed");
        assert!(later.is_ok(), "{later:?}");
        assert_eq!(later_kept.as_deref(), Some(&b"{}"[..]));
        #[cfg(unix)]
        assert_eq!(
            replaced_kept.as_deref(),
            Some(&b"{}"[..]),
            "removed a file it didn't claim"
        );
    }

    /// `scene.save` and `prefab.save` leave a claimed file alone too.
    #[test]
    fn asset_saves_leave_claimed_files_alone() {
        let dir = format!("ponko-test-claimed-asset-{}", random_token().unwrap());
        let claimed = new_game_file(&format!("assets/{dir}/scene.json"), "json").unwrap();
        let saved = crate::assets::write(&format!("{dir}/scene.json"), b"{}");
        let unchanged = std::fs::metadata(&claimed.path).map(|m| m.len());
        drop(claimed);
        let later = crate::assets::write(&format!("{dir}/scene.json"), b"{}");
        std::fs::remove_dir_all(format!("assets/{dir}")).unwrap();
        assert!(
            saved
                .unwrap_err()
                .contains("a screenshot or trace will be written there")
        );
        assert_eq!(unchanged.unwrap(), 0);
        assert!(later.is_ok(), "{later:?}");
    }

    /// `overwrite` replaces only a plain file with no other names, and
    /// replaces all of it.
    #[cfg(unix)]
    #[test]
    fn overwrite_refuses_pipes_and_hard_links() {
        let dir = format!("ponko-test-special-{}", random_token().unwrap());
        std::fs::create_dir(&dir).unwrap();
        let fifo = std::ffi::CString::new(format!("{dir}/pipe.json")).unwrap();
        // SAFETY: a valid NUL-terminated path.
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let started = std::time::Instant::now();
        let pipe = write_game_file(&format!("{dir}/pipe.json"), "json", b"{}", true);
        let waited = started.elapsed();
        std::fs::write(format!("{dir}/a.json"), "{\"long\": \"old contents\"}").unwrap();
        std::fs::hard_link(format!("{dir}/a.json"), format!("{dir}/b.json")).unwrap();
        let linked = write_game_file(&format!("{dir}/b.json"), "json", b"{}", true);
        let linked_target = std::fs::read_to_string(format!("{dir}/a.json")).unwrap();
        std::fs::write(format!("{dir}/c.json"), "{\"long\": \"old contents\"}").unwrap();
        let plain = write_game_file(&format!("{dir}/c.json"), "json", b"{}", true);
        let plain_now = std::fs::read_to_string(format!("{dir}/c.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(pipe.is_err(), "{pipe:?}");
        assert!(waited < Duration::from_secs(5), "waited {waited:?} on a pipe");
        assert!(linked.unwrap_err().contains("hard links"));
        assert_eq!(linked_target, "{\"long\": \"old contents\"}");
        assert!(plain.is_ok(), "{plain:?}");
        assert_eq!(plain_now, "{}", "replaced whole");
    }

    #[test]
    fn traces_and_screenshots_claim_a_new_file_first() {
        let mut ctx = ctx();
        ctx.headless = false;
        // Relative to the working folder (the crate's, under cargo test).
        let dir = format!("ponko-test-files-{}", random_token().unwrap());
        let name = format!("{dir}/traces/t.json");
        let first = handle(&mut ctx, "perf.trace", &json!({ "path": name, "frames": 1 }));
        let claimed = std::fs::metadata(&name).map(|m| m.len());
        let again = handle(&mut ctx, "perf.trace", &json!({ "path": name, "frames": 1 }));
        // A second trace replaces the first, whose file nobody will write
        // now: it is removed, so the path can be asked for again.
        let other = format!("{dir}/traces/other.json");
        let replaced = handle(&mut ctx, "perf.trace", &json!({ "path": other, "frames": 1 }));
        let first_left = Path::new(&name).exists();
        let retried = handle(&mut ctx, "perf.trace", &json!({ "path": name, "frames": 1 }));
        // A screenshot into a new folder.
        let shot = screenshot_path(&json!({ "path": format!("{dir}/shots/a.png") }));
        let shot_made = shot.as_ref().is_ok_and(|c| c.path.exists());
        if let Ok(shot) = shot {
            shot.abandon();
        }
        let shot_removed = !Path::new(&format!("{dir}/shots/a.png")).exists();
        // Where nothing would write the trace: refused, and nothing left.
        ctx.perf.headless = true;
        let headless_name = format!("{dir}/headless/h.json");
        let headless = handle(&mut ctx, "perf.trace", &json!({ "path": headless_name }));
        let headless_left = Path::new(&format!("{dir}/headless")).exists();
        ctx.perf.headless = false;
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(first.is_ok(), "into a new folder: {first:?}");
        assert_eq!(claimed.unwrap(), 0, "made empty when asked, written later");
        assert!(again.unwrap_err().contains("already exists"));
        assert!(replaced.is_ok(), "{replaced:?}");
        assert!(!first_left, "the replaced trace's file was left behind");
        assert!(retried.is_ok(), "{retried:?}");
        assert!(shot_made, "a screenshot can go into a new folder");
        assert!(shot_removed, "an abandoned file is removed");
        assert!(headless.unwrap_err().contains("headless"));
        assert!(!headless_left, "refused before its folder was made");
        // Temp files get a fresh random name each time.
        let temp =
            |ctx: &mut Ctx| PathBuf::from(handle(ctx, "perf.trace", &json!({})).unwrap()["path"].as_str().unwrap());
        let a = temp(&mut ctx);
        let a_exists = a.exists();
        let b = temp(&mut ctx);
        assert_ne!(a, b);
        assert!(a_exists && !a.exists(), "replaced, so removed");
        assert!(b.exists());
        let _ = std::fs::remove_file(b);
        assert!(screenshot_path(&json!({ "path": "../x.png" })).is_err());
    }

    // Unix only: making a link on Windows needs a privilege tests can't
    // count on. There `overwrite` relies on the link check alone (no
    // `O_NOFOLLOW`), which the same `symlink_metadata` test covers.
    #[cfg(unix)]
    #[test]
    fn overwrite_never_writes_through_a_link() {
        let dir = format!("ponko-test-links-{}", random_token().unwrap());
        std::fs::create_dir(&dir).unwrap();
        std::os::unix::fs::symlink(
            std::fs::canonicalize(&dir).unwrap().join("real.json"),
            format!("{dir}/link.json"),
        )
        .unwrap();
        std::fs::write(format!("{dir}/real.json"), "{}").unwrap();
        let result = write_game_file(&format!("{dir}/link.json"), "json", b"[]", true);
        let real = std::fs::read_to_string(format!("{dir}/real.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(result.unwrap_err().contains("is a link"));
        assert_eq!(real, "{}", "the link's target is untouched");
    }

    /// The open itself refuses a link too (`O_NOFOLLOW`), for one made
    /// after the check.
    #[cfg(unix)]
    #[test]
    fn the_overwriting_open_refuses_a_link_by_itself() {
        let dir = format!("ponko-test-nofollow-{}", random_token().unwrap());
        std::fs::create_dir(&dir).unwrap();
        std::fs::write(format!("{dir}/real.json"), "{}").unwrap();
        std::os::unix::fs::symlink("real.json", format!("{dir}/link.json")).unwrap();
        let opened = open_to_overwrite(Path::new(&format!("{dir}/link.json")));
        let real = std::fs::read_to_string(format!("{dir}/real.json")).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(opened.is_err());
        assert_eq!(real, "{}");
    }

    #[test]
    fn an_idle_connection_without_the_token_is_dropped_and_frees_its_slot() {
        let control = Control::start_with_auth_timeout("idle", true, false, Duration::from_millis(300)).unwrap();
        let connect = || {
            let c = TcpStream::connect(control.addr()).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            c
        };
        // Every slot taken by a client that never says anything, and one
        // that trickles bytes without a newline.
        let idle: Vec<TcpStream> = (0..MAX_CONNECTIONS - 1).map(|_| connect()).collect();
        let trickle = connect();
        trickle.set_nonblocking(true).unwrap();
        // The trickler stops writing once the reply is there, like a real
        // client: writing to a connection the server has closed resets it,
        // and on Windows the reset throws away the reply not read yet.
        let trickler = thread::spawn(move || {
            let (mut reply, mut buf) = (Vec::new(), [0u8; 256]);
            let mut stream = &trickle;
            let give_up = std::time::Instant::now() + Duration::from_secs(10);
            while !reply.contains(&b'\n') && std::time::Instant::now() < give_up {
                if reply.is_empty() {
                    match stream.write(b"{") {
                        Err(e) if e.kind() != std::io::ErrorKind::WouldBlock => panic!("the trickle was cut off: {e}"),
                        _ => {}
                    }
                }
                thread::sleep(Duration::from_millis(50));
                match stream.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => reply.extend_from_slice(&buf[..n]),
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(e) => panic!("the trickling client lost its reply: {e}"),
                }
            }
            (trickle, String::from_utf8(reply).unwrap())
        });
        // They are told why and closed, so a real client gets in.
        let mut reply = String::new();
        BufReader::new(&idle[0]).read_line(&mut reply).unwrap();
        assert!(reply.contains("no request with the right token"), "{reply}");
        let (_trickle, reply) = trickler.join().unwrap();
        assert!(
            reply.contains("no request with the right token"),
            "a trickle doesn't keep it open: {reply}"
        );
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let c = connect();
            if ask(&c)["error"].as_str().unwrap().contains("token") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the idle connections' slots were never freed"
            );
            thread::sleep(Duration::from_millis(10));
        }
        drop(idle);
    }

    #[test]
    fn a_connection_that_sent_the_token_may_stay_idle() {
        let control = Control::start_with_auth_timeout("trusted", true, false, Duration::from_millis(200)).unwrap();
        let client = TcpStream::connect(control.addr()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut writer = client.try_clone().unwrap();
        let mut reader = BufReader::new(&client);
        let mut request = |id: u64| {
            writeln!(
                writer,
                "{}",
                json!({ "id": id, "token": &*control.token, "method": "logs.get" })
            )
            .unwrap();
            let mut reply = String::new();
            reader.read_line(&mut reply).unwrap();
            serde_json::from_str::<Value>(&reply).unwrap()
        };
        assert_eq!(request(1)["id"], 1);
        thread::sleep(Duration::from_millis(600));
        assert_eq!(request(2)["id"], 2, "still served after idling past the token deadline");
    }

    /// Sends `logs.get` (answered without the game loop) with `token`.
    /// `Null` if the connection was refused or closed instead.
    fn ask_with(client: &TcpStream, token: &str) -> Value {
        let mut writer = client.try_clone().unwrap();
        let mut reply = String::new();
        let asked = writeln!(writer, "{}", json!({ "id": 1, "token": token, "method": "logs.get" }))
            .and_then(|()| BufReader::new(client).read_line(&mut reply));
        asked
            .ok()
            .and_then(|_| serde_json::from_str(&reply).ok())
            .unwrap_or(Value::Null)
    }

    #[test]
    fn a_stranger_gets_one_reply_and_is_disconnected() {
        let control = Control::start_with("stranger", true, false).unwrap();
        let client = TcpStream::connect(control.addr()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        assert!(ask(&client)["error"].as_str().unwrap().contains("token"));
        let mut rest = Vec::new();
        assert!(
            BufReader::new(&client)
                .read_to_end(&mut rest)
                .map_or(true, |_| rest.is_empty())
        );
    }

    #[test]
    fn clients_that_never_read_their_replies_dont_hold_slots() {
        let control = Control::start_with("no-readers", true, false).unwrap();
        // Every slot taken by a client that sends a megabyte of bad lines
        // and never reads a reply.
        let flooders: Vec<_> = (0..MAX_CONNECTIONS)
            .map(|_| {
                let mut c = TcpStream::connect(control.addr()).unwrap();
                thread::spawn(move || {
                    let lines = b"not json\n".repeat(1024 * 1024 / 9);
                    let _ = c.write_all(&lines);
                    c // kept open, never read
                })
            })
            .collect();
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let c = TcpStream::connect(control.addr()).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            if ask_with(&c, &control.token)["id"] == 1 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "the flooders kept every slot");
            thread::sleep(Duration::from_millis(10));
        }
        let held: Vec<TcpStream> = flooders.into_iter().map(|f| f.join().unwrap()).collect();
        drop(held);
    }

    /// Sends a request with a wrong token and reads the reply: a connection
    /// that is being served answers it at once, without the game loop.
    fn ask(client: &TcpStream) -> Value {
        let mut writer = client.try_clone().unwrap();
        writeln!(
            writer,
            "{}",
            json!({ "id": 1, "token": "wrong", "method": "sim.status" })
        )
        .unwrap();
        let mut reply = String::new();
        BufReader::new(client).read_line(&mut reply).unwrap();
        serde_json::from_str(&reply).unwrap()
    }

    #[test]
    fn an_over_long_line_is_refused_before_the_token_is_read() {
        let control = Control::start_with("long-line", true, false).unwrap();
        let client = TcpStream::connect(control.addr()).unwrap();
        client.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
        let mut writer = client.try_clone().unwrap();
        // One byte past the limit, without a newline. Exactly that much, so
        // the server has read everything when it hangs up (unread bytes
        // would turn its close into a reset that loses the reply).
        let sender = thread::spawn(move || writer.write_all(&vec![b'x'; MAX_LINE + 1]).unwrap());
        let mut reply = String::new();
        let mut reader = BufReader::new(&client);
        reader.read_line(&mut reply).unwrap();
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert!(reply["error"].as_str().unwrap().contains("longer than"), "{reply}");
        sender.join().unwrap();
        // Then the connection is closed.
        let mut rest = Vec::new();
        assert!(reader.read_to_end(&mut rest).map_or(true, |_| rest.is_empty()));
    }

    #[test]
    fn connections_past_the_limit_are_refused_until_one_closes() {
        let control = Control::start_with("connections", true, false).unwrap();
        let connect = || {
            let c = TcpStream::connect(control.addr()).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
            c
        };
        let mut open: Vec<TcpStream> = (0..MAX_CONNECTIONS).map(|_| connect()).collect();
        for (i, c) in open.iter().enumerate() {
            assert_eq!(ask_with(c, &control.token)["id"], 1, "connection {i}");
        }
        let extra = connect();
        let mut reply = String::new();
        BufReader::new(&extra).read_line(&mut reply).unwrap();
        assert!(reply.contains("too many connections"), "{reply}");
        // Closing one frees its slot once its thread notices.
        drop(open.pop());
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            let c = connect();
            if ask(&c)["error"].as_str().unwrap().contains("token") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the closed connection's slot was never freed"
            );
            drop(c);
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn short_names() {
        assert_eq!(short("a::b::Health"), "Health");
        assert_eq!(short("Health"), "Health");
    }
}
