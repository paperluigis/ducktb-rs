use serde::{Serialize, Deserialize};
use crate::types::*;
use futures_util::{SinkExt, StreamExt};
use std::collections::VecDeque;
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
						ServerOp::MsgMouse(_) =>      format!("MOUSE\0{}",            serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgRoom(_) =>       format!("ROOM\0{}",             serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgUserUpdate(_) => format!("USER_UPDATE\0{}",      serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgUserJoined(_) => format!("USER_JOINED\0{}",      serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgUserChNick(_) => format!("USER_CHANGE_NICK\0{}", serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgUserLeft(_) =>   format!("USER_LEFT\0{}",        serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgTyping(_) =>     format!("TYPING\0{}",           serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgMessage(_) =>    format!("MESSAGE\0{}",          serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgHistory(_) =>    format!("HISTORY\0{}",          serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgRateLimits(s) => format!("RATE_LIMITS\0{}",      serde_json::to_string(&s).unwrap()),
					})).await.is_err() { return }
				} else { return }
			}
		}
	}
}

fn printduck<T: std::fmt::Debug>(e: T) -> T {
	println!("{:?}", e); e
}

async fn message(str: String, uid: UserID, t: &Sender<ClientOp>, first: bool) -> Option<()> {
	let f = str.find("\0")?;
	let (tp, rr) = str.split_at(f + 1);
	let (tp, _) = tp.split_at(tp.len() - 1);
	if first {
		if tp != "USER_JOINED" { return None }
		t.send(ClientOp::MsgUserJoined(uid, serde_json::from_str(&rr).map_err(printduck).ok()?)).await.ok()?;
	} else {
		// we want to die if we ever hit backpressure
		t.try_send(match tp {
			"MOUSE"            => duck_up(uid, V1C2SMessages::Mouse     (serde_json::from_str(&rr).map_err(printduck).ok()?)),
			"TYPING"           => duck_up(uid, V1C2SMessages::Typing    (serde_json::from_str(&rr).map_err(printduck).ok()?)),
			"MESSAGE"          => duck_up(uid, V1C2SMessages::Message   (serde_json::from_str(&rr).map_err(printduck).ok()?)),
			"ROOM"             => duck_up(uid, V1C2SMessages::Room      (serde_json::from_str(&rr).map_err(printduck).ok()?)),
			"USER_CHANGE_NICK" => ClientOp::MsgUserChNick(uid, serde_json::from_str(&rr).ok()?),
			//"" => V1C2SMessages::Msg(uid, serde_json::from_str::<C2S>(&rr).ok()?),
			_ => { println!("received {}, which is unimplemented...", tp); return None }
		}).ok()?;
	}
	Some(())
}

#[derive(Deserialize)]
struct V1C2SMouse(f32, f32);
#[derive(Deserialize)]
struct V1C2STyping(bool, #[serde(skip)] ());
#[derive(Deserialize)]
struct V1C2SMessage(String, #[serde(skip)] ());
#[derive(Deserialize)]
struct V1C2SRoom(RoomID, #[serde(skip)] ());
enum V1C2SMessages {
	Mouse(V1C2SMouse),
	Typing(V1C2STyping),
	Message(V1C2SMessage),
	Room(V1C2SRoom)
}

#[derive(Debug, Clone, Serialize)]
struct V1S2CHistory   (VecDeque<HistEntry>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct V1S2CRoom      (RoomID, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct V1S2CUserJoined(User, u64);
#[derive(Debug, Clone, Serialize)]
struct V1S2CUserLeft  (UserID, u64);
#[derive(Debug, Clone, Serialize)]
struct V1S2CUserChNick(UserID, (UserNick, UserColor), (UserNick, UserColor), u64);
#[derive(Debug, Clone, Serialize)]
struct V1S2CMouse     (UserID, f32, f32);
#[derive(Debug, Clone, Serialize)]
struct V1S2CUserUpdate(Vec<User>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct V1S2CTyping    (Vec<UserID>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct V1S2CMessage   (TextMessage, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
enum V1S2CMessages {
	Mouse(V1S2CMouse),
	Room(V1S2CRoom),
	UserUpdate(V1S2CUserUpdate),
	UserJoined(V1S2CUserJoined),
	UserChNick(V1S2CUserChNick),
	UserLeft(V1S2CUserLeft),
	Typing(V1S2CTyping),
	Message(V1S2CMessage),
	History(V1S2CHistory),
}

fn up_duck(a: ServerOp) -> V1S2CMessages {
	match a {
		ServerOp::MsgMouse(s) => V1S2CMessages::Mouse(V1S2CMouse(s.1,s.2,s.3)),
		ServerOp::MsgRoom(mut s) => V1S2CMessages::Room(V1S2CRoom(s.0.swap_remove(0),())),
		ServerOp::MsgUserUpdate(s) => V1S2CMessages::UserUpdate(V1S2CUserUpdate(s.1,())),
		ServerOp::MsgUserJoined(s) => V1S2CMessages::UserJoined(V1S2CUserJoined(s.1,s.2)),
		ServerOp::MsgUserChNick(s) => V1S2CMessages::UserChNick(V1S2CUserChNick(s.1,s.2,s.3,s.4)),
		ServerOp::MsgUserLeft(s) => V1S2CMessages::UserLeft(V1S2CUserLeft(s.1,s.2)),
		ServerOp::MsgTyping(s) => V1S2CMessages::Typing(V1S2CTyping(s.1,())),
		ServerOp::MsgMessage(s) => V1S2CMessages::Message(V1S2CMessage(s.1,())),
		ServerOp::MsgHistory(s) => V1S2CMessages::History(V1S2CHistory(s.1,())),
		_ => panic!("not convertable")
	}
}

fn duck_up(uid: UserID, a: V1C2SMessages) -> ClientOp {
	let rh = RoomHandle::new(0);
	match a {
		V1C2SMessages::Mouse(x)   => ClientOp::MsgMouse   (uid, C2SMouse   (rh, x.0, x.1)),
		V1C2SMessages::Typing(x)  => ClientOp::MsgTyping  (uid, C2STyping  (rh, x.0)),
		V1C2SMessages::Message(x) => ClientOp::MsgMessage (uid, C2SMessage (rh, x.0)),
		V1C2SMessages::Room(x)    => ClientOp::MsgRoomJoin(uid, C2SRoomJoin(x.0, true)),
	}
}

