use serde::{Serialize, Deserialize};
use crate::types::*;
use futures_util::{SinkExt, StreamExt};
use std::collections::VecDeque;
use tokio::select;
use tokio::net::TcpStream;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message, tungstenite::protocol::frame::coding::CloseCode};

pub async fn handle(bs: &mut WebSocketStream<TcpStream>, messages: &mut Receiver<ServerOp>, t: Sender<ClientOp>, ee: UserID, mut msg_1st: bool) -> (bool, CloseCode, String) {
	loop {
		select!{
			msg = bs.next() => {
				match msg {
					Some(Ok(Message::Text(str))) => {
						if let Err(s) = message(str, ee, &t, msg_1st).await {
							return (false, s.0, s.1)
						}
					}
					Some(Ok(Message::Close(o))) => {
						return (o.is_some_and(|x| x.code == CloseCode::Away), CloseCode::Normal, String::default())
					}
					_ => return (false, CloseCode::Unsupported, "expected text message".into())
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
						ServerOp::MsgMessageDM(_) =>  format!("MESSAGE_DM\0{}",       serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgHistory(_) =>    format!("HISTORY\0{}",          serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgCustomR(_) =>    format!("CUSTOM_R\0{}",         serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgCustomU(_) =>    format!("CUSTOM_U\0{}",         serde_json::to_string(&up_duck(msg)).unwrap()),
						ServerOp::MsgRateLimits(s) => format!("RATE_LIMITS\0{}",      serde_json::to_string(&s).unwrap()),
					})).await.is_err() { return (true, CloseCode::Normal, String::default()) }
				} else { return (true, CloseCode::Normal, String::default()) }
			}
		}
	}
}

async fn message(str: String, uid: UserID, t: &Sender<ClientOp>, first: bool) -> Result<(), (CloseCode, String)> {
	let f = str.find("\0").ok_or_else(|| { (CloseCode::Invalid, "expected message type, null byte, message data".into()) })?;
	let (tp, rr) = str.split_at(f + 1);
	let (tp, _) = tp.split_at(tp.len() - 1);
	if first {
		if tp != "USER_JOINED" { return Err((CloseCode::Protocol, "expected USER_JOINED message".into())) }
		//t.send(ClientOp::MsgUserJoined(uid, serde_json::from_str(&rr).map_err(printduck).ok()?)).await.ok()?;
		//t.send(duck_up(uid, V1C2SMessages::UserJoined(serde_json::from_str(&rr).map_err(|e| )))).await.ok()?;
		t.send(duck_up(uid, V1C2SMessages::UserJoined(serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?))).await.map_err(|_| { (CloseCode::Away, "server ducked up".into()) })?;
	} else {
		// we want to die if we ever hit backpressure
		t.try_send(match tp {
			"MOUSE"            => duck_up(uid, V1C2SMessages::Mouse     (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			"TYPING"           => duck_up(uid, V1C2SMessages::Typing    (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			"MESSAGE"          => duck_up(uid, V1C2SMessages::Message   (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			"MESSAGE_DM"       => duck_up(uid, V1C2SMessages::MessageDM (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			"ROOM"             => duck_up(uid, V1C2SMessages::Room      (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			"USER_CHANGE_NICK" => ClientOp::MsgUserChNick(uid, serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?),
			"CUSTOM_R"         => duck_up(uid, V1C2SMessages::CustomR   (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			"CUSTOM_U"         => duck_up(uid, V1C2SMessages::CustomU   (serde_json::from_str(&rr).map_err(|e| { (CloseCode::Invalid, format!("{}", e)) })?)),
			//"" => V1C2SMessages::Msg(uid, serde_json::from_str::<C2S>(&rr).ok1()?),
			_ => { return Err((CloseCode::Invalid, format!("message {} not supported", tp))) }
		}).map_err(|_| { (CloseCode::Away, "server too slow :P".into()) })?;
	}
	Ok(())
}

#[derive(Deserialize)]
struct V1C2SUserJoined(UserNick, UserColor, RoomID);
#[derive(Deserialize)]
struct V1C2SMouse(f32, f32);
#[derive(Deserialize)]
struct V1C2STyping(bool, #[serde(skip)] ());
#[derive(Deserialize)]
struct V1C2SMessage(String, #[serde(skip)] ());
#[derive(Deserialize)]
struct V1C2SMessageDM(String, UserID);
#[derive(Deserialize)]
struct V1C2SRoom(RoomID, #[serde(skip)] ());
#[derive(Deserialize)]
struct V1C2SCustomR(String, UserCustomData);
#[derive(Deserialize)]
struct V1C2SCustomU(UserID, String, UserCustomData);
enum V1C2SMessages {
	UserJoined(V1C2SUserJoined),
	Mouse(V1C2SMouse),
	Typing(V1C2STyping),
	Message(V1C2SMessage),
	MessageDM(V1C2SMessageDM),
	Room(V1C2SRoom),
	CustomR(V1C2SCustomR),
	CustomU(V1C2SCustomU),
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
struct V1S2CMessageDM (TextMessageDM, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct V1S2CCustomR   (UserID, String, UserCustomData);
#[derive(Debug, Clone, Serialize)]
struct V1S2CCustomU   (UserID, String, UserCustomData);
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
	MessageDM(V1S2CMessageDM),
	History(V1S2CHistory),
	CustomR(V1S2CCustomR),
	CustomU(V1S2CCustomU),
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
		ServerOp::MsgMessageDM(s) => V1S2CMessages::MessageDM(V1S2CMessageDM(s.1,())),
		ServerOp::MsgHistory(s) => V1S2CMessages::History(V1S2CHistory(s.1,())),
		ServerOp::MsgCustomR(s) => V1S2CMessages::CustomR(V1S2CCustomR(s.1,s.2,s.3)),
		ServerOp::MsgCustomU(s) => V1S2CMessages::CustomU(V1S2CCustomU(s.1,s.2,s.3)),
		_ => panic!("not convertable")
	}
}

fn duck_up(uid: UserID, a: V1C2SMessages) -> ClientOp {
	let rh = RoomHandle::new(0);
	match a {
		V1C2SMessages::UserJoined(x) => ClientOp::MsgUserJoined(uid, C2SUserJoined(x.0, x.1, vec![x.2])),
		V1C2SMessages::Mouse(x)      => ClientOp::MsgMouse     (uid, C2SMouse     (rh, x.0, x.1)),
		V1C2SMessages::Typing(x)     => ClientOp::MsgTyping    (uid, C2STyping    (rh, x.0)),
		V1C2SMessages::Message(x)    => ClientOp::MsgMessage   (uid, C2SMessage   (rh, x.0)),
		V1C2SMessages::MessageDM(x)  => ClientOp::MsgMessageDM (uid, C2SMessageDM (rh, x.0, x.1)),
		V1C2SMessages::Room(x)       => ClientOp::MsgRoomJoin  (uid, C2SRoomJoin  (x.0, true)),
		V1C2SMessages::CustomR(x)    => ClientOp::MsgCustomR  (uid, C2SCustomR   (rh, x.0, x.1)),
		V1C2SMessages::CustomU(x)    => ClientOp::MsgCustomU  (uid, C2SCustomU   (rh, x.0, x.1, x.2)),
	}
}

