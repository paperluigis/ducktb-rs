use crate::config::*;
use crate::types::{ClientOp, ServerOp, UserID};
use std::time::Duration;
use futures_util::{SinkExt, StreamExt};
use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::time::interval;
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

macro_rules! s2c_encode {
	($e:ident $($a:ident : $b:ident),*) => {
		match $e {
			$(
				ServerOp::$a(s) => format!(concat!(stringify!($b), "\0{}"), serde_json::to_string(&s).unwrap()),
			)*
		}
	};
}

macro_rules! c2s_decode {
	($e:ident $u:ident $r:ident $($a:ident : $b:ident),*) => {
		match $e {
			$(
			stringify!($a) => ClientOp::$b($u, serde_json::from_str(&$r).map_err(|a| printduck("parsing error", a)).ok()?),
			)*
			//"" => ClientOp::Msg(uid, serde_json::from_str::<C2S>(&rr).map_err(|a| printduck("duckconnect", a)).ok()?),
			_ => { println!("received {}, which is unimplemented...", $e); return None }
		}
	}
}

pub async fn handle(mut bs: WebSocketStream<TcpStream>, messages: &mut Receiver<ServerOp>, t: Sender<ClientOp>, ee: UserID, mut msg_1st: bool) {
	let mut ping = interval(Duration::from_secs(PING_INTERVAL));
	ping.tick().await;
	loop {
		select!{
			msg = bs.next() => {
				match msg {
					Some(Ok(Message::Text(str))) => {
						if message(str, ee, &t, msg_1st).await.is_none() { return }
					}
					// we ignore pings because we don't care
					Some(Ok(Message::Ping(_))) => {}
					Some(Ok(Message::Pong(_))) => {}
					_ => return
				}
				msg_1st = false;
			}
			_ = ping.tick() => {
				// it's pinging time
				// The contents of the payload are not significant, so we can put anything here.
				if bs.send(Message::Ping(vec![114,101,97,100,32,105,102,32,99,117,116,101])).await.is_err() { return }
			}
			msg = messages.recv() => {
				if let Some(msg) = msg {
					if bs.send(Message::Text(s2c_encode!(msg
						MsgHello: HELLO,
						MsgMouse: MOUSE,
						MsgRoom: ROOM,
						MsgUserUpdate: USER_UPDATE,
						MsgUserJoined: USER_JOINED,
						MsgUserChNick: USER_CHANGE_NICK,
						MsgUserLeft: USER_LEFT,
						MsgTyping: TYPING,
						MsgMessage: MESSAGE,
						MsgMessageDM: MESSAGE_DM,
						MsgHistory: HISTORY,
						MsgRateLimits: RATE_LIMITS,
						MsgCustomR: CUSTOM_R,
						MsgCustomU: CUSTOM_U
					))).await.is_err() { return }
				} else { return }
			}
		}
	}
}

fn printduck<T: std::fmt::Debug, U: std::fmt::Display + ?Sized>(b: &U, e: T) -> T {
	println!("{}: {:?}", b, e); e
}

async fn message(str: String, uid: UserID, t: &Sender<ClientOp>, first: bool) -> Option<()> {
	let f = str.find("\0")?;
	let (tp, rr) = str.split_at(f + 1);
	let (tp, _) = tp.split_at(tp.len() - 1);
	if first {
		if tp != "USER_JOINED" { return None }
		t.send(ClientOp::MsgUserJoined(uid, serde_json::from_str(&rr).ok()?)).await.ok()?;
	} else {
		// we want to die if we ever hit backpressure
		t.try_send(c2s_decode!(tp uid rr
			MOUSE: MsgMouse,
			TYPING: MsgTyping,
			MESSAGE: MsgMessage,
			MESSAGE_DM: MsgMessageDM,
			ROOM_JOIN: MsgRoomJoin,
			ROOM_LEAVE: MsgRoomLeave,
			USER_CHANGE_NICK: MsgUserChNick,
			CUSTOM_R: MsgCustomR,
			CUSTOM_U: MsgCustomU
		)).ok()?;
	}
	Some(())
}
