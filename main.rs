extern crate nix;
extern crate random_string;
extern crate futures_channel;
extern crate futures_util;
extern crate json;
extern crate tokio;

use futures_channel::mpsc::{unbounded, UnboundedSender};
use futures_util::{SinkExt, StreamExt};
use json::{array, object};
use nix::sys::socket::{setsockopt, sockopt};
use tokio::time::{interval, Duration};
use tokio::net::{TcpListener, TcpStream};
use tokio::{spawn, select};
use tokio_tungstenite::{
    accept_async,
    tungstenite::Message,
};
use random_string::generate;
use std::collections::{VecDeque, HashMap};
use std::env;
use std::net::IpAddr;
use std::os::unix::io::AsRawFd;
use std::sync::{Arc, Mutex};
use std::mem::drop;
use std::time::{SystemTime, UNIX_EPOCH};

type SusMap = Arc<Mutex<HashMap<u32, Susser>>>;
type SusRoom = Arc<Mutex<HashMap<u32, Room>>>;

struct HistMsg {
	home: String,
	sid: u32,
	content: String,
	nick: String,
	color: u32
}
struct HistJoin {
	home: String,
	sid: u32,
	nick: String,
	color: u32
}
struct HistLeave {
	home: String,
	sid: u32,
	nick: String,
	color: u32
}
struct HistChNick {
	hole: String,
	sid: u32,
	old_nick: String,
	old_color: u32,
	new_nick: String,
	new_color: u32
}
enum HistEntry {
	Message(HistMsg),
	Join(HistJoin),
	Leave(HistLeave),
	ChNick(HistChNick)
}

struct Room {
	hist: VecDeque<HistEntry>
}

const MAX_MOUSE: u8 = 100;
const MAX_CHNICK: u8 = 1;
const MAX_MESSAGE: u8 = 5;
const MAX_TYPING: u8 = 8;

struct SusRate {
	// all of those reset to 0 every 5 seconds
	mouse: u8,
	chnick: u8,
	message: u8,
	typing: u8
}

struct Susser {
	counter: SusRate,
	is_typing: bool,
	in_room: String,
	nick: String,
	// 0x00rrggbb
	color: u32,
	haship: u64,
	tx: UnboundedSender<Message>
}

const CS_HEX: &str = "0123456789abcdef";


#[tokio::main]
async fn main() {
	let wtf = env::var("BIND").unwrap_or("127.0.0.1:8000".into());
	let abbi: String = generate(16, CS_HEX);
	let que = TcpListener::bind(&wtf).await.expect("DANG IT");
	let rf = que.as_raw_fd();
	setsockopt(rf, sockopt::ReuseAddr, &true).ok();
	println!("pls work :skull: (listening on {}), server string '{}'", wtf, abbi);
	let mut conn_seq: u32 = 0x48aeb931;
	let ducks = SusMap::new(Mutex::new(HashMap::new()));
	let rooms = SusRoom::new(Mutex::new(HashMap::new()));
	while let Ok((flow, _)) = que.accept().await {
		spawn(conn(flow, conn_seq, ducks.clone(), abbi.clone()));
		conn_seq += 1984;
	}
}

async fn conn(s: TcpStream, seq: u32, ducks: SusMap, srv: String) {
	let addr = s.peer_addr().unwrap();
	println!("\x1b[33mconn+ \x1b[34m[{:0>8x}|{:?}]\x1b[0m", seq, addr);
	let bs = accept_async(s).await;
	if !bs.is_ok() {
		println!("...nvm");
		return;
	}
	let (tx, mut rx) = unbounded();

	let (mut tws, mut rws) = bs.unwrap().split();

	let sus = Susser {
		counter: SusRate { message: 0, chnick: 0, mouse: 0, typing: 0 },
		is_typing: false,
		color: 0,
		nick: "".to_string(),
		in_room: "".to_string(),
		haship: haship(addr.ip()),
		tx: tx,
	};
	let mut msg_1st = true;
	// println!("ducks mutex lock on line {}", std::line!());
	ducks.lock().unwrap().insert(seq, sus);

	// DO I LOOK LIKE I CARE-
	let _ = tws.send(format!("HELLO\0[\"{}\",\"{:0>8x}\"]", srv, seq).into()).await;

	let mut intv = interval(Duration::from_millis(100));
	let mut counter_clear = 0;
	loop {
		select!{
			msg = rx.next() => {
				let msg = msg.unwrap();
				match tws.send(msg).await {
					Err(_) => break,
					Ok(()) => ()
				}
			}
			msg = rws.next() => {
				if msg.is_none() { break }
				let msg = msg.unwrap();
				match msg {
					Ok(a) => match a {
						Message::Text(str) => {
							if !message(str, seq, &ducks, get_ts(), msg_1st).await { break };
							msg_1st = false;
						}
						_ => break
					}
					Err(_) => break
				}
			}
			_ = intv.tick() => {
				counter_clear += 1;
				if counter_clear > 50 {
					counter_clear = 0;
					let mut l = ducks.lock().unwrap();
					let c = &mut l.get_mut(&seq).unwrap().counter;
					c.message = 0;
					c.chnick = 0;
					c.mouse = 0;
					c.typing = 0;
				}
				// let _ = tws.send("TICK\0[]".into()).await;
			}
		};
	}
	// we do NOT give a fuck if close() fails
	// (if it does the underlying connection is probably severed anyway)
	let _ = tws.close().await;
    println!("\x1b[31mconn- \x1b[34m[{:0>8x}|{:?}]\x1b[0m", seq, addr);

	leave_room(seq, &ducks);
	// println!("ducks mutex lock on line {}", std::line!());
	let mut dorks = ducks.lock().unwrap();
	//let duck = dorks.get_mut(&seq).unwrap();
	dorks.remove(&seq);
	drop(dorks);
}

fn leave_room(id: u32, ducks: &SusMap) {
	// println!("ducks mutex lock on line {}", std::line!());
	let mut lurks = ducks.lock().unwrap();
	let mut x = lurks.get_mut(&id).unwrap();
	let room = x.in_room.clone();
	if room == "" { return }
	x.in_room = "".to_string();
	drop(lurks);
	let ts = get_ts();
	send_broad(&room, &ducks, "USER_LEFT".into(), array![ idtostr(id), ts ]);
	send_broad(&room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &room) ]);
}

fn join_room(room: &str, id: u32, ducks: &SusMap) {
	// println!("ducks mutex lock on line {}", std::line!());
	let mut lurks = ducks.lock().unwrap();
	let x = lurks.get_mut(&id).unwrap();
	if room == x.in_room { return }
	drop(lurks);
	leave_room(id, ducks);
	// println!("ducks mutex lock on line {}", std::line!());
	let mut lurks = ducks.lock().unwrap();
	let x = lurks.get_mut(&id).unwrap();
	let nick = x.nick.to_string();
	let col = u32tocol(x.color);
	x.in_room = room.to_string();
	drop(lurks);
	let ts = get_ts();
	send_uni(id, &ducks, "ROOM".into(), array![room]);
	// TODO: implement history
	send_uni(id, &ducks, "HISTORY".into(), array![[]]);

	send_broad(room.into(), &ducks, "USER_JOINED".into(), array![{ sid: idtostr(id), nick: nick.clone(), color: col.clone(), time: ts }]);
	send_broad(room.into(), &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, room) ]);
}

fn send_broad(to_room: &str, ducks: &SusMap, t: String, val: json::JsonValue) {
	if t != "MOUSE" {
		if to_room == "" {
			println!("\x1b[33mtx    \x1b[34m[broadcast]\x1b[0m {:?} {}", t, val.dump());
		} else {
			println!("\x1b[33mtx    \x1b[34m[broadcast|\x1b[37m{}\x1b[34m]\x1b[0m {:?} {}", to_room, t, val.dump());
		}
	}
	// println!("ducks mutex lock on line {}", std::line!());
	let ducks = ducks.lock().unwrap();
	for (_, sus) in ducks.iter() {
		if to_room != "" && sus.in_room != to_room { continue }
		let _ = sus.tx.unbounded_send(Message::Text(format!("{}\0{}", t, val.dump())));
	}
}

fn send_uni(to_id: u32, ducks: &SusMap, t: String, val: json::JsonValue) {
	println!("\x1b[34mtx    \x1b[34m[{:0>8x}]\x1b[0m {:?} {}", to_id, t, val.dump());
	// println!("ducks mutex lock on line {}", std::line!());
	let ducks = ducks.lock().unwrap();
	// I DON'T FUCKING CARE IF SEND FAILS
	let _ = ducks.get(&to_id).unwrap().tx.unbounded_send(Message::Text(format!("{}\0{}", t, val.dump())));
}

async fn message(str: String, id: u32, ducks: &SusMap, ts: u64, first: bool) -> bool {
	let f = str.find("\0");
	if f.is_none() {
		return false;
	}

	let (tp, rr) = str.split_at(f.unwrap() + 1);
	let jd = json::parse(rr);
	if !jd.is_ok() { return false }
	let jv = jd.unwrap();
	if !jv.is_array() { return false }
	if tp != "MOUSE\0" {
		println!("\x1b[32mrx    \x1b[34m[{:0>8x}]\x1b[0m {:?} {}", id, tp, jv.dump());
	}
	if first {
		if tp != "USER_JOINED\0" { return false }
		// println!("ducks mutex lock on line {}", std::line!());
		let mut unducks = ducks.lock().unwrap();
		let sus = unducks.get_mut(&id).unwrap();

		if sus.in_room != "" { return false }
		let nick = &jv[0];
		let color = &jv[1];
		let room = &jv[2];
		let jl = jv.len();
		if	!(jl == 2 || jl == 3) ||
			!nick.is_string() ||
			!color.is_string() ||
			!(jl == 2 || room.is_string()) { return false }
		let color = coltou32(color.as_str().unwrap());
		if	color.is_none() { return false }
		let nick = nick.as_str().unwrap().trim();
		let color = color.unwrap();
		let room = room.as_str().unwrap_or("lobby").trim();
		if	nick == "" ||
			room == "" { return false }
		sus.nick = nick.to_string();
		sus.color = color;
		// sus.in_room = room.to_string();
		drop(unducks);
		join_room(room, id, &ducks);
	} else {
		match tp {
			"MESSAGE\0" => {
				let room: String;
				{
					let mut lel = ducks.lock().unwrap();
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.message += 1;
					if lel.counter.message > MAX_MESSAGE { return false }
					room = lel.in_room.clone();
				}
				let content = &jv[0];
				if	jv.len() != 1 ||
					!content.is_string() {
					return false;
				}
				send_broad(&room, ducks, "MESSAGE".into(), array![{
					sid: idtostr(id),
					time: ts,
					content: content.clone()
				}]);
			}
			"TYPING\0" => {
				let typing = &jv[0];
				if	jv.len() != 1 ||
					!typing.is_boolean() {
					return false;
				}
				let mut ch = false;
				let room: String;
				{
					let mut lel = ducks.lock().unwrap();
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.typing += 1;
					if lel.counter.typing > MAX_TYPING { return false }
					if let json::JsonValue::Boolean(c) = typing {
						if *c != lel.is_typing {
							lel.is_typing = *c;
							ch = true;
						}
					}
					room = lel.in_room.clone();
				}
				if ch {
					send_broad(&room, &ducks, "TYPING".into(), array![ ducktotyp(ducks, &room) ]);
				}
				//if typing == true {
				//	send_uni(id, ducks, "MESSAGE".into(), array![{sid:"system",time:ts,content:"YOU BITCH ASS MOTHERFU-"}]);
				//}
			}
			"MOUSE\0" => {
				{
					let mut lel = ducks.lock().unwrap();
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.mouse += 1;
					if lel.counter.mouse > MAX_MOUSE { return false }
				}
				let x = &jv[0];
				let y = &jv[1];
				if	jv.len() != 2 ||
					!x.is_number() ||
					!y.is_number() {
					return false;
				}
				// println!("ducks mutex lock on line {}", std::line!());
				let bb = ducks.lock().unwrap();
				let room = bb.get(&id).unwrap().in_room.clone();
				drop(bb);
				send_broad(&room, ducks, "MOUSE".into(), array![idtostr(id), x.clone(), y.clone()]);
			}
			"USER_CHANGE_NICK\0" => {
				let nick = &jv[0];
				let color = &jv[1];
				let jl = jv.len();
				if	jl != 2 ||
					!nick.is_string() ||
					!color.is_string() { return false }
				let color = coltou32(color.as_str().unwrap());
				if	color.is_none() { return false }
				let nick = nick.as_str().unwrap().trim();
				let color = color.unwrap();
				let room: String;
				let pnick: String;
				let pcol: u32;
				{
					let mut lel = ducks.lock().unwrap();
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.chnick += 1;
					if lel.counter.chnick > MAX_CHNICK { return false }
					room = lel.in_room.clone();
					pnick = lel.nick.clone();
					pcol = lel.color;
					lel.nick = nick.to_string();
					lel.color = color;
				}
				send_broad(&room, ducks, "USER_CHANGE_NICK".into(), array![idtostr(id), [pnick.clone(),u32tocol(pcol)], [nick,u32tocol(color)], ts]);
				send_broad(&room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &room) ]);
			}
			"ROOM\0" => {
				let room = &jv[0];
				if	jv.len() != 1 ||
					!room.is_string() {
					return false;
				}
				let room = room.as_str().unwrap().trim();
				if room == "" {
					return false;
				}
				join_room(room, id, &ducks);
			}
			_ => {
				println!("skull emoji");
				return false;
			}
		}
	}
	return true;
}

fn ducktosl(ducks: &SusMap, room: &str) -> json::JsonValue {
	let mut arr = array![];
	// println!("ducks mutex lock on line {}", std::line!());
	let ducks = ducks.lock().unwrap();
	for (id, sus) in ducks.iter() {
		if room != "" && sus.in_room != room { continue }
		let o = object!{
			sid: idtostr(*id),
			nick: sus.nick.clone(),
			color: u32tocol(sus.color),
			home: hidtostr(sus.haship)
		};
		arr.push(o).ok().expect("wait wtf is");
	}
	return arr;
}

fn ducktotyp(ducks: &SusMap, room: &str) -> json::JsonValue {
	let mut arr = array![];
	// println!("ducks mutex lock on line {}", std::line!());
	let ducks = ducks.lock().unwrap();
	for (id, sus) in ducks.iter() {
		if room != "" && sus.in_room != room { continue }
		if sus.is_typing {
			arr.push(idtostr(*id)).ok().expect("wait wtf is");
		}
	}
	return arr;
}

fn coltou32(inp: &str) -> Option<u32> {
	if inp.len() == 7 && inp.starts_with("#") {
		return u32::from_str_radix(&inp[1..], 16).ok();
	} else {
		return None
	}
}

fn u32tocol(inp: u32) -> String {
	return format!("#{:0>6x}", inp);
}
fn idtostr(inp: u32) -> String {
	return format!("{:0>8x}", inp);
}
fn hidtostr(inp: u64) -> String {
	return format!("{:0>16x}", inp);
}

fn haship(inp: IpAddr) -> u64 {
	return 5115;
}

fn get_ts() -> u64 {
	let a = SystemTime::now();
	let b = a.duration_since(UNIX_EPOCH).expect("Time travel can't be supported");
	return b.as_millis().try_into().expect("ARE WE IN THE FUTURE??");
}
