use crate::config::*;
use crate::types::*;
use futures_util::{SinkExt, StreamExt};
use std::time::Instant;
use tokio::{select, spawn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{Sender, channel};
use tokio_tungstenite::{
	accept_hdr_async,
	tungstenite::{Message, handshake::server::{Request, Response, ErrorResponse}}
};


pub async fn listen(l: TcpListener, t: Sender<ClientOp>) {
	let mut conn_seq = 0x48aeb931u32;
	while let Ok((flow, _)) = l.accept().await {
		spawn(conn(flow, UserID::new(conn_seq), t.clone()));
		conn_seq += 1984;
	}
}

async fn conn(y: TcpStream, ee: UserID, t: Sender<ClientOp>) {
	let addr = y.peer_addr().expect("what da hell man");
	let mut uip = addr.ip();
	let headcb = |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
		for (k, v) in req.headers().iter() {
			if k == "x-forwarded-for" && TRUST_REAL_IP_HEADER {
				let str = v.to_str().ok();
				if let Some(str) = str {
					let str = str.split(',').next_back().unwrap_or("");
					if let Some(ip) = str.parse().ok() {
						uip = ip;
					}
				}
			}
		}
		return Result::Ok(resp);
	};
	let bs = accept_hdr_async(y, headcb).await;
	if bs.is_err() { return }
	let bs = bs.unwrap();
	let (mut tws, mut rws) = bs.split();

	let (tx, mut messages) = channel(48);
	let balls = Susser {
		counter: SusRate::new(),
		is_typing: false,
		u: User {
			color: UserColor::default(),
			nick: UserNick::default(),
			haship: hash_ip(&uip),
			id: ee
		},
		rooms: vec![],
		ip: uip,
		tx: tx,
	};
	let mut msg_1st = true;
	if t.send(ClientOp::Connection(ee, balls)).await.is_err() { return };
	loop {
		select!{
			msg = rws.next() => {
				match msg {
					Some(Ok(Message::Text(str))) => {
						if message(str, ee, &t, msg_1st).await.is_none() { break }
					}
					_ => break
				}
				msg_1st = false;
			}
			msg = messages.recv() => {
				if let Some(msg) = msg {
					if tws.send(Message::Text(match msg {
						ServerOp::Disconnect => break,
						ServerOp::MsgHello(s) =>      format!("HELLO\0{}",            serde_json::to_string(&s).unwrap()),
						ServerOp::MsgMouse(s) =>      format!("MOUSE\0{}",            serde_json::to_string(&s).unwrap()),
						ServerOp::MsgRoom(s) =>       format!("ROOM\0{}",             serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserUpdate(s) => format!("USER_UPDATE\0{}",      serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserJoined(s) => format!("USER_JOINED\0{}",      serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserChNick(s) => format!("USER_CHANGE_NICK\0{}", serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserLeft(s) =>   format!("USER_LEFT\0{}",        serde_json::to_string(&s).unwrap()),
						ServerOp::MsgTyping(s) =>     format!("TYPING\0{}",           serde_json::to_string(&s).unwrap()),
						ServerOp::MsgMessage(s) =>    format!("MESSAGE\0{}",          serde_json::to_string(&s).unwrap()),
						ServerOp::MsgHistory(s) =>    format!("HISTORY\0{}",          serde_json::to_string(&s).unwrap()),
						ServerOp::MsgRateLimits(s) => format!("RATE_LIMITS\0{}",      serde_json::to_string(&s).unwrap()),
					})).await.is_err() { break }
				}
			}
		}
	}
	let start_time = Instant::now();
	let _ = t.send(ClientOp::Disconnect(ee)).await;
	println!("WORKER: [{}] sending Disconnect took {}us", ee, start_time.elapsed().as_micros());
}

async fn message(str: String, uid: UserID, t: &Sender<ClientOp>, first: bool) -> Option<()> {
	let f = str.find("\0")?;
	let (tp, rr) = str.split_at(f + 1);
	let (tp, _) = tp.split_at(tp.len() - 1);
	if first {
		if tp != "USER_JOINED" { return None }
		let s = serde_json::from_str::<C2SUserJoined>(&rr).ok()?;
		t.send(ClientOp::MsgUserJoined(uid, s)).await.ok()?;
	} else {
		let start_time = Instant::now();
		// we want to die if we ever hit backpressure
		t.try_send(match tp {
			"MOUSE"            => ClientOp::MsgMouse     (uid, serde_json::from_str::<C2SMouse>(&rr).ok()?),
			"TYPING"           => ClientOp::MsgTyping    (uid, serde_json::from_str::<C2STyping>(&rr).ok()?),
			"MESSAGE"          => ClientOp::MsgMessage   (uid, serde_json::from_str::<C2SMessage>(&rr).ok()?),
			"ROOM"             => ClientOp::MsgRoom      (uid, serde_json::from_str::<C2SRoom>(&rr).ok()?),
			"USER_CHANGE_NICK" => ClientOp::MsgUserChNick(uid, serde_json::from_str::<C2SUserChNick>(&rr).ok()?),
			//"" => ClientOp::Msg(uid, serde_json::from_str::<C2S>(&rr).ok()?),
			_ => { println!("received {}, which is unimplemented...", tp); return None }
		}).ok()?;
		println!("WORKER: [{}] sending {} took {}us", uid, tp, start_time.elapsed().as_micros());
	}
	Some(())
}
