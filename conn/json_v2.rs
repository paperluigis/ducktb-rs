use crate::types::{ClientOp, ServerOp, UserID};
use futures_util::{SinkExt, StreamExt};
use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

pub async fn handle(mut bs: WebSocketStream<TcpStream>, mut messages: Receiver<ServerOp>, t: Sender<ClientOp>, ee: UserID) {
	let mut msg_1st = true;
	loop {
		select!{
			msg = bs.next() => {
				match msg {
					Some(Ok(Message::Text(str))) => {
						if message(str, ee, &t, msg_1st).await.is_none() { return }
					}
					_ => return
				}
				msg_1st = false;
			}
			msg = messages.recv() => {
				if let Some(msg) = msg {
					if bs.send(Message::Text(match msg {
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
					})).await.is_err() { return }
				}
			}
		}
	}
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
		t.try_send(match tp {
			"MOUSE"            => ClientOp::MsgMouse     (uid, serde_json::from_str(&rr).ok()?),
			"TYPING"           => ClientOp::MsgTyping    (uid, serde_json::from_str(&rr).ok()?),
			"MESSAGE"          => ClientOp::MsgMessage   (uid, serde_json::from_str(&rr).ok()?),
			"ROOM_JOIN"        => ClientOp::MsgRoomJoin  (uid, serde_json::from_str(&rr).ok()?),
			"ROOM_LEAVE"       => ClientOp::MsgRoomLeave (uid, serde_json::from_str(&rr).ok()?),
			"USER_CHANGE_NICK" => ClientOp::MsgUserChNick(uid, serde_json::from_str(&rr).ok()?),
			//"" => ClientOp::Msg(uid, serde_json::from_str::<C2S>(&rr).ok()?),
			_ => { println!("received {}, which is unimplemented...", tp); return None }
		}).ok()?;
	}
	Some(())
}
