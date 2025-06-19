// websocket listener for the ducktb protocol

mod proto_json_v2;

const PROTOCOLS: [&'static str; 2] = ["json-v2", "msgpack-v1"];

use crate::config::{ListenerConfig, ListenAddress};
use crate::types::{RemoteAddress, UserID, RequestID, C2S, S2C};

use std::collections::HashMap;
use std::fs::File;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use derive_new::new;
use futures_util::future::{select_all, join_all};
use nix::fcntl::{Flock, FlockArg};
use tokio::net::{UnixListener, UnixStream, TcpListener, TcpStream};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{Sender, Receiver, channel};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::interval;
use tokio::{spawn, select};
use tokio_tungstenite::accept_hdr_async;
use tokio_tungstenite::tungstenite::{
	http::{StatusCode, HeaderValue},
	handshake::server::{Request, Response, ErrorResponse},
	protocol::CloseFrame
};

// some abstraction over tcp and unix sockets
enum ListenerType {
	TCP(TcpListener),
	// flock must be stored along with the listener and dropped with it (we don't use the lock file itself though)
	Unix(UnixListener, #[allow(unused)] Flock<File>),
}
enum StreamType {
	TCP(TcpStream),
	Unix(UnixStream),
}
impl ListenerType {
	// TODO: figure out errors
	pub async fn accept(&self) -> Result<(StreamType, RemoteAddress), anyhow::Error> {
		Ok(match self {
			Self::TCP(l)     => l.accept().await.map(|e| {
				// try extracting an ipv4 address from an ipv6 mapped representation
				let addr = if let SocketAddr::V6(a) = e.1 {
					a.ip().to_ipv4_mapped().map(|e| IpAddr::V4(e)).unwrap_or_else(|| e.1.ip())
				} else { e.1.ip() };
				(StreamType::TCP(e.0), RemoteAddress::IP(addr))
			})?,
			Self::Unix(l, _) => l.accept().await.and_then(|e| {
				let cred = e.0.peer_cred()?;
				Ok((StreamType::Unix(e.0), RemoteAddress::Unix(cred)))
			})?,
		})
	}
}

/// s2c
#[derive(Debug, Clone)]
pub enum ListenerRxOp {
	ReloadCfg(ListenerConfig),
	Shutdown(String),
	Event(UserID, S2C),
	Keepalive,
}

/// c2s
#[derive(Debug, Clone)]
pub enum ListenerTxOp {
	Request(UserID, Option<RequestID>, C2S),
	Connect(UserID, RemoteAddress),
	Disconnect(UserID)
}


pub struct Listener {
	config: ListenerConfig,
	listeners: Vec<ListenerType>,
	connection_jh: Vec<JoinHandle<()>>,
	state: Arc<Mutex<AcceptersState>>,

	tx: Sender<ListenerTxOp>,
}

#[derive(new)]
struct AcceptersState {
	// incremental connection ids
	#[new(value="0")]
	next_sid: u32,
	#[new(value="HashMap::new()")]
	connections: HashMap<UserID, Connection>
}

#[derive(new)]
struct Connection {
	tx2: Sender<ListenerRxOp>,
}

impl Listener {
	pub async fn run(config: ListenerConfig, tx: Sender<ListenerTxOp>, mut rx: Receiver<ListenerRxOp>) -> Result<(), ()> {
		let mut s = Self {
			config, tx,
			listeners: vec![],
			connection_jh: vec![],
			state: Arc::new(Mutex::new(AcceptersState::new())),
		};
		s.bind_listeners().await?;

		let mut timer = interval(if s.config.ping_interval == 0 {
			Duration::MAX
		} else {
			Duration::from_secs(s.config.ping_interval.into())
		});

		loop {
			select! {
				_ = (&mut timer).tick() => {
					s.send_to_all(ListenerRxOp::Keepalive).await;
				}
				_ = s.accept_all() => {},
				msg = rx.recv() => {
					//info!("s2c msg {:?}", msg);
					if let None = msg { break }
					let msg = msg.unwrap();
					match msg {
						ListenerRxOp::ReloadCfg(_cfg) => todo!("support for reloading listener cfg"),
						ListenerRxOp::Event(sid, ref _evt) => s.send_to_conn(sid, msg).await,
						ListenerRxOp::Shutdown(ref _reason) => {
							s.send_to_all(msg).await;
							break
						}
						ListenerRxOp::Keepalive => unreachable!("only generated internally"),
					}
				},
			}
		}

		info!("waiting for connection threads to exit");
		join_all(s.connection_jh.iter_mut()).await;
		info!("listener thread exiting");
		Ok(())
	}
	async fn bind_listeners(&mut self) -> Result<(), ()> {
		let mut successful_binds: usize = 0;
		// drop all existing listeners
		self.listeners.clear();
		for listen in &self.config.listen {
			match listen {
				ListenAddress::Unix(path) => {
					let mut lock_path = path.clone();
					// TODO: use add_extension once it becomes stable
					{
						let mut ext = lock_path.extension().unwrap().to_os_string();
						ext.push(".lock");
						lock_path.set_extension(ext);
					}
					//debug!("{}", lock_path.display());
					let lock_file = match File::create(&lock_path) {
						Ok(f) => f,
						Err(e) => {
							error!("failed to open lock file {}: {}", lock_path.display(), e);
							continue
						}
					};
					let lock_file = match Flock::lock(lock_file, FlockArg::LockExclusiveNonblock) {
						Ok(f) => f,
						Err(_) => {
							error!("failed to lock file {} -- perhaps another server is listening?", lock_path.display());
							continue
						}
					};
					let _ = std::fs::remove_file(&path);
					match UnixListener::bind(path) {
						Ok(s) => {
							info!("listening on unix sock {}", path.display());
							self.listeners.push(ListenerType::Unix(s, lock_file));
							successful_binds += 1;
						},
						Err(e) => {
							error!("failed to bind unix sock to {}: {}", path.display(), e);
						}
					}
				},
				ListenAddress::TCP(addr) => {
					match TcpListener::bind(addr).await {
						Ok(s) => {
							info!("listening on tcp sock {}", addr);
							successful_binds += 1;
							self.listeners.push(ListenerType::TCP(s));
						},
						Err(e) => {
							error!("failed to bind tcp sock to {}: {}", addr, e);
						}
					}
				}
			}
		}
		if successful_binds == 0 {
			error!("failed to bind any socket");
			Err(())
		} else {
			Ok(())
		}
	}


	async fn accept_all(&mut self) -> () {
		let futures = self.listeners.iter_mut().map(|listener| Box::pin(listener.accept()));
		match select_all(futures).await.0 {
			Ok((StreamType::TCP(conn), addr)) => {
				//info!("tcp connection {:?}", addr);
				spawn(Self::accept(self.config.clone(), self.state.clone(), self.tx.clone(), conn, addr));
			},
			Ok((StreamType::Unix(conn), addr)) => {
				//info!("unix connection {:?}", addr);
				spawn(Self::accept(self.config.clone(), self.state.clone(), self.tx.clone(), conn, addr));
			},
			Err(s) => warn!("received error while accepting: {}", s),
		}
	}

	// S is what tungstenite expects for a stream
	async fn accept<S>(config: ListenerConfig, state: Arc<Mutex<AcceptersState>>,
		tx: Sender<ListenerTxOp>,
		stream: S, addr: RemoteAddress)
		where S: AsyncRead + AsyncWrite + Unpin {
		debug!("handling connection from {:?}", addr);
		if let Err(err) = Self::accept_inner(config, state, tx, stream, addr).await {
			warn!("received error while accepting: {}", err);
		}
	}
	async fn accept_inner<S>(config: ListenerConfig, state: Arc<Mutex<AcceptersState>>,
		tx: Sender<ListenerTxOp>,
		stream: S, mut addr: RemoteAddress) -> Result<(), anyhow::Error>
		where S: AsyncRead + AsyncWrite + Unpin {
		let mut err_resp = ErrorResponse::new(None);
		*err_resp.status_mut() = StatusCode::BAD_REQUEST;
		let mut used_protocol = "".to_string();
		let headcb = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
			for (k, v) in req.headers().iter() {
				if k == "x-forwarded-for" && config.trust_real_ip_header {
					debug!("x-forwarded-for: {:?}", v);
					if let Ok(str) = v.to_str() {
						let str = str.split(',').next_back().unwrap_or("");
						if let Some(ip) = str.parse().ok() {
							addr = RemoteAddress::IP(ip);
						}
					}
				}
				if k == "sec-websocket-protocol" {
					debug!("sec-websocket-protocol: {:?}", v);
					if let Ok(str) = v.to_str() {
						for q in str.split(',').map(|e| e.trim()) {
							if PROTOCOLS.iter().any(|&i| i==q) {
								used_protocol = q.to_string();
								break;
							}
						}
					}
				}
			}
			if used_protocol == "" {
				*err_resp.body_mut() = Some("whoops you forgot to set the websocket protocol...\n".into());
				warn!("websocket protocol was not set");
				return Err(err_resp);
			}
			resp.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_str(&used_protocol).expect("websocket protocol name should be ascii"));
			Ok(resp)
		};
		let mut ws_stream = accept_hdr_async(stream, headcb).await?;

		// completely arbitrary value
		let (tx2, rx2) = channel(8);

		let conn = Connection { tx2 };

		let sid = {
			let mut state = state.lock().await;
			let mut sid;
			// in case we actually do have u32::MAX-1 connections (u32::MAX being the system user)
			// this will loop forever
			loop {
				sid = UserID::new(state.next_sid);
				state.next_sid = if state.next_sid == u32::MAX-1 { 0 } else { state.next_sid + 1 };
				if let None = state.connections.get(&sid) {
					state.connections.insert(sid, conn);
					break sid;
				}
			}
		};

		info!("accepted connection from {} (sid={}) with protocol {}", addr, sid, used_protocol);
		tx.send(ListenerTxOp::Connect(sid, addr)).await.unwrap();

		let close_reason = match &*used_protocol {
			"json-v2" => proto_json_v2::handle(&mut ws_stream, sid, tx.clone(), rx2).await,
			"msgpack-v1" => { error!("msgpack-v1 not implemented yet"); "".into() },
			_ => unreachable!(),
		};

		tx.send(ListenerTxOp::Disconnect(sid)).await.unwrap();
		let _ = ws_stream.close(Some(CloseFrame { code: 3001.into(), reason: close_reason.into() } )).await;
		Ok(())
	}
	async fn send_to_conn(&self, sid: UserID, msg: ListenerRxOp) {
		let state = self.state.lock().await;
		match state.connections.get(&sid) {
			Some(conn) => {
				let _ = conn.tx2.send(msg.clone()).await;
			}
			None => debug!("received message for sid={} but it's gone\n{:?}", sid, msg),
		}
	}
	async fn send_to_all(&self, msg: ListenerRxOp) {
		let state = self.state.lock().await;
		let futs = state.connections.values().map(|conn| conn.tx2.send(msg.clone()));
		join_all(futs).await; // who cares if it got to the clients amirite
	}
}

