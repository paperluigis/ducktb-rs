// protocol handler for json_v2

use crate::types::{S2CResponse, RequestID, UserID, C2S, S2C, ResponseCodes, UserCustomData};

use super::{ListenerRxOp, ListenerTxOp};

use futures_util::{StreamExt, SinkExt};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::select;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

const PING_PAYLOAD: [u8; 12] = [114,101,97,100,32,105,102,32,99,117,116,101];

macro_rules! s2c_encode {
	($data:ident, $($rusttype:ident : $ptype:ident),*) => {
		match $data {
			$(
				S2C::$rusttype(s) => format!(concat!(stringify!($ptype), "\0{}"), serde_json::to_string(&s).unwrap()),
			)*
		}
	};
}

macro_rules! c2s_decode {
	($msgtype:ident, $rid:ident, $data:ident, $($ptype:ident : $rusttype:ident),*) => {
		Ok(match $msgtype {
			$(
			stringify!($ptype) => C2S::$rusttype(match serde_json::from_str($data) {
				Ok(s) => s,
				Err(e) => { return Err(S2CResponse($rid, ResponseCodes::ParseError, format!("failed to parse message: {}", e), None)) }
			}),
			)*
			_ => { return Err(S2CResponse($rid, ResponseCodes::ParseError, format!("message type {} is not supported", $msgtype), None)) }
		})
	}
}

fn s2c_encode(data: S2C) -> String {
	s2c_encode!(data,
		Hello: HELLO,
		Mouse: MOUSE,
		Room: ROOM,
		UserUpdate: USER_UPDATE,
		UserJoined: USER_JOINED,
		UserChNick: USER_CHANGE_NICK,
		UserLeft: USER_LEFT,
		Typing: TYPING,
		Message: MESSAGE,
		MessageDM: MESSAGE_DM,
		History: HISTORY,
		RateLimits: RATE_LIMITS,
		CustomR: CUSTOM_R,
		CustomU: CUSTOM_U,
		Response: RESPONSE
	)
}
fn c2s_decode(msgtype: &str, rid: Option<RequestID>, data: &str) -> Result<C2S, S2CResponse> {
	c2s_decode!(msgtype, rid, data,
		USER_JOINED: UserJoined,
		MOUSE: Mouse,
		TYPING: Typing,
		MESSAGE: Message,
		MESSAGE_DM: MessageDM,
		ROOM_JOIN: RoomJoin,
		ROOM_LEAVE: RoomLeave,
		USER_CHANGE_NICK: UserChNick,
		CUSTOM_R: CustomR,
		CUSTOM_U: CustomU
	)
}

pub async fn handle<S>(stream: &mut WebSocketStream<S>, sid: UserID,
	tx: Sender<ListenerTxOp>, mut rx: Receiver<ListenerRxOp>) -> String
	where S: AsyncRead + AsyncWrite + Unpin {

	let mut unfinished_msg_what: Option<(C2S, Option<RequestID>)> = None;
	loop {
		select! {
			// S2C
			msg = rx.recv() => {
				if let None = msg { return "".into() }
				let msg = msg.unwrap();
				match msg {
					// these messages aren't supposed to be sent here
					ListenerRxOp::ReloadCfg(_) => unreachable!(),

					ListenerRxOp::Event(_sid, msg) => {
						if let S2C::Response(ref rsp) = msg {
							if rsp.0.is_none() {
								if rsp.1 != ResponseCodes::Success {
									return format!("error ({}): {}", rsp.1 as u8, rsp.2);
								}
							} else {
								// how do i not duplicate this?
								if let Err(e) = stream.send(Message::Text(s2c_encode(msg))).await {
									return format!("websocket error: {}", e);
								}
							}
						} else if let Err(e) = stream.send(Message::Text(s2c_encode(msg))).await {
							return format!("websocket error: {}", e);
						}
					}
					ListenerRxOp::Shutdown(reason) => {
						//if let Err(e) = stream.send(Message::Text(s2c_encode(S2C::Response(S2CResponse(None, 255, reason.clone(), None))))).await {
						//	return format!("websocket error: {}", e);
						//}
						return reason;
					}
					ListenerRxOp::Keepalive => {
						if let Err(e) = stream.send(Message::Ping(PING_PAYLOAD.to_vec())).await {
							return format!("websocket error: {}", e);
						}
					}
				}
			} 
			// C2S
			msg = stream.next() => {
				if let None = msg { return "".into() }
				let msg = msg.unwrap();
				if let Err(_) = msg { return "".into() }
				let msg = msg.unwrap();
				match msg {
					Message::Close(_) => return "".into(),
					Message::Binary(data) => {
						if let Some((msg, rqid)) = unfinished_msg_what {
							unfinished_msg_what = None;
							if let Some(resp) = finish_msg_what(msg, data, sid, rqid, &tx).await {
								if let None = resp.0 { return resp.2 }
								if let Err(e) = stream.send(Message::Text(s2c_encode(S2C::Response(resp)))).await {
									return format!("websocket error: {}", e);
								}
							}
						} else {
							return "unexpected binary data".into()
						}
					}
					// we don't care about pings
					Message::Ping(_) => (),
					Message::Pong(_) => (),
					Message::Frame(_) => unreachable!("the documentation says i'm not going to get this value"),
					Message::Text(msg) => {
						if unfinished_msg_what.is_some() {
							return "please provide the binary data for the message you sent :P".into()
						}
						//info!("ws rx msg {} {:?}", sid, msg);
						match do_message(msg, sid, &tx).await {
							StateMachineThingWhat::Finish(Some(resp)) => {
								if let None = resp.0 { return resp.2 }
								if let Err(e) = stream.send(Message::Text(s2c_encode(S2C::Response(resp)))).await {
									return format!("websocket error: {}", e);
								}
							}
							StateMachineThingWhat::Finish(None) => {}
							StateMachineThingWhat::WaitingForBinaryData(msg, rqid) => {
								unfinished_msg_what = Some((msg, rqid));
							}
						}
					}
				}
			}
		}
	}
}

enum StateMachineThingWhat {
	Finish(Option<S2CResponse>),
	WaitingForBinaryData(C2S, Option<RequestID>),
}

async fn do_message(msg: String, sid: UserID, tx: &Sender<ListenerTxOp>) -> StateMachineThingWhat {
	let mut lsp = msg.splitn(3, "\0");
	let msgtype = match lsp.next() { Some(s) => s, None =>
		return StateMachineThingWhat::Finish(Some(S2CResponse(None, ResponseCodes::ParseError, "bad message format".into(), None))) };
	let mut data = match lsp.next() { Some(s) => s, None =>
		return StateMachineThingWhat::Finish(Some(S2CResponse(None, ResponseCodes::ParseError, "bad message format".into(), None))) };
	let mut rid = None;
	if let Some(str) = lsp.next() {
		rid = Some(data);
		data = str;
	};

	let rid: Option<RequestID> = match rid {
		Some(str) => match str.parse() {
			Ok(s) => Some(RequestID::new(s)),
			Err(e) => return StateMachineThingWhat::Finish(Some(S2CResponse(None, ResponseCodes::ParseError, format!("bad request id: {}", e), None)))
		}
		None => None
	};

	match c2s_decode(msgtype, rid, data) {
		Ok(data) => {
			if let C2S::CustomR(ref s) = data {
				if let UserCustomData::Placeholder = s.2 {
					return StateMachineThingWhat::WaitingForBinaryData(data, rid)
				}
			}
			if let C2S::CustomU(ref s) = data {
				if let UserCustomData::Placeholder = s.3 {
					return StateMachineThingWhat::WaitingForBinaryData(data, rid)
				}
			}
			if let Err(_) = tx.send(ListenerTxOp::Request(sid, rid, data)).await {
				return StateMachineThingWhat::Finish(Some(S2CResponse(None, ResponseCodes::ServerError, "server is out of capacity??".into(), None)))
			}
		}
		Err(r) => return StateMachineThingWhat::Finish(Some(r))
	};

	StateMachineThingWhat::Finish(None)
	// = str.find("\0").ok_or_else(|| { (CloseCode::Invalid, "expected message type, null byte, message data".into()) })?;
}

async fn finish_msg_what(mut msg: C2S, data: Vec<u8>, sid: UserID, rid: Option<RequestID>, tx: &Sender<ListenerTxOp>) -> Option<S2CResponse> {
	match msg {
		C2S::CustomR(ref mut s) => { s.2 = UserCustomData::Data(data) }
		C2S::CustomU(ref mut s) => { s.3 = UserCustomData::Data(data) }
		_ => unreachable!()
	}

	if let Err(_) = tx.send(ListenerTxOp::Request(sid, rid, msg)).await {
		Some(S2CResponse(None, ResponseCodes::ServerError, "server is out of capacity??".into(), None))
	} else { None }
}
