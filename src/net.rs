//! Networking: three ways to keep players in step, behind one entry,
//! [`ctx.net`](crate::ctx::Ctx::net) ([`Networking`]).
//!
//! | Pick | For | Start with | Example |
//! |---|---|---|---|
//! | **Lockstep** ([`Net`]) | Deterministic simulations with 2–8 players (strategy, fighting, co-op puzzles): every peer runs the same ticks from the same inputs, so only inputs cross the network | [`Game::lockstep`](crate::game::Game::lockstep), then `ctx.net.set_input` / `ctx.net.input` | [`examples/lockstep.rs`](https://github.com/ponkoengine/ponko/blob/main/examples/lockstep.rs) |
//! | **Messages** ([`NetMessages`]) | Everything else: your own packets on reliable and unreliable lanes between a host and its clients (chat, lobbies, events, your own replication) | [`Game::messages`](crate::game::Game::messages), then `ctx.net.messages` | [`examples/net_messages.rs`](https://github.com/ponkoengine/ponko/blob/main/examples/net_messages.rs) |
//! | **Snapshots** ([`NetSnapshots`], [`SnapshotStream`], [`Interest`], [`Authority`]) | Server-authoritative worlds on top of messages: the owner sends timestamped states, everyone else shows them a little in the past | `ctx.net.snapshots` and `ctx.net.interest` | [`examples/net_replication.rs`](https://github.com/ponkoengine/ponko/blob/main/examples/net_replication.rs) |
//!
//! Lockstep and messages are independent and can run side by side: lockstep
//! for the simulation, messages for a chat that must not hold ticks back.
//! The engine drives what is there (see the table on [`Networking`]); a
//! single-player game has an offline lockstep session and nothing else.
//! Feature `steam` adds Steam relays and lobbies for both transports.
//!
//! # Lockstep
//!
//! Every peer runs the same simulation from the same inputs, so only
//! inputs cross the network.
//!
//! The host (player 0) relays inputs in a star. Tick `t` runs once every
//! player's input for `t` is in; your own input is scheduled
//! [`input_delay`](Net::input_delay) ticks ahead, which hides the round trip.
//! Every packet repeats what the other side hasn't acknowledged, so lost
//! packets cost nothing but a little bandwidth. Every
//! [`checksum_every`](Net::checksum_every) ticks the peers compare a hash of
//! every reflected component and resource and report a [`Desync`] naming the
//! components that differ.
//!
//! Plain UDP on std sockets, polled from the game thread: no async runtime,
//! no extra thread.

use std::collections::{BTreeMap, VecDeque};
use std::io;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::mpsc;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::perf::Instant;
use crate::reflect::WorldReflect;
use ponko_ecs::World;

#[cfg(test)]
mod lockstep_conformance;
#[cfg(test)]
mod message_conformance;
mod messages;
#[cfg(test)]
mod scripted;
pub use messages::{Lane, MAX_MESSAGE_BYTES, MessageEvent, MessageStats, NetMessages, PeerId};

mod interest;
mod sequence;
mod service;
pub(crate) mod snapshots;
pub use interest::{
    Interest, InterestChange, InterestChangeKind, InterestConfig, InterestEntity, InterestEntry, InterestTier,
    InterestViewer, MAX_INTEREST_TIERS,
};
pub use sequence::{Authority, AuthorityCheck, LatestWins, UpdateStamp, seq_diff, seq_newer};
pub use service::{NetSnapshots, Networking};
#[cfg(feature = "3d")]
pub use snapshots::NetPose;
pub use snapshots::{
    ClockSync, Interpolate, Pose, SnapshotBuffer, SnapshotConfig, SnapshotPush, SnapshotSample, SnapshotState,
    SnapshotStream,
};

#[cfg(feature = "steam")]
mod steam_messages;
#[cfg(feature = "steam")]
#[path = "net/steam.rs"]
mod steam_transport;
#[cfg(feature = "steam")]
pub use steam_messages::{SteamLobby, SteamLobbyVisibility};
#[cfg(feature = "steam")]
pub use steam_transport::{steam_lobby_from_args, steam_wait_for_invite};

/// The most players in one session.
pub const MAX_PLAYERS: usize = 8;
/// The most bytes one player's input for one tick can have.
pub const MAX_INPUT_BYTES: usize = 1024;

const MAGIC: [u8; 4] = *b"CRBN";
const VERSION: u8 = 1;
const HELLO: u8 = 1;
const WELCOME: u8 = 2;
const START: u8 = 3;
const INPUTS: u8 = 4;
const CHECK: u8 = 5;
const REJECT: u8 = 6;
/// "I stopped and need nothing more": peers stop resending to us.
const BYE: u8 = 7;

/// Ticks of input kept per player; peers are never more than
/// `input_delay + 1` ticks apart, far less than this.
const RING: usize = 256;
const MAX_DELAY: u32 = 64;
const PACKET: usize = 1400;
/// An INPUTS packet with nothing in it yet is this long at most: magic,
/// version, kind, player count, one acknowledgement per player, block count.
const INPUTS_HEADER: usize = 4 + 1 + 1 + 1 + 8 * MAX_PLAYERS + 1;
/// A block's header (stream, first tick, tick count) and one tick's length.
const BLOCK_HEADER: usize = 1 + 8 + 1 + 2;
// The largest input always fits a packet of its own, so `flush` can keep
// every packet within PACKET.
const _: () = assert!(INPUTS_HEADER + BLOCK_HEADER + MAX_INPUT_BYTES <= PACKET);
/// Resend unacknowledged inputs (and keep the link alive) this often.
const RESEND: Duration = Duration::from_millis(20);
const HELLO_EVERY: Duration = Duration::from_millis(100);
/// How long a finished session keeps resending for peers that still miss
/// inputs, at most.
const LINGER: Duration = Duration::from_secs(1);
/// A peer silent this long while we linger has left too.
const GONE: Duration = Duration::from_millis(250);
/// Receive errors in a row the UDP transport skips before it reports "no
/// packet" for this poll.
const UDP_ERROR_SKIPS: usize = 64;

/// Where a session is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NetState {
    /// No session: one local player, inputs apply at once.
    Offline,
    /// Waiting for players to join (host) or for the host (join).
    Connecting,
    /// Everyone is in; ticks run as inputs arrive.
    Running,
    /// A peer went quiet for longer than [`Net::timeout`], or the host
    /// refused us (see [`Net::error`]). Ticks don't run any more.
    Lost,
}

/// The peers' worlds stopped matching: from `tick` on, they're different games.
///
/// ```
/// # use ponko::prelude::*;
/// let d = ponko::net::Desync { tick: 120, player: 1, components: vec!["Transform2D".into()] };
/// assert!(d.to_string().contains("Transform2D"));
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Desync {
    /// The first checked tick that differed.
    pub tick: u64,
    /// The player whose world differs from ours.
    pub player: usize,
    /// Reflected components (and `resource:Name`s) whose values differ.
    pub components: Vec<String>,
}

impl std::fmt::Display for Desync {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "desync with player {} at tick {}: {} differ",
            self.player,
            self.tick,
            self.components.join(", ")
        )
    }
}

/// What a session has sent so far ([`Net::stats`]).
///
/// ```
/// # use ponko::prelude::*;
/// assert_eq!(ponko::net::Net::default().stats().bytes_sent, 0);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NetStats {
    /// Packets sent, to all peers.
    pub packets_sent: u64,
    /// Bytes sent (UDP payload), to all peers.
    pub bytes_sent: u64,
}

/// A lockstep session: `ctx.net.lockstep`, which `ctx.net` dereferences
/// to. Start one with [`Net::host`], [`Net::join`] or [`Net::loopback`] and
/// pass it to [`Game::lockstep`](crate::game::Game::lockstep). Without one, it is
/// offline: one player whose input applies on the next tick, so the same
/// game code runs alone and online.
///
/// Systems send their player's input with [`set_input`](Self::set_input)
/// and read everyone's with [`input`](Self::input) in simulation systems:
///
/// ```
/// use ponko::prelude::*;
/// use serde::{Deserialize, Serialize};
///
/// #[derive(Serialize, Deserialize)]
/// struct Move(f32);
///
/// #[derive(Component)]
/// struct Pos(f32);
///
/// let world = Game::new("t", 1, 1)
///     .sim_system(|ctx: &mut Ctx| {
///         ctx.net.set_input(&Move(1.0)); // usually from the keyboard, in a frame system
///         for player in 0..ctx.net.players() {
///             if let Some(Move(dx)) = ctx.net.input::<Move>(player) {
///                 ctx.world.query::<&mut Pos>().for_each(|p| p.0 += dx);
///             }
///         }
///     })
///     .run_headless(3, |ctx| {
///         ctx.world.spawn((Pos(0.0),));
///     });
/// # let _ = world;
/// ```
pub struct Net {
    session: Option<Box<Session>>,
    /// This player's input for the next tick it can go into.
    pending: Vec<u8>,
    /// Everyone's input for the tick that ran last.
    current: Vec<Vec<u8>>,
    player: usize,
    players: usize,
    delay: u32,
    checksum_every: u32,
    timeout: Duration,
    state: NetState,
    error: Option<String>,
    desync: Option<Desync>,
    /// Waits in a row, to back off from yielding to sleeping.
    waits: u32,
    /// Keep resending on drop until peers have our inputs (see `Drop`). Off
    /// for scripted test sessions, which step [`linger_step`](Self::linger_step)
    /// on their own clock instead.
    linger: bool,
}

impl Default for Net {
    fn default() -> Self {
        Self {
            session: None,
            pending: Vec::new(),
            current: vec![Vec::new()],
            player: 0,
            players: 1,
            delay: 0,
            checksum_every: 0,
            timeout: Duration::from_secs(10),
            state: NetState::Offline,
            error: None,
            desync: None,
            waits: 0,
            linger: true,
        }
    }
}

impl std::fmt::Debug for Net {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Net")
            .field("state", &self.state)
            .field("player", &self.player)
            .field("players", &self.players)
            .finish_non_exhaustive()
    }
}

impl Net {
    /// Hosts a session for `players` players (2–8, you included as player
    /// 0) on `addr`, such as `"0.0.0.0:7777"`. Port 0 picks a free one; see
    /// [`local_addr`](Self::local_addr). The game starts once everyone joined.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let net = ponko::net::Net::host("127.0.0.1:0", 2).unwrap();
    /// assert!(net.local_addr().unwrap().port() != 0);
    /// ```
    ///
    /// # Errors
    ///
    /// `InvalidInput` unless `players` is 2 to 8; the socket's error if `addr`
    /// can't be bound (in use, not this machine's).
    pub fn host(addr: impl ToSocketAddrs, players: usize) -> io::Result<Net> {
        if !(2..=MAX_PLAYERS).contains(&players) {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "2 to 8 players"));
        }
        let transport = Udp::bind(addr)?;
        Ok(Self::with(Session::new(Box::new(transport), Role::Host), 0, players))
    }

    /// Joins the session hosted at `host`, such as `"192.168.1.20:7777"`.
    /// The host decides the player count, input delay and checksum rate; both
    /// sides must use the same [`Game::seed`](crate::game::Game::seed).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let net = ponko::net::Net::join("127.0.0.1:7777").unwrap();
    /// assert_eq!(net.state(), ponko::net::NetState::Connecting);
    /// ```
    ///
    /// # Errors
    ///
    /// If `host` doesn't resolve to an address, or no local socket can be bound.
    pub fn join(host: impl ToSocketAddrs) -> io::Result<Net> {
        let host = host
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let any: SocketAddr = if host.is_ipv4() {
            "0.0.0.0:0".parse().expect("address")
        } else {
            "[::]:0".parse().expect("address")
        };
        let transport = Udp::bind(any)?;
        Ok(Self::with(Session::new(Box::new(transport), Role::Client(host)), 0, 0))
    }

    /// `players` sessions connected in memory, the first one hosting: for
    /// tests, or bots in one process. Run each in its own game (and thread).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let nets = ponko::net::Net::loopback(2);
    /// assert_eq!(nets.len(), 2);
    /// ```
    #[must_use]
    pub fn loopback(players: usize) -> Vec<Net> {
        Self::loopback_lossy(players, 0)
    }

    /// Like [`loopback`](Self::loopback), dropping one packet in `drop_one_in`
    /// (0: none), to test that games survive loss.
    #[doc(hidden)]
    #[must_use]
    pub fn loopback_lossy(players: usize, drop_one_in: u32) -> Vec<Net> {
        assert!((2..=MAX_PLAYERS).contains(&players), "2 to 8 players");
        let (txs, rxs): (Vec<_>, Vec<_>) = (0..players).map(|_| mpsc::channel()).unzip();
        let txs = std::sync::Arc::new(txs);
        rxs.into_iter()
            .enumerate()
            .map(|(i, rx)| {
                let transport = Local {
                    me: Local::addr(i),
                    rx,
                    peers: txs.clone(),
                    drop_one_in,
                    sent: i as u32,
                };
                if i == 0 {
                    Self::with(Session::new(Box::new(transport), Role::Host), 0, players)
                } else {
                    Self::with(Session::new(Box::new(transport), Role::Client(Local::addr(0))), 0, 0)
                }
            })
            .collect()
    }

    /// A session on a caller-supplied transport, hosting `players` players
    /// or joining `host`: the seam the conformance tests in
    /// `net/lockstep_conformance.rs` script. Drive it with
    /// [`ready_at`](Self::ready_at) and a simulated clock.
    #[cfg(test)]
    fn transported(transport: Box<dyn Transport>, host: Option<SocketAddr>, players: usize) -> Net {
        let role = host.map_or(Role::Host, Role::Client);
        let mut net = Self::with(Session::new(transport, role), 0, players);
        net.linger = false;
        net
    }

    fn with(session: Session, player: usize, players: usize) -> Net {
        let mut net = Net::default();
        net.session = Some(Box::new(session));
        net.player = player;
        net.players = players;
        net.current = vec![Vec::new(); players.max(1)];
        net.delay = 3;
        net.checksum_every = 60;
        net.state = NetState::Connecting;
        net
    }

    /// Schedules each input this many ticks ahead (default 3, at most 64):
    /// input sent at tick `t` is used at tick `t + delay` everywhere. More
    /// hides more latency but feels laggier; at 60 Hz, 3 ticks covers a
    /// 50 ms round trip. Set on the host.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let net = ponko::net::Net::host("127.0.0.1:0", 2).unwrap().input_delay(5);
    /// # let _ = net;
    /// ```
    ///
    /// # Panics
    ///
    /// If `ticks` is over 64.
    #[must_use]
    pub fn input_delay(mut self, ticks: u32) -> Self {
        assert!(ticks <= MAX_DELAY, "input delay is at most {MAX_DELAY} ticks");
        self.delay = ticks;
        self
    }

    /// Compares worlds every this many ticks (default 60; 0 never). Each
    /// check serializes the reflected world, so big worlds want a larger
    /// number. Set on the host.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let net = ponko::net::Net::host("127.0.0.1:0", 2).unwrap().checksum_every(300);
    /// # let _ = net;
    /// ```
    #[must_use]
    pub fn checksum_every(mut self, ticks: u32) -> Self {
        self.checksum_every = ticks;
        self
    }

    /// Gives up on a peer that's silent this long (default 10 s): the state
    /// becomes [`NetState::Lost`] and headless runs stop.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let net = ponko::net::Net::join("127.0.0.1:7777").unwrap().timeout(std::time::Duration::from_secs(3));
    /// # let _ = net;
    /// ```
    #[must_use]
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// The address this side listens on (`None` offline).
    #[must_use]
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.session.as_ref().map(|s| s.transport.local_addr())
    }

    /// Which player this is: 0 is the host (and the offline player).
    #[must_use]
    pub fn player(&self) -> usize {
        self.player
    }
    /// How many players there are (0 while joining).
    #[must_use]
    pub fn players(&self) -> usize {
        self.players
    }
    /// Where the session is.
    #[must_use]
    pub fn state(&self) -> NetState {
        self.state
    }
    /// Why the session was lost, if it was.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    /// Packets and bytes sent so far.
    pub fn stats(&self) -> NetStats {
        self.session.as_ref().map_or_else(NetStats::default, |s| s.stats)
    }
    /// The first desync found, if any.
    #[must_use]
    pub fn desync(&self) -> Option<&Desync> {
        self.desync.as_ref()
    }

    /// Sets this player's input for the next tick it can go into (encoded
    /// with serde as JSON). The last call before that tick wins, and the
    /// input is cleared after it's sent: set it every frame for held keys.
    ///
    /// # Panics
    /// If the encoded input is over [`MAX_INPUT_BYTES`].
    pub fn set_input<T: Serialize>(&mut self, input: &T) {
        self.pending.clear();
        serde_json::to_writer(&mut self.pending, input).expect("inputs serialize");
        assert!(
            self.pending.len() <= MAX_INPUT_BYTES,
            "input is over {MAX_INPUT_BYTES} bytes"
        );
    }

    /// Like [`set_input`](Self::set_input), with bytes you encode yourself.
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// let mut net = ponko::net::Net::default();
    /// net.set_input_bytes(&[1, 2, 3]);
    /// ```
    ///
    /// # Panics
    ///
    /// If `input` is over [`MAX_INPUT_BYTES`].
    pub fn set_input_bytes(&mut self, input: &[u8]) {
        assert!(input.len() <= MAX_INPUT_BYTES, "input is over {MAX_INPUT_BYTES} bytes");
        self.pending.clear();
        self.pending.extend_from_slice(input);
    }

    /// `player`'s input for the current tick, or `None` if they sent none
    /// (or it isn't a `T`). Read it in simulation systems.
    #[must_use]
    pub fn input<T: DeserializeOwned>(&self, player: usize) -> Option<T> {
        let bytes = self.input_bytes(player);
        if bytes.is_empty() {
            return None;
        }
        serde_json::from_slice(bytes).ok()
    }

    /// `player`'s input for the current tick as bytes (empty if none).
    ///
    /// ```
    /// # use ponko::prelude::*;
    /// assert!(ponko::net::Net::default().input_bytes(0).is_empty());
    /// ```
    pub fn input_bytes(&self, player: usize) -> &[u8] {
        self.current.get(player).map_or(&[], Vec::as_slice)
    }

    /// Called once before the first tick: the game's seed must match the host's.
    pub(crate) fn begin(&mut self, seed: u64) {
        if let Some(s) = self.session.as_deref_mut() {
            s.seed = seed;
        }
    }

    /// Gets ready for `tick`: true once every player's input for it is in.
    pub(crate) fn ready(&mut self, tick: u64) -> bool {
        if self.session.is_none() {
            // Offline never reads the clock.
            std::mem::swap(&mut self.current[0], &mut self.pending);
            self.pending.clear();
            return true;
        }
        self.ready_at(tick, Instant::now())
    }

    /// [`ready`](Self::ready) at a time the caller chooses. Every protocol
    /// timer (resends, hellos, the silence timeout) is measured against
    /// `now`, so a scripted test owns the clock.
    fn ready_at(&mut self, tick: u64, now: Instant) -> bool {
        let Some(s) = self.session.as_deref_mut() else {
            std::mem::swap(&mut self.current[0], &mut self.pending);
            self.pending.clear();
            return true;
        };
        if self.state == NetState::Lost {
            return false;
        }
        s.poll(
            now,
            &mut Settings {
                player: &mut self.player,
                players: &mut self.players,
                delay: &mut self.delay,
                checksum_every: &mut self.checksum_every,
                desync: &mut self.desync,
            },
        );
        if let Some(e) = s.error.take() {
            return self.lose(e);
        }
        if !s.started {
            return false;
        }
        self.state = NetState::Running;
        if self.current.len() != self.players {
            self.current.resize(self.players, Vec::new());
        }
        // Our input for `tick + delay`, once.
        let due = tick + u64::from(self.delay);
        if s.streams[self.player].have <= due {
            s.streams[self.player].push(due, &self.pending);
            self.pending.clear();
        }
        s.flush(now, self.player, self.players);
        if let Some(quiet) = s.silent_peer(now, self.timeout) {
            return self.lose(format!("player {quiet} stopped answering"));
        }
        if s.streams.iter().any(|st| st.have <= tick) {
            return false;
        }
        self.waits = 0;
        for (p, st) in s.streams.iter().enumerate() {
            self.current[p].clear();
            self.current[p].extend_from_slice(st.get(tick));
        }
        true
    }

    fn lose(&mut self, error: String) -> bool {
        eprintln!("ponko lockstep: {error}");
        self.error = Some(error);
        self.state = NetState::Lost;
        false
    }

    /// After a tick ran: compares worlds when it's time.
    pub(crate) fn end_tick(&mut self, world: &World, tick: u64) {
        let Some(s) = self.session.as_deref_mut() else { return };
        if self.checksum_every == 0 || !(tick + 1).is_multiple_of(u64::from(self.checksum_every)) {
            return;
        }
        let check = Check::of(world, tick);
        s.send_check(&check);
        if self.desync.is_none() {
            self.desync = s.compare_stored(&check);
            if let Some(d) = &self.desync {
                eprintln!("ponko lockstep: {d}");
            }
        }
        s.own_checks.push_back(check);
        if s.own_checks.len() > 16 {
            s.own_checks.pop_front();
        }
    }

    /// Waits a little for packets (headless runs, between ticks). False
    /// once the session is lost.
    pub(crate) fn wait(&mut self) -> bool {
        // Peers are usually only microseconds behind (fast-forward, loopback):
        // yield first, sleep only if the wait drags on.
        self.waits += 1;
        if self.waits < 64 {
            std::thread::yield_now();
        } else {
            std::thread::sleep(Duration::from_micros(200));
        }
        self.waited(Instant::now())
    }

    /// The clock half of [`wait`](Self::wait): gives up on connecting once
    /// nobody answered for [`timeout`](Self::timeout). False once lost.
    fn waited(&mut self, now: Instant) -> bool {
        if self.state == NetState::Connecting {
            // Connecting has its own patience: the other side may still be starting.
            let s = self.session.as_deref_mut().expect("online");
            let since = *s.connecting_since.get_or_insert(now);
            if now - since > self.timeout {
                return self.lose("nobody answered".into());
            }
        }
        self.state != NetState::Lost
    }

    /// One step of the goodbye (see `Drop`): keeps resending what peers
    /// still miss. True once it is over (everyone has our inputs, has left
    /// or went quiet, or `end` passed) and the BYEs are out.
    fn linger_step(&mut self, now: Instant, end: Instant) -> bool {
        let Some(s) = self.session.as_deref_mut() else {
            return true;
        };
        s.poll(
            now,
            &mut Settings {
                player: &mut self.player,
                players: &mut self.players,
                delay: &mut self.delay,
                checksum_every: &mut self.checksum_every,
                desync: &mut self.desync,
            },
        );
        if now >= end || s.settled(now, self.player) {
            // Lost BYEs only cost the others GONE of waiting.
            for i in 0..s.peers.len() {
                let addr = s.peers[i].addr;
                s.send_simple(addr, BYE);
            }
            return true;
        }
        s.flush(now, self.player, self.players);
        false
    }
}

impl Drop for Net {
    /// A peer that stops (its last tick ran, or it quit) may hold inputs
    /// the others still miss, because a packet got lost: it keeps resending
    /// until every peer has them, has left too, or a second passed.
    fn drop(&mut self) {
        let Some(s) = self.session.as_deref() else { return };
        if !self.linger || !s.started || self.state == NetState::Lost {
            return;
        }
        let end = Instant::now() + LINGER.min(self.timeout);
        while !self.linger_step(Instant::now(), end) {
            std::thread::sleep(Duration::from_millis(1));
        }
    }
}

/// What the host tells joining players.
struct Settings<'a> {
    player: &'a mut usize,
    players: &'a mut usize,
    delay: &'a mut u32,
    checksum_every: &'a mut u32,
    desync: &'a mut Option<Desync>,
}

/// One player's inputs, by tick.
struct Stream {
    /// The first tick not received yet (everything before it is in).
    have: u64,
    ring: Vec<(u64, Vec<u8>)>,
}

impl Stream {
    fn new(delay: u32) -> Self {
        Self {
            // The first `delay` ticks have no input from anyone.
            have: u64::from(delay),
            ring: (0..RING).map(|_| (u64::MAX, Vec::new())).collect(),
        }
    }
    fn push(&mut self, tick: u64, data: &[u8]) {
        if tick < self.have || tick >= self.have + RING as u64 {
            return;
        }
        let slot = &mut self.ring[tick as usize % RING];
        slot.0 = tick;
        slot.1.clear();
        slot.1.extend_from_slice(data);
        while self.ring[self.have as usize % RING].0 == self.have {
            self.have += 1;
        }
    }
    fn get(&self, tick: u64) -> &[u8] {
        let slot = &self.ring[tick as usize % RING];
        if slot.0 == tick { &slot.1 } else { &[] }
    }
}

enum Role {
    Host,
    Client(SocketAddr),
}

struct Peer {
    addr: SocketAddr,
    player: usize,
    /// Per stream: the first tick this peer doesn't have.
    acked: [u64; MAX_PLAYERS],
    /// Per stream: what we last sent it up to.
    sent: [u64; MAX_PLAYERS],
    last_heard: Instant,
    last_sent: Option<Instant>,
    /// Sent us inputs, so it knows the game started.
    running: bool,
    /// Said BYE: it stopped and needs nothing more.
    left: bool,
}

struct Session {
    transport: Box<dyn Transport>,
    role: Role,
    peers: Vec<Peer>,
    streams: Vec<Stream>,
    started: bool,
    seed: u64,
    last_hello: Option<Instant>,
    last_start: Option<Instant>,
    connecting_since: Option<Instant>,
    error: Option<String>,
    own_checks: VecDeque<Check>,
    /// Peers' checks for ticks we haven't checked yet: (player, check).
    their_checks: VecDeque<(usize, Check)>,
    out: Vec<u8>,
    buf: Vec<u8>,
    stats: NetStats,
}

impl Session {
    fn new(transport: Box<dyn Transport>, role: Role) -> Self {
        Self {
            transport,
            role,
            peers: Vec::new(),
            streams: Vec::new(),
            started: false,
            seed: 0,
            last_hello: None,
            last_start: None,
            connecting_since: None,
            error: None,
            own_checks: VecDeque::new(),
            their_checks: VecDeque::new(),
            out: Vec::with_capacity(PACKET),
            buf: vec![0; 64 * 1024],
            stats: NetStats::default(),
        }
    }

    fn poll(&mut self, now: Instant, settings: &mut Settings) {
        if self.streams.is_empty() && matches!(self.role, Role::Host) {
            self.streams = (0..*settings.players).map(|_| Stream::new(*settings.delay)).collect();
        }
        if let Some(e) = self.transport.error() {
            self.error = Some(e);
        }
        while let Some((from, n)) = self.transport.recv(&mut self.buf) {
            let packet = std::mem::take(&mut self.buf);
            self.receive(from, &packet[..n], now, settings);
            self.buf = packet;
        }
        match self.role {
            Role::Host => {
                if !self.started && self.peers.len() + 1 == *settings.players {
                    self.start(now);
                }
                if self.started && self.last_start.is_none_or(|t| now - t >= RESEND) {
                    self.last_start = Some(now);
                    for i in 0..self.peers.len() {
                        if !self.peers[i].running {
                            let addr = self.peers[i].addr;
                            self.send_simple(addr, START);
                        }
                    }
                }
            }
            Role::Client(host) => {
                if self.peers.is_empty() && self.last_hello.is_none_or(|t| now - t >= HELLO_EVERY) {
                    self.last_hello = Some(now);
                    self.begin(HELLO);
                    self.out.extend_from_slice(&self.seed.to_le_bytes());
                    self.emit(host);
                }
            }
        }
    }

    /// Sends the packet in `out`.
    fn emit(&mut self, to: SocketAddr) {
        self.stats.packets_sent += 1;
        self.stats.bytes_sent += self.out.len() as u64;
        self.transport.send(to, &self.out);
    }

    fn begin(&mut self, kind: u8) {
        self.out.clear();
        self.out.extend_from_slice(&MAGIC);
        self.out.push(VERSION);
        self.out.push(kind);
    }
    fn send_simple(&mut self, to: SocketAddr, kind: u8) {
        self.begin(kind);
        self.emit(to);
    }

    fn receive(&mut self, from: SocketAddr, packet: &[u8], now: Instant, settings: &mut Settings) {
        let mut r = Reader(packet);
        if r.take(4) != Some(&MAGIC[..]) || r.u8() != Some(VERSION) {
            return;
        }
        let Some(kind) = r.u8() else { return };
        if let Some(peer) = self.peers.iter_mut().find(|p| p.addr == from) {
            peer.last_heard = now;
        }
        match (kind, &self.role) {
            (HELLO, Role::Host) => {
                let Some(seed) = r.u64() else { return };
                if seed != self.seed {
                    self.begin(REJECT);
                    self.out.push(1);
                    self.emit(from);
                    return;
                }
                let player = match self.peers.iter().find(|p| p.addr == from) {
                    Some(p) => p.player,
                    None if self.peers.len() + 1 < *settings.players => {
                        let player = self.peers.len() + 1;
                        self.peers.push(Peer::new(from, player, now));
                        player
                    }
                    None => {
                        self.begin(REJECT);
                        self.out.push(2);
                        self.emit(from);
                        return;
                    }
                };
                self.begin(WELCOME);
                self.out.push(player as u8);
                self.out.push(*settings.players as u8);
                self.out.extend_from_slice(&settings.delay.to_le_bytes());
                self.out.extend_from_slice(&settings.checksum_every.to_le_bytes());
                self.emit(from);
            }
            (WELCOME, Role::Client(host)) if from == *host && self.peers.is_empty() => {
                let (Some(player), Some(players), Some(delay), Some(every)) = (r.u8(), r.u8(), r.u32(), r.u32()) else {
                    return;
                };
                let (player, players) = (player as usize, players as usize);
                if player == 0 || player >= players || players > MAX_PLAYERS || delay > MAX_DELAY {
                    return;
                }
                *settings.player = player;
                *settings.players = players;
                *settings.delay = delay;
                *settings.checksum_every = every;
                self.streams = (0..players).map(|_| Stream::new(delay)).collect();
                self.peers.push(Peer::new(from, 0, now));
            }
            (START, Role::Client(host)) if from == *host && !self.peers.is_empty() && !self.started => self.start(now),
            (REJECT, Role::Client(host)) if from == *host => {
                self.error = Some(match r.u8() {
                    Some(1) => "the host uses a different Game::seed".into(),
                    _ => "the session is full".into(),
                });
            }
            (INPUTS, _) => self.receive_inputs(from, r, now, settings),
            (BYE, _) => {
                if let Some(peer) = self.peers.iter_mut().find(|p| p.addr == from) {
                    peer.left = true;
                }
            }
            (CHECK, _) => {
                let Some(i) = self.peers.iter().position(|p| p.addr == from) else {
                    return;
                };
                let Some(check) = Check::read(&mut r) else { return };
                let player = self.peers[i].player;
                if settings.desync.is_none() {
                    match self.own_checks.iter().find(|c| c.tick == check.tick) {
                        Some(own) => {
                            *settings.desync = own.compare(&check, player);
                            if let Some(d) = settings.desync.as_ref() {
                                eprintln!("ponko lockstep: {d}");
                            }
                        }
                        None => {
                            self.their_checks.push_back((player, check));
                            if self.their_checks.len() > 64 {
                                self.their_checks.pop_front();
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// The game is on: from now on silence counts against peers.
    fn start(&mut self, now: Instant) {
        self.started = true;
        for p in &mut self.peers {
            p.last_heard = now;
        }
    }

    fn receive_inputs(&mut self, from: SocketAddr, mut r: Reader, now: Instant, settings: &Settings) {
        let Some(i) = self.peers.iter().position(|p| p.addr == from) else {
            return;
        };
        let players = *settings.players;
        if self.streams.len() != players {
            return;
        }
        if matches!(self.role, Role::Client(_)) && !self.started {
            self.start(now); // inputs mean the game is on, even if START got lost
        }
        self.peers[i].running = true;
        let Some(n) = r.u8() else { return };
        for acked in self.peers[i].acked.iter_mut().take(n as usize) {
            let Some(have) = r.u64() else { return };
            *acked = (*acked).max(have);
        }
        let Some(count) = r.u8() else { return };
        for _ in 0..count {
            let (Some(stream), Some(first), Some(ticks)) = (r.u8(), r.u64(), r.u8()) else {
                return;
            };
            let stream = stream as usize;
            // Only the host relays others' inputs; a client may only send its own.
            let allowed = stream < players
                && match self.role {
                    Role::Host => stream == self.peers[i].player,
                    Role::Client(_) => stream != *settings.player,
                };
            for t in 0..u64::from(ticks) {
                let Some(len) = r.u16() else { return };
                let Some(data) = r.take(len as usize) else { return };
                if allowed {
                    self.streams[stream].push(first + t, data);
                }
            }
        }
    }

    /// Sends each peer the inputs it doesn't have yet, and our acks.
    fn flush(&mut self, now: Instant, me: usize, players: usize) {
        if !self.started || self.streams.len() != players {
            return;
        }
        for i in 0..self.peers.len() {
            let peer = &self.peers[i];
            let streams: &[usize] = match self.role {
                Role::Host => &[0, 1, 2, 3, 4, 5, 6, 7][..players],
                Role::Client(_) => &[me],
            };
            let fresh = streams
                .iter()
                .any(|&s| s != peer.player && self.streams[s].have > peer.sent[s]);
            if !fresh && peer.last_sent.is_some_and(|t| now - t < RESEND) {
                continue;
            }
            // One packet, or several when the streams don't fit in one
            // (large inputs from many players): every stream gets its turn
            // in this flush, and no packet goes over PACKET.
            let mut sent = self.peers[i].sent;
            let mut done = [false; MAX_PLAYERS];
            loop {
                self.begin(INPUTS);
                self.out.push(players as u8);
                for st in &self.streams {
                    self.out.extend_from_slice(&st.have.to_le_bytes());
                }
                let count_at = self.out.len();
                self.out.push(0);
                let mut more = false;
                for &s in streams {
                    let peer = &self.peers[i];
                    if s == peer.player || done[s] {
                        continue;
                    }
                    let st = &self.streams[s];
                    let first = peer.acked[s];
                    if first >= st.have {
                        done[s] = true;
                        continue;
                    }
                    // The block's header and its first tick go together.
                    // They always fit an empty packet (asserted next to PACKET).
                    if self.out.len() + BLOCK_HEADER + st.get(first).len() > PACKET {
                        more = true;
                        continue;
                    }
                    done[s] = true;
                    let ticks_at = self.out.len() + 9;
                    self.out.push(s as u8);
                    self.out.extend_from_slice(&first.to_le_bytes());
                    self.out.push(0);
                    let mut ticks = 0u8;
                    let mut t = first;
                    while t < st.have && ticks < u8::MAX {
                        let data = st.get(t);
                        if self.out.len() + 2 + data.len() > PACKET {
                            break;
                        }
                        self.out.extend_from_slice(&(data.len() as u16).to_le_bytes());
                        self.out.extend_from_slice(data);
                        ticks += 1;
                        t += 1;
                    }
                    self.out[ticks_at] = ticks;
                    self.out[count_at] += 1;
                    sent[s] = t;
                }
                let addr = self.peers[i].addr;
                self.emit(addr);
                if !more {
                    break;
                }
            }
            let peer = &mut self.peers[i];
            peer.sent = sent;
            peer.last_sent = Some(now);
        }
    }

    /// Every peer has all the inputs we send it, or has left.
    fn settled(&self, now: Instant, me: usize) -> bool {
        self.peers.iter().all(|p| {
            let has_all = match self.role {
                Role::Host => (0..self.streams.len()).all(|s| s == p.player || p.acked[s] >= self.streams[s].have),
                Role::Client(_) => p.acked[me] >= self.streams[me].have,
            };
            has_all || p.left || now - p.last_heard > GONE
        })
    }

    fn silent_peer(&self, now: Instant, timeout: Duration) -> Option<usize> {
        self.peers
            .iter()
            .find(|p| now - p.last_heard > timeout)
            .map(|p| p.player)
    }

    fn send_check(&mut self, check: &Check) {
        self.begin(CHECK);
        check.write(&mut self.out);
        for i in 0..self.peers.len() {
            let addr = self.peers[i].addr;
            self.emit(addr);
        }
    }

    /// Compares a fresh own check with peers' checks that arrived first.
    fn compare_stored(&mut self, own: &Check) -> Option<Desync> {
        let mut found = None;
        self.their_checks.retain(|(player, theirs)| {
            if theirs.tick != own.tick {
                return theirs.tick > own.tick;
            }
            if found.is_none() {
                found = own.compare(theirs, *player);
            }
            false
        });
        found
    }
}

impl Peer {
    fn new(addr: SocketAddr, player: usize, now: Instant) -> Self {
        Self {
            addr,
            player,
            acked: [0; MAX_PLAYERS],
            sent: [0; MAX_PLAYERS],
            last_heard: now,
            last_sent: None,
            running: false,
            left: false,
        }
    }
}

/// A hash of the reflected world at one tick, per component.
struct Check {
    tick: u64,
    total: u64,
    parts: Vec<(String, u64)>,
}

fn fnv(mut h: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100_0000_01b3);
    }
    h
}
const FNV_START: u64 = 0xcbf2_9ce4_8422_2325;

impl Check {
    fn of(world: &World, tick: u64) -> Check {
        let save = world.save();
        let mut parts: BTreeMap<String, u64> = BTreeMap::new();
        for e in save["entities"].as_array().into_iter().flatten() {
            let id = e["entity"].as_u64().unwrap_or(0).to_le_bytes();
            for (name, value) in e["components"].as_object().into_iter().flatten() {
                let h = parts.entry(name.clone()).or_insert(FNV_START);
                *h = fnv(fnv(*h, &id), value.to_string().as_bytes());
            }
        }
        for (name, value) in save["resources"].as_object().into_iter().flatten() {
            parts.insert(format!("resource:{name}"), fnv(FNV_START, value.to_string().as_bytes()));
        }
        let parts: Vec<(String, u64)> = parts.into_iter().collect();
        let total = parts
            .iter()
            .fold(FNV_START, |h, (n, v)| fnv(fnv(h, n.as_bytes()), &v.to_le_bytes()));
        Check { tick, total, parts }
    }
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.tick.to_le_bytes());
        out.extend_from_slice(&self.total.to_le_bytes());
        let parts = self.parts.len().min(64);
        out.push(parts as u8);
        for (name, hash) in &self.parts[..parts] {
            let name = &name.as_bytes()[..name.len().min(255)];
            out.push(name.len() as u8);
            out.extend_from_slice(name);
            out.extend_from_slice(&hash.to_le_bytes());
        }
    }
    fn read(r: &mut Reader) -> Option<Check> {
        let tick = r.u64()?;
        let total = r.u64()?;
        let n = r.u8()?;
        let mut parts = Vec::with_capacity(n as usize);
        for _ in 0..n {
            let len = r.u8()?;
            let name = String::from_utf8_lossy(r.take(len as usize)?).into_owned();
            parts.push((name, r.u64()?));
        }
        Some(Check { tick, total, parts })
    }
    fn compare(&self, theirs: &Check, player: usize) -> Option<Desync> {
        if self.total == theirs.total {
            return None;
        }
        let mut components: Vec<String> = Vec::new();
        for (name, hash) in &self.parts {
            if theirs.parts.iter().find(|(n, _)| n == name).map(|(_, h)| h) != Some(hash) {
                components.push(name.clone());
            }
        }
        for (name, _) in &theirs.parts {
            if !self.parts.iter().any(|(n, _)| n == name) {
                components.push(name.clone());
            }
        }
        Some(Desync {
            tick: self.tick,
            player,
            components,
        })
    }
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if self.0.len() < n {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }
    fn u16(&mut self) -> Option<u16> {
        self.take(2).map(|b| u16::from_le_bytes(b.try_into().expect("2 bytes")))
    }
    fn u32(&mut self) -> Option<u32> {
        self.take(4).map(|b| u32::from_le_bytes(b.try_into().expect("4 bytes")))
    }
    fn u64(&mut self) -> Option<u64> {
        self.take(8).map(|b| u64::from_le_bytes(b.try_into().expect("8 bytes")))
    }
}

/// Moves packets: UDP, or memory for loopback sessions.
trait Transport: Send {
    fn send(&mut self, to: SocketAddr, data: &[u8]);
    fn recv(&mut self, buf: &mut [u8]) -> Option<(SocketAddr, usize)>;
    fn local_addr(&self) -> SocketAddr;
    /// A failure the transport found on its own (a lobby that couldn't be joined).
    fn error(&mut self) -> Option<String> {
        None
    }
    /// The Steam client and lobby, for Steam sessions.
    #[cfg(feature = "steam")]
    fn steam(&self) -> Option<(&steamworks::Client, Option<steamworks::LobbyId>)> {
        None
    }
}

struct Udp(UdpSocket);
impl Udp {
    fn bind(addr: impl ToSocketAddrs) -> io::Result<Udp> {
        let socket = UdpSocket::bind(addr)?;
        socket.set_nonblocking(true)?;
        Ok(Udp(socket))
    }
}
impl Transport for Udp {
    fn send(&mut self, to: SocketAddr, data: &[u8]) {
        // Lost packets are resent anyway; a full buffer is a lost packet.
        let _ = self.0.send_to(data, to);
    }
    fn recv(&mut self, buf: &mut [u8]) -> Option<(SocketAddr, usize)> {
        // Windows reports an earlier send to a closed port as a receive
        // error; skip those. A socket that only fails must not hold the
        // game thread, so give up for this poll after a few in a row: the
        // silence timeout then ends the session.
        for _ in 0..UDP_ERROR_SKIPS {
            match self.0.recv_from(buf) {
                Ok((n, from)) => return Some((from, n)),
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => return None,
                Err(_) => {}
            }
        }
        None
    }
    fn local_addr(&self) -> SocketAddr {
        self.0.local_addr().expect("bound socket")
    }
}

/// A packet in memory: who sent it, and its bytes.
type Packet = (SocketAddr, Vec<u8>);

struct Local {
    me: SocketAddr,
    rx: mpsc::Receiver<Packet>,
    peers: std::sync::Arc<Vec<mpsc::Sender<Packet>>>,
    drop_one_in: u32,
    sent: u32,
}
impl Local {
    fn addr(i: usize) -> SocketAddr {
        SocketAddr::from(([127, 0, 0, 1], 1 + i as u16))
    }
}
impl Transport for Local {
    fn send(&mut self, to: SocketAddr, data: &[u8]) {
        self.sent = self.sent.wrapping_add(1);
        // Drop about one in `drop_one_in`, scattered: dropping exactly every
        // nth packet can lock onto one peer when a sender sends n packets
        // per poll, so that peer would never hear from it.
        let mix = (u64::from(self.sent) | u64::from(self.me.port()) << 32).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        if self.drop_one_in > 0 && (mix >> 40).is_multiple_of(u64::from(self.drop_one_in)) {
            return;
        }
        if let Some(tx) = self.peers.get(to.port() as usize - 1) {
            let _ = tx.send((self.me, data.to_vec()));
        }
    }
    fn recv(&mut self, buf: &mut [u8]) -> Option<(SocketAddr, usize)> {
        let (from, data) = self.rx.try_recv().ok()?;
        let n = data.len().min(buf.len());
        buf[..n].copy_from_slice(&data[..n]);
        Some((from, n))
    }
    fn local_addr(&self) -> SocketAddr {
        self.me
    }
}

/// Steamworks SDK used by Steam transports.
#[cfg(feature = "steam")]
pub use steamworks as steam;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_fill_in_order_and_ignore_old_ticks() {
        let mut s = Stream::new(2);
        assert_eq!(s.have, 2);
        s.push(3, b"b");
        assert_eq!(s.have, 2, "tick 2 is missing");
        s.push(2, b"a");
        assert_eq!(s.have, 4);
        s.push(2, b"x");
        assert_eq!(s.get(2), b"a", "duplicates are ignored");
        assert!(s.get(0).is_empty(), "ticks before the delay are empty");
    }

    #[test]
    fn checks_name_the_components_that_differ() {
        let mut a = World::new();
        a.reflect::<crate::d2::Transform>();
        a.spawn((crate::d2::Transform::default(),));
        let mut b = World::new();
        b.reflect::<crate::d2::Transform>();
        b.spawn((crate::d2::Transform::at(glam::vec2(1.0, 0.0)),));
        let (ca, cb) = (Check::of(&a, 5), Check::of(&b, 5));
        assert!(ca.compare(&Check::of(&a, 5), 1).is_none());
        let d = ca.compare(&cb, 1).expect("different");
        assert_eq!(d.components, ["Transform2D"]);
        let mut out = Vec::new();
        cb.write(&mut out);
        let back = Check::read(&mut Reader(&out)).unwrap();
        assert_eq!((back.tick, back.total, back.parts), (cb.tick, cb.total, cb.parts));
    }
}
