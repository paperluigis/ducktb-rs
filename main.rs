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

type UserNick = String;
// format is 0rgb
type UserColor = u32;
type UserID = u32;
type RoomID = String;
type UserHashedIP = u64;

type SusMap = Arc<Mutex<HashMap<UserID, Susser>>>;
type SusRoom = Arc<Mutex<HashMap<RoomID, Room>>>;

#[derive(Debug)]
struct HistMsg {
	home: UserHashedIP,
	sid: UserID,
	content: String,
	nick: UserNick,
	color: UserColor,
	ts: u64
}
#[derive(Debug)]
struct HistJoin {
	home: UserHashedIP,
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	ts: u64
}
#[derive(Debug)]
struct HistLeave {
	home: UserHashedIP,
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	ts: u64
}
#[derive(Debug)]
struct HistChNick {
	home: UserHashedIP,
	sid: UserID,
	old_nick: UserNick,
	old_color: UserColor,
	new_nick: UserNick,
	new_color: UserColor,
	ts: u64
}

#[derive(Debug)]
enum HistEntry {
	Message(HistMsg),
	Join(HistJoin),
	Leave(HistLeave),
	ChNick(HistChNick)
}

struct Room {
	hist: VecDeque<HistEntry>,
	// refcounting
	conn_users: u16
}

const HIST_ENTRY_MAX: usize = 512;
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
	in_room: RoomID,
	nick: UserNick,
	color: UserColor,
	haship: UserHashedIP,
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
	let mut conn_seq: UserID = 0x48aeb931;
	let ducks = SusMap::new(Mutex::new(HashMap::new()));
	let rooms = SusRoom::new(Mutex::new(HashMap::new()));
	while let Ok((flow, _)) = que.accept().await {
		spawn(conn(flow, conn_seq, ducks.clone(), rooms.clone(), abbi.clone()));
		conn_seq += 1984;
	}
}

async fn conn(s: TcpStream, seq: UserID, ducks: SusMap, rooms: SusRoom, srv: String) {
	let addr = s.peer_addr().expect("what da hell man");
	println!("\x1b[33mconn+ \x1b[34m[{:0>8x}|{:?}]\x1b[0m", seq, addr);
	let bs = accept_async(s).await;
	if !bs.is_ok() {
		println!("...nevermind that");
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
							if !message(str, seq, &ducks, &rooms, get_ts(), msg_1st).await { break };
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

	leave_room(seq, &ducks, &rooms);
	let mut dorks = ducks.lock().unwrap();
	//let duck = dorks.get_mut(&seq).unwrap();
	dorks.remove(&seq);
	//drop(dorks);
}

fn leave_room(id: UserID, ducks: &SusMap, rooms: &SusRoom) {
	let mut lurks = ducks.lock().unwrap();
	let x = lurks.get_mut(&id).unwrap();
	let nick = x.nick.clone();
	let col = x.color;
	let hip = x.haship;
	let room = x.in_room.clone();
	if room == "" { return }
	x.in_room = "".to_string();
	drop(lurks);
	let ts = get_ts();
	send_userleave(room.clone(), &ducks, &rooms, &nick, col, id, hip, ts);

	let mut rooks = rooms.lock().unwrap();
	let hoho = rooks.get_mut(&room).unwrap();
	hoho.conn_users -= 1;
	if hoho.conn_users != 0 { return }
	rooks.remove(&room);
}

fn join_room(room: RoomID, id: UserID, ducks: &SusMap, rooms: &SusRoom) {
	let room = room.to_string();
	let mut lurks = ducks.lock().unwrap();
	let x = lurks.get_mut(&id).unwrap();
	if room == x.in_room { return }
	drop(lurks);
	leave_room(id, ducks, rooms);
	let mut lurks = ducks.lock().unwrap();
	let x = lurks.get_mut(&id).unwrap();

	let mut rooks = rooms.lock().unwrap();
	if !rooks.contains_key(&room) {
		let hoho = Room {
			conn_users: 0,
			hist: VecDeque::new()
		};
		rooks.insert(room.clone(), hoho);
	}
	let hoho = rooks.get_mut(&room).unwrap();
	hoho.conn_users += 1;
	let nick = x.nick.to_string();
	let col = x.color;
	let hip = x.haship;
	x.in_room = room.to_string();
	drop(lurks);
	let ts = get_ts();
	send_uni(id, &ducks, "ROOM".into(), array![room.clone()]);
	send_uni(id, &ducks, "HISTORY".into(), array![hist2json(&hoho.hist)]);

	drop(rooks);
	send_userjoin(room, &ducks, &rooms, &nick, col, id, hip, ts);
}

fn hist2json(v: &VecDeque<HistEntry>) -> json::JsonValue {
	let mut a = json::JsonValue::Array(Vec::with_capacity(v.len()));
	for entry in v.iter() {
		let _ = a.push(match entry {
			HistEntry::Leave(b) => object!{
				left: true,
				time: b.ts,
				home: hidtostr(b.home),
				sid: idtostr(b.sid),
				nick: b.nick.to_string(),
				color: u32tocol(b.color)
			},
			HistEntry::Join(b) => object!{
				joined: true,
				time: b.ts,
				home: hidtostr(b.home),
				sid: idtostr(b.sid),
				nick: b.nick.to_string(),
				color: u32tocol(b.color)
			},
			HistEntry::Message(b) => object!{
				time: b.ts,
				home: hidtostr(b.home),
				sid: idtostr(b.sid),
				nick: b.nick.to_string(),
				color: u32tocol(b.color),
				message: b.content.clone()
			},
			HistEntry::ChNick(b) => object! {
				time: b.ts,
				home: hidtostr(b.home),
				sid: idtostr(b.sid),
				nick: b.old_nick.to_string(),
				color: u32tocol(b.old_color),
				newnick: b.new_nick.to_string(),
				newcolor: u32tocol(b.new_color),
			}
		});
	}
	return a;
}

fn send_userleave(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	nick: &str, col: UserColor, uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::Leave(HistLeave {
		nick: nick.into(),
		home: ip,
		color: col,
		sid: uid,
		ts: ts
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, &ducks, "USER_LEFT".into(), array![ idtostr(uid), ts ]);
	send_broad(&to_room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &to_room) ]);
}

fn send_userjoin(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	nick: &str, col: UserColor, uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::Join(HistJoin {
		nick: nick.into(),
		home: ip,
		color: col,
		sid: uid,
		ts: ts
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, &ducks, "USER_JOINED".into(), array![{ sid: idtostr(uid), nick: nick.clone(), color: u32tocol(col), time: ts }]);
	send_broad(&to_room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &to_room) ]);
}

fn send_userchnick(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	old_nick: &str, old_col: UserColor, new_nick: &str, new_col: UserColor,
	uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::ChNick(HistChNick {
		old_nick: old_nick.into(),
		new_nick: new_nick.into(),
		home: ip,
		old_color: old_col,
		new_color: new_col,
		sid: uid,
		ts: ts
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, ducks, "USER_CHANGE_NICK".into(), array![idtostr(uid), [old_nick,u32tocol(old_col)],[new_nick,u32tocol(new_col)], ts]);
	send_broad(&to_room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &to_room) ]);
}

fn send_message(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	nick: &str, col: UserColor, content: String, uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::Message(HistMsg {
		nick: nick.into(),
		home: ip,
		color: col,
		sid: uid,
		ts: ts,
		content: content.clone()
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, ducks, "MESSAGE".into(), array![{
		sid: idtostr(uid),
		time: ts,
		content: content.clone()
	}]);
}

fn send_broad(to_room: &str, ducks: &SusMap, t: String, val: json::JsonValue) {
	if t != "MOUSE" {
		if to_room == "" {
			println!("\x1b[33mtx    \x1b[34m[broadcast]\x1b[0m {:?} {}", t, val.dump());
		} else {
			println!("\x1b[33mtx    \x1b[34m[broadcast|\x1b[37m{}\x1b[34m]\x1b[0m {:?} {}", to_room, t, val.dump());
		}
	}
	let ducks = ducks.lock().unwrap();
	for (_, sus) in ducks.iter() {
		if to_room != "" && sus.in_room != to_room { continue }
		let _ = sus.tx.unbounded_send(Message::Text(format!("{}\0{}", t, val.dump())));
	}
}

fn send_uni(to_id: UserID, ducks: &SusMap, t: String, val: json::JsonValue) {
	println!("\x1b[34mtx    \x1b[34m[{:0>8x}]\x1b[0m {:?} {}", to_id, t, val.dump());
	let ducks = ducks.lock().unwrap();
	// I DON'T FUCKING CARE IF SEND FAILS
	let _ = ducks.get(&to_id).unwrap().tx.unbounded_send(Message::Text(format!("{}\0{}", t, val.dump())));
}

async fn message(str: String, id: UserID, ducks: &SusMap, rooms: &SusRoom, ts: u64, first: bool) -> bool {
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
		join_room(room.into(), id, &ducks, &rooms);
	} else {
		match tp {
			"MESSAGE\0" => {
				let room: String;
				let nick: String;
				let col: UserColor;
				let hip: UserHashedIP;
				{
					let mut lel = ducks.lock().unwrap();
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.message += 1;
					if lel.counter.message > MAX_MESSAGE { return false }
					room = lel.in_room.clone();
					nick = lel.nick.clone();
					col = lel.color;
					hip = lel.haship;
				}
				let content = &jv[0];
				if	jv.len() != 1 ||
					!content.is_string() {
					return false;
				}
				let content = content.to_string();
				send_message(room, ducks, rooms, &nick, col, content, id, hip, ts);
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
				let pcol: UserColor;
				let hip: UserHashedIP;
				{
					let mut lel = ducks.lock().unwrap();
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.chnick += 1;
					if lel.counter.chnick > MAX_CHNICK { return false }
					hip = lel.haship;
					room = lel.in_room.clone();
					pnick = lel.nick.clone();
					pcol = lel.color;
					lel.nick = nick.to_string();
					lel.color = color;
				}
				send_userchnick(room, &ducks, &rooms, &pnick, pcol, &nick, color, id, hip, ts);
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
				join_room(room.into(), id, &ducks, &rooms);
			}
			_ => {
				return false;
			}
		}
	}
	return true;
}

fn ducktosl(ducks: &SusMap, room: &str) -> json::JsonValue {
	let mut arr = array![];
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
	let ducks = ducks.lock().unwrap();
	for (id, sus) in ducks.iter() {
		if room != "" && sus.in_room != room { continue }
		if sus.is_typing {
			arr.push(idtostr(*id)).ok().expect("wait wtf is");
		}
	}
	return arr;
}

fn coltou32(inp: &str) -> Option<UserColor> {
	if inp.len() == 7 && inp.starts_with("#") {
		return u32::from_str_radix(&inp[1..], 16).ok();
	} else {
		return None
	}
}

fn u32tocol(inp: UserColor) -> String {
	return format!("#{:0>6x}", inp);
}
fn idtostr(inp: UserID) -> String {
	return format!("{:0>8x}", inp);
}
fn hidtostr(inp: u64) -> String {
	return format!("{:0>16x}", inp);
}

fn haship(inp: IpAddr) -> u64 {
	//todo: make it irreversible
	if let IpAddr::V4(v4) = inp {
		let oc = v4.octets();
		return 0x19fa920130b0ba21u64 |
			(u64::from(oc[0]) * 10495007) |
			(u64::from(oc[1]) * 39950100);
	} else if let IpAddr::V6(v6) = inp {
		let oc = v6.segments();
		return 0x481040b16b00b135u64 |
			(u64::from(oc[0]) * 40100233) |
			(u64::from(oc[1]) * 40100100) |
			(u64::from(oc[2]) * 49521111);
	} else {
		panic!("that's not happening");
	}
}

fn get_ts() -> u64 {
	let a = SystemTime::now();
	let b = a.duration_since(UNIX_EPOCH).expect("Time travel can't be supported");
	return b.as_millis().try_into().expect("ARE WE IN THE FUTURE??");
}
