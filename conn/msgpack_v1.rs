use crate::config::*;
use crate::types::{ClientOp, ServerOp, UserID};
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::time::interval;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message, tungstenite::protocol::frame::coding::CloseCode};

macro_rules! s2c_encode {
	($e:ident $($a:ident : $b:expr),*) => {
		match $e {
			$(
				ServerOp::$a(s) => {
					let n = rmp_serde::to_vec(&s).unwrap();
					let mut q = Vec::<u8>::with_capacity(n.len()+1);
					q.push($b);
					q.extend_from_slice(&n);
					q
				},
			)*
		}
	};
}

macro_rules! c2s_decode {
	($e:ident $u:ident $r:ident $($b:ident : $a:expr),*) => {
		match $e[0] {
			$(
			$a => ClientOp::$b($u, rmp_serde::from_slice(&$e[1..]).map_err(|a| (CloseCode::Invalid, format!("{}", a)))?),
			)*
			//"" => ClientOp::Msg(uid, serde_json::from_str::<C2S>(&rr).map_err(|a| printduck("duckconnect", a)).ok()?),
			_ => { return Err((CloseCode::Invalid, format!("message {:>02x} not supported", $e[0]))) }
		}
	}
}

pub async fn handle(bs: &mut WebSocketStream<TcpStream>, messages: &mut Receiver<ServerOp>, t: Sender<ClientOp>, ee: UserID, mut msg_1st: bool) -> (bool, CloseCode, String) {
	let mut ping = interval(Duration::from_secs(PING_INTERVAL));
	ping.tick().await;
	loop {
		select!{
			msg = bs.next() => {
				match msg {
					Some(Ok(Message::Binary(y))) => {
						if let Err(s) = message(y, ee, &t, msg_1st).await { return (false, s.0, s.1) }
					}
					Some(Ok(Message::Close(o))) => {
						return (o.is_some_and(|x| x.code == CloseCode::Away), CloseCode::Normal, String::default())
					}
					// we ignore pings because we don't care
					Some(Ok(Message::Ping(_))) => {}
					Some(Ok(Message::Pong(_))) => {}
					_ => return (false, CloseCode::Unsupported, "expected binary message".into())
				}
				msg_1st = false;
			}
			_ = ping.tick() => {
				// it's pinging time
				// The contents of the payload are not significant, so we can put anything here.
				if bs.send(Message::Ping(vec![114,101,97,100,32,105,102,32,99,117,116,101])).await.is_err() {
					return (true, CloseCode::Normal, String::default())
				}
			}
			msg = messages.recv() => {
				if let Some(msg) = msg {
					if bs.send(Message::Binary(s2c_encode!(msg
						MsgHello: 0xff,
						MsgMouse: 0x10,
						MsgRoom: 0x11,
						MsgUserUpdate: 0x12,
						MsgUserJoined: 0x13,
						MsgUserChNick: 0x14,
						MsgUserLeft: 0x15,
						MsgTyping: 0x16,
						MsgMessage: 0x17,
						MsgMessageDM: 0x18,
						MsgHistory: 0x19,
						MsgRateLimits: 0x20,
						MsgCustomR: 0x21,
						MsgCustomU: 0x22
					))).await.is_err() { return (true, CloseCode::Normal, String::default()) }
				} else { return (true, CloseCode::Normal, String::default()) }
			}
		}
	}
}

async fn message(v: Vec<u8>, uid: UserID, t: &Sender<ClientOp>, first: bool) -> Result<(), (CloseCode, String)> {
	if first {
		t.send(c2s_decode!(v uid rr
			MsgUserJoined: 0x13
		)).await.map_err(|_| { (CloseCode::Away, "server too slow :P".into()) })?;
	} else {
		t.try_send(c2s_decode!(v uid rr
			MsgMouse: 0x10,
			MsgTyping: 0x16,
			MsgMessage: 0x17,
			MsgMessageDM: 0x18,
			MsgRoomJoin: 0x11,
			MsgRoomLeave: 0x12,
			MsgUserChNick: 0x14,
			MsgCustomR: 0x21,
			MsgCustomU: 0x22
		)).map_err(|_| { (CloseCode::Away, "server too slow :P".into()) })?;
	}
	Ok(())
}

