use derive_more::{Display, Deref, From};
use derive_new::new;
use futures::future::join_all;
use futures_util::{SinkExt, StreamExt};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use std::{
	os::fd::AsRawFd,
	env, fmt, str::FromStr,
	collections::{VecDeque, HashMap},
	net::IpAddr,
	time::{Instant, Duration, SystemTime, UNIX_EPOCH}
};
use tokio::{
	net::{TcpListener, TcpStream},
	sync::mpsc::{channel, Sender},
	time::interval,
	spawn, select, join
};
use tokio_tungstenite::{
	accept_hdr_async,
	tungstenite::{Message, handshake::server::{Request, Response, ErrorResponse}}
};

// ========== logic handling side ==========
#[tokio::main]
async fn main() {
	println!("pls work :skull: (listening on {})", wtf);
	while let Some(i) = messages.recv().await {
		eprint!("  MAIN: {:?}\ntook ", i);
		let start_time = Instant::now();
		match i {
		}
		eprintln!("{}us", start_time.elapsed().as_micros());
	}
	let _ = join!(jh1, jh2);
}

// ========== connection handling side ==========
async fn wrap_conn(y: TcpStream, ee: UserID, t: Sender<ClientOp>) {
	conn(y, ee, &t).await;
}

// ========== utility functions ==========
