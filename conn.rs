// single-room JSON protocol
mod json_v1;
// multiple-room JSON protocol
mod json_v2;
// same as json-v2 but with a different serialization protocol
mod msgpack_v1;

use crate::config::*;
use crate::types::*;
use rand::prelude::*;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::{spawn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{Sender, channel};
use tokio_tungstenite::{
	accept_hdr_async,
	tungstenite::{http::{StatusCode, HeaderValue},  handshake::server::{Request, Response, ErrorResponse}}
};




pub async fn listen(l: TcpListener, t: Sender<ClientOp>, r: Arc<Mutex<HashMap<String, ConnState>>>) {
	let mut conn_seq = 0x48aeb931u32;
	while let Ok((flow, _)) = l.accept().await {
		spawn(conn(flow, UserID::new(conn_seq), t.clone(), r.clone()));
		conn_seq += 1984;
	}
}

pub const PROTOCOLS: [&'static str; 2] = ["json-v1", "json-v2"];



async fn conn(y: TcpStream, mut ee: UserID, t: Sender<ClientOp>, r: Arc<Mutex<HashMap<String, ConnState>>>) {
	let addr = y.peer_addr().expect("what da hell man");
	let mut uip = addr.ip();
	let mut proto: String = "".into();
	let mut resume_id: String = "".into();

	let headcb = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
		let mut invalid_protocol = false;
		let mut e = ErrorResponse::new(None);

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
			if proto == "" && k == "sec-websocket-protocol" {
				let str = v.to_str().ok();
				if let Some(str) = str {
					for q in str.split(',').map(|e| e.trim()) {
						if PROTOCOLS.iter().any(|&i| i==q) {
							proto = q.into();
							break
						} else {
							invalid_protocol = true;
						}
					}
				} else { return Err(e) }
			}
		}
		if proto == "" && invalid_protocol { *e.status_mut() = StatusCode::BAD_REQUEST; return Err(e) }
		if proto != "" {
			resp.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_str(&proto).unwrap());
		}
		resume_id = req.uri().query().unwrap_or("").to_string();
		Ok(resp)
	};
	let bs = accept_hdr_async(y, headcb).await;
	if bs.is_err() { return }
	let bs = bs.unwrap();

	let qm = r.lock().expect("please").remove(&resume_id);

	let mut messages;
	let mut resumed = false;
	if let Some(s) = qm {
		resumed = true;
		messages = s.rx;
		ee = s.user_id;
		if t.send(ClientOp::Resume(ee)).await.is_err() { return };
	} else {
		resume_id = format!("{:0>16x}", random::<u64>());
		let (tx, rx) = channel(48);
		messages = rx;
		let balls = Susser::new(ee, uip, tx);
		if t.send(ClientOp::Connection(ee, balls, resume_id.clone())).await.is_err() { return };
	}
	match &*proto {
		"" | "json-v1" => json_v1::handle(bs, &mut messages, t.clone(), ee, !resumed).await,
		"json-v2" => json_v2::handle(bs, &mut messages, t.clone(), ee, !resumed).await,
		_ => panic!("not happening")
	}
	let mu: ConnState = ConnState {
		user_id: ee,
		disconnect_timer: SESSION_TIMEOUT,
		rx: messages,
	};
	r.lock().expect("please").insert(resume_id, mu);
}
