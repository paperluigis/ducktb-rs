// protocol handler for msgpack_v1

use crate::types::{S2CResponse, RequestID, UserID, C2S, S2C, ResponseCodes, UserCustomData};

use super::{ListenerRxOp, ListenerTxOp};

// why do i need this anyway
use arrayref::array_ref;
use futures_util::{StreamExt, SinkExt};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::select;
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::Message;

// arbitrary ping payload
// possibly has a meaningful value idk
const PING_PAYLOAD: [u8; 9] = [182,182,167,179,250,226,130,27,108];

macro_rules! s2c_encode {
	($data:ident, $($ptype:literal : $rusttype:ident),*) => { {
		let mut vec = Vec::new();
		match $data {
			$(
				S2C::$rusttype(s) => {
					vec.push($ptype);
					let mut ser = rmp_serde::Serializer::new(&mut vec).with_struct_map();
					s.serialize(&mut ser).expect("serialization shouldn't fail");
				},
			)*
		};
		vec
	} };
}

macro_rules! c2s_decode {
	($msgtype:ident, $rqid:ident, $data:ident, $($ptype:literal : $rusttype:ident),*) => { {
		Ok(match $msgtype {
			$(
				$ptype => C2S::$rusttype(match rmp_serde::from_slice($data) {
					Ok(s) => s,
					Err(e) => { return Err(S2CResponse($rqid, ResponseCodes::ParseError, format!("failed to parse message: {}", e), None)) }
				}),
			)*
			_ => { return Err(S2CResponse($rqid, ResponseCodes::ParseError, format!("message type {} is not supported", $msgtype), None)) }
		})
	} }
}

fn s2c_encode(data: S2C) -> Vec<u8> {
	s2c_encode!(data,
		0xff: Hello,
		0x10: Mouse,
		0x11: Room,
		0x12: UserUpdate,
		0x13: UserJoined,
		0x14: UserChNick,
		0x15: UserLeft,
		0x16: Typing,
		0x17: Message,
		0x18: MessageDM,
		0x19: History,
		0x20: RateLimits,
		0x21: CustomR,
		0x22: CustomU,
		0xfe: Response
	)
}
fn c2s_decode(mut data: &[u8]) -> Result<(Option<RequestID>, C2S), S2CResponse> {
	let data_len = data.len();
	if data_len < 2 {
		return Err(S2CResponse(None, ResponseCodes::ParseError, format!("expected at least 2 bytes (message type, messagepack data)"), None))
	}
	// [00 YY YY YY YY]XX __ __ __ ...
	// YYYYYYYY = request id
	// XX = msgtype
	// __ = messagepack data

	let mut msgtype = data[0];
	let mut rqid = None;
	if msgtype == 0 {
		if data_len < 7 {
			return Err(S2CResponse(None, ResponseCodes::ParseError, format!("expected at least 7 bytes (null byte, request id, message type, messagepack data)"), None))
		}
		rqid = Some(RequestID::new(u32::from_le_bytes(*array_ref![data,1,4])));
		msgtype = data[5];
		data = &data[6..];
	} else {
		data = &data[1..];
	}
	let msg = c2s_decode!(msgtype, rqid, data,
		0x13: UserJoined,
		0x10: Mouse,
		0x16: Typing,
		0x17: Message,
		0x18: MessageDM,
		0x11: RoomJoin,
		0x12: RoomLeave,
		0x14: UserChNick,
		0x21: CustomR,
		0x22: CustomU
	)?;
	Ok((rqid, msg))
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
								if let Err(e) = stream.send(Message::Binary(s2c_encode(msg))).await {
									return format!("websocket error: {}", e);
								}
							}
						} else if let Err(e) = stream.send(Message::Binary(s2c_encode(msg))).await {
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
						if let Some(resp) = do_message(data, sid, &tx).await {
							if let None = resp.0 { return resp.2 }
							if let Err(e) = stream.send(Message::Binary(s2c_encode(S2C::Response(resp)))).await {
								return format!("websocket error: {}", e);
							}
						}
					}
					// we don't care about pings
					Message::Ping(_) => (),
					Message::Pong(_) => (),
					Message::Frame(_) => unreachable!("the documentation says i'm not going to get this value"),
					Message::Text(msg) => {
						return "unexpected text data".into()
					}
				}
			}
		}
	}
}

async fn do_message(data: Vec<u8>, sid: UserID, tx: &Sender<ListenerTxOp>) -> Option<S2CResponse> {
	let (rqid, msg) = match c2s_decode(&data) {
		Ok(a) => a,
		Err(e) => return Some(e)
	};

	if let C2S::CustomR(ref s) = msg {
		if let UserCustomData::Placeholder = s.2 {
			return Some(S2CResponse(None, ResponseCodes::ParseError, format!("custom data must not be null"), None))
		}
	}
	if let C2S::CustomU(ref s) = msg {
		if let UserCustomData::Placeholder = s.3 {
			return Some(S2CResponse(None, ResponseCodes::ParseError, format!("custom data must not be null"), None))
		}
	}
	if let Err(_) = tx.send(ListenerTxOp::Request(sid, rqid, msg)).await {
		return Some(S2CResponse(None, ResponseCodes::ServerError, "server is out of capacity??".into(), None))
	}

	None
}

/*

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
			ServerOp::UsageError(r) => { return (false, CloseCode::Invalid, r) }, 
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

*/
