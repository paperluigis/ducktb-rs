mod json_v1;
mod json_v2;

use crate::config::*;
use crate::types::*;
use tokio::{spawn};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc::{Sender, channel};
use tokio_tungstenite::{
	accept_hdr_async,
	tungstenite::{http::{StatusCode, HeaderValue},  handshake::server::{Request, Response, ErrorResponse}}
};


pub async fn listen(l: TcpListener, t: Sender<ClientOp>) {
	let mut conn_seq = 0x48aeb931u32;
	while let Ok((flow, _)) = l.accept().await {
		spawn(conn(flow, UserID::new(conn_seq), t.clone()));
		conn_seq += 1984;
	}
}

pub const PROTOCOLS: [&'static str; 2] = ["json-v1", "json-v2"];

async fn conn(y: TcpStream, ee: UserID, t: Sender<ClientOp>) {
	let addr = y.peer_addr().expect("what da hell man");
	let mut uip = addr.ip();
	let mut proto: String = "".into();
	let headcb = |req: &Request, mut resp: Response| -> Result<Response, ErrorResponse> {
		let mut invalid_protocol = false;
		let mut e = ErrorResponse::new(None);
		*e.status_mut() = StatusCode::BAD_REQUEST;

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
		if proto == "" {
			if invalid_protocol { return Err(e) }
			else { proto = "json-v1".into() }
		}

		resp.headers_mut().insert("sec-websocket-protocol", HeaderValue::from_str(&proto).unwrap());
		Ok(resp)
	};
	let bs = accept_hdr_async(y, headcb).await;
	if bs.is_err() { return }
	let bs = bs.unwrap();

	let (tx, messages) = channel(48);
	let balls = Susser::new(ee, uip, tx);
	if t.send(ClientOp::Connection(ee, balls)).await.is_err() { return };

	match &*proto {
		"json-v1" => json_v1::handle(bs, messages, t.clone(), ee).await,
		"json-v2" => json_v2::handle(bs, messages, t.clone(), ee).await,
		_ => panic!("not happening")
	}

	let _ = t.send(ClientOp::Disconnect(ee)).await;
}
