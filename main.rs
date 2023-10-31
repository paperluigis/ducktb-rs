// what should we send as the "server" field in the hello message?
const HELLO_IDENTITY: &str = "never liked ur smile brah";
// should we trust the X-Forwarded-For header?
const TRUST_REAL_IP_HEADER: bool = true;
// what room should we consider the default?
const LOBBY_ROOM_NAME: &str = "lobby";
// what rooms should we keep in memory even if there aren't any users in them?
const KEEP_ROOMS: [&'static str; 2] = ["lobby", "duck-room"];
// how many messages should we store in rooms?
const HIST_ENTRY_MAX: usize = 512;
// how many events are users allowed to send in a 5-second period?
const MAX_MOUSE: u8 = 100;
const MAX_CHNICK: u8 = 1;
const MAX_MESSAGE: u8 = 5;
const MAX_TYPING: u8 = 8;

use derive_more::{Display, Deref, From};
use derive_new::new;
use futures::future::join_all;
use futures_util::{SinkExt, StreamExt};
use nix::sys::socket::{setsockopt, sockopt};
use serde::{de::Error, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{json, Value};
use std::{
	os::fd::AsRawFd,
	env, fmt, str::FromStr,
	collections::{VecDeque, HashMap},
	net::IpAddr,
	time::{SystemTime, UNIX_EPOCH}
};
use tokio::{
	net::{TcpListener, TcpStream},
	sync::mpsc::{channel, Sender},
	spawn, select
};
use tokio_tungstenite::{
	accept_hdr_async,
	tungstenite::{Message, handshake::server::{Request, Response, ErrorResponse}}
};

#[derive(PartialEq, Clone, Debug, Display, Deref, Serialize, Deserialize, From)]
struct UserNick(String);
// format is 0rgb
#[derive(Clone, Copy, PartialEq, Debug, Deref, From)]
struct UserColor(u32);
#[derive(Hash, Eq, Clone, Copy, PartialEq, Debug, Deref)]
struct UserID(u32);
#[derive(Clone, Hash, Eq, PartialEq, Debug, Display, Deref, Serialize, Deserialize, From)]
struct RoomID(String);
#[derive(Clone, Copy, PartialEq, Debug, Deref, From)]
struct UserHashedIP(u64);

impl Serialize for UserID {
	fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S: Serializer {
		if s.is_human_readable() {
			s.serialize_str(&self.to_string())
		} else {
			s.serialize_u32(self.0)
		}
	}
}
impl Serialize for UserColor {
	fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S: Serializer {
		if s.is_human_readable() {
			s.serialize_str(&self.to_string())
		} else {
			s.serialize_u32(self.0)
		}
	}
}
impl Serialize for UserHashedIP {
	fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S: Serializer {
		if s.is_human_readable() {
			s.serialize_str(&self.to_string())
		} else {
			s.serialize_u64(self.0)
		}
	}
}
impl<'de> Deserialize<'de> for UserID {
	fn deserialize<D>(d: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
		if d.is_human_readable() {
			let s = String::deserialize(d)?;
			Self::from_str(&s).map_err(D::Error::custom)
		} else {
			let b = u32::deserialize(d)?;
			Ok(Self(b))
		}
	}
}
impl<'de> Deserialize<'de> for UserColor {
	fn deserialize<D>(d: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
		if d.is_human_readable() {
			let s = String::deserialize(d)?;
			Self::from_str(&s).map_err(D::Error::custom)
			//Self::from_str(&s).map_err(|_| Deserializer::Error)?
		} else {
			let b = u32::deserialize(d)?;
			if b > 0xffffff { return Err(D::Error::custom("that ain't a valid 0rgb 32-bit color")) };
			Ok(Self(b))
		}
	}
}
/*impl Deserialize<'de> for UserHashedIP {
	fn deserialize<D>(&self, deserializer: D) -> Result<Self, D::Error> where D: Deserializer {
		Ok(Self(if d.is_human_readable() {
			let s = String::deserialize(deserializer)?;
			u64::from_str(s)?
		} else {
			u64::deserialize(deserializer)?
		}))
	}
}*/

impl fmt::Display for UserColor {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "#{:0>6x}", self.0)
	}
}
impl fmt::Display for UserID {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:0>8x}", self.0)
	}
}
impl fmt::Display for UserHashedIP {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:0>16x}", self.0)
	}
}

struct SillyParsingError;
impl fmt::Display for SillyParsingError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "The client got too silly.")
	}
}

impl FromStr for UserID {
	type Err = SillyParsingError;
	fn from_str(inp: &str) -> Result<Self, Self::Err> {
		if inp.len() == 8 {
			return Ok(UserID(u32::from_str_radix(&inp, 16).map_err(|_| SillyParsingError)?));
		} else {
			return Err(SillyParsingError);
		}
	}
}
impl FromStr for UserColor {
	type Err = SillyParsingError;
	fn from_str(inp: &str) -> Result<Self, Self::Err> {
		if inp.len() == 7 && inp.starts_with("#") {
			return Ok(UserColor(u32::from_str_radix(&inp[1..], 16).map_err(|_| SillyParsingError)?));
		} else {
			return Err(SillyParsingError);
		}
	}
}

type SusMap = HashMap<UserID, Susser>;
type SusRoom = HashMap<RoomID, Room>;

#[derive(Debug, Clone, Serialize)]
struct HistMsg {
	home: UserHashedIP,
	sid: UserID,
	content: String,
	nick: UserNick,
	color: UserColor,
	ts: u64
}
#[derive(Debug, Clone, Serialize)]
struct HistJoin {
	home: UserHashedIP,
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	ts: u64
}
#[derive(Debug, Clone, Serialize)]
struct HistLeave {
	home: UserHashedIP,
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	ts: u64
}
#[derive(Debug, Clone, Serialize)]
struct HistChNick {
	home: UserHashedIP,
	sid: UserID,
	old_nick: UserNick,
	old_color: UserColor,
	new_nick: UserNick,
	new_color: UserColor,
	ts: u64
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="lowercase", tag="type")]
enum HistEntry {
	Message(HistMsg),
	Join(HistJoin),
	Leave(HistLeave),
	ChNick(HistChNick)
}


#[derive(Debug, new)]
struct Room {
	#[new(default)]
	hist: VecDeque<HistEntry>,
	id: RoomID,
	#[new(default)]
	users: Vec<UserID>
}

#[derive(Debug, Clone, new)]
struct SusRate {
	#[new(value="0")]
	mouse: u8,
	#[new(value="0")]
	chnick: u8,
	#[new(value="0")]
	message: u8,
	#[new(value="0")]
	typing: u8
}

#[derive(Debug)]
struct Susser {
	// internal fields
	counter: SusRate,
	ip: IpAddr,
	tx: Sender<ServerOp>,
	is_typing: bool,
	rooms: Vec<RoomID>,

	u: User
}

#[derive(Debug, Clone, Serialize)]
struct User {
	#[serde(rename="sid")]
	id: UserID,
	nick: UserNick,
	color: UserColor,
	#[serde(rename="home")]
	haship: UserHashedIP
}

#[derive(Debug, Clone, Serialize)]
struct TextMessage {
	time: u64,
	sid: UserID,
	content: String
}

// `, #[serde(skip)] ()` is a workaround to treat a newtype struct as a tuple with one element
// this is done for consistency with all other messages
#[derive(Debug, Deserialize)]
struct C2SUserJoined(UserNick, UserColor, RoomID);
#[derive(Debug, Deserialize)]
struct C2SUserChNick(UserNick, UserColor);
#[derive(Debug, Deserialize)]
struct C2SRoom(RoomID, #[serde(skip)] ());
#[derive(Debug, Deserialize)]
struct C2SMessage(String, #[serde(skip)] ());
#[derive(Debug, Deserialize)]
struct C2STyping(bool, #[serde(skip)] ());
#[derive(Debug, Deserialize)]
struct C2SMouse(f32, f32);

#[derive(Debug, Clone, Serialize)]
struct S2CHello(String, UserID);
#[derive(Debug, Clone, Serialize)]
struct S2CRoom(RoomID, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct S2CHistory(VecDeque<HistEntry>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct S2CUserJoined(User, u64);
#[derive(Debug, Clone, Serialize)]
struct S2CUserLeft(UserID, u64);
#[derive(Debug, Clone, Serialize)]
struct S2CMouse(UserID, f32, f32);
#[derive(Debug, Clone, Serialize)]
struct S2CUserUpdate(Vec<User>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct S2CTyping(Vec<UserID>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize)]
struct S2CMessage(TextMessage, #[serde(skip)] ());

#[derive(Debug)]
enum ClientOp {
	// duck
	Duck(u32),
	// client connection states
	Connection(UserID, Susser),
	Disconnect(UserID),
	// client messages
	MsgUserJoined(UserID, C2SUserJoined),
	MsgUserChNick(UserID, C2SUserChNick),
	MsgRoom(UserID, C2SRoom),
	MsgMessage(UserID, C2SMessage),
	MsgTyping(UserID, C2STyping),
	MsgMouse(UserID, C2SMouse)
}

#[derive(Debug, Clone)]
enum ServerOp {
	// server be like "you should kill yourself NOW"
	Disconnect,
	// server messages
	MsgHello(S2CHello),
	MsgRoom(S2CRoom),
	MsgHistory(S2CHistory),
	MsgUserJoined(S2CUserJoined),
	MsgUserLeft(S2CUserLeft),
	MsgMouse(S2CMouse),
	MsgUserUpdate(S2CUserUpdate),
	MsgTyping(S2CTyping),
	MsgMessage(S2CMessage)
}

// ========== logic handling side ==========
#[tokio::main]
async fn main() {
	let wtf = env::var("BIND").unwrap_or("127.0.0.1:8000".into());
	let que = TcpListener::bind(&wtf).await.expect("DANG IT");
	let rf = que.as_raw_fd();
	setsockopt(rf, sockopt::ReuseAddr, &true).ok();
	println!("pls work :skull: (listening on {})", wtf);
	let (tx_msg, mut messages) = channel(8);
	let mut ducks = SusMap::new();
	let mut rooms = SusRoom::new();
	let jh = spawn(listen(que, tx_msg));
	while let Some(i) = messages.recv().await {
		match i {
			ClientOp::Connection(uid, balls) => {
				println!("\x1b[33mconn+ \x1b[34m[{}|{:?}]\x1b[0m", uid, balls.ip);
				if balls.tx.send(ServerOp::MsgHello(S2CHello(HELLO_IDENTITY.into(), uid))).await.is_err() { break }
				ducks.insert(uid, balls);
			},
			ClientOp::Disconnect(uid) => {
			    println!("\x1b[31mconn- \x1b[34m[{}]\x1b[0m", uid);
				let balls = ducks.get(&uid).unwrap();
				for rid in balls.rooms.clone() { leave_room(uid, rid.clone(), &mut ducks, &mut rooms).await; }
				ducks.remove(&uid);
			},
			ClientOp::MsgUserJoined(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				// TODO: validate
				mf.u.nick = duck.0;
				mf.u.color = duck.1;
				join_room(uid, duck.2, &mut ducks, &mut rooms).await;
			},
			ClientOp::MsgMouse(uid, duck) => {
				let mf = ducks.get(&uid).expect("nope");
				let rf = rooms.get_mut(&mf.rooms[0]).expect("no way");
				send_broad(rf, ServerOp::MsgMouse(S2CMouse(uid, duck.0, duck.1)), &ducks).await;
			},
			ClientOp::MsgTyping(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				let rf = rooms.get_mut(&mf.rooms[0]).expect("no way");
				if mf.is_typing == duck.0 { continue }
				mf.is_typing = duck.0;
				send_broad(rf, ServerOp::MsgTyping(S2CTyping(
					rf.users.iter().filter(|p| ducks.get(p).unwrap().is_typing).map(|i| i.clone()).collect(), ()
				)), &ducks).await;
			},
			ClientOp::MsgRoom(uid, duck) => {
				// TODO: validate
				let mf = ducks.get(&uid).expect("nope");
				leave_room(uid, mf.rooms[0].clone(), &mut ducks, &mut rooms).await;
				join_room(uid, duck.0, &mut ducks, &mut rooms).await;
			},
			ClientOp::MsgMessage(uid, duck) => {
				// TODO: validate
				let mf = ducks.get_mut(&uid).expect("nope");
				let rf = rooms.get_mut(&mf.rooms[0]).expect("no way");
				send_broad(rf, ServerOp::MsgMessage(S2CMessage(TextMessage { time: timestamp(), sid: uid, content: duck.0 }, ())), &ducks).await;
			},
			ClientOp::MsgUserChNick(uid, duck) => {}
			ClientOp::Duck(i) => {}
		}
	}
	let _ = jh.await;
}

async fn join_room(balls: UserID, joins: RoomID, with_da: &mut SusMap, in_the: &mut SusRoom) -> usize {
	let mut room = match in_the.get_mut(&joins) {
		None => {
			let room = Room::new(joins.clone());
			in_the.insert(joins.clone(), room);
			in_the.get_mut(&joins)
		}
		a => a,
	}.unwrap();
	room.users.push(balls);
	with_da.get_mut(&balls).expect("how did we get here?").rooms.push(room.id.clone());
	let duck = with_da.get(&balls).expect("how did we get here?");
	let r = duck.rooms.len();
	send_uni(duck, ServerOp::MsgRoom(S2CRoom(joins.clone(), ()))).await;
	send_broad(room, ServerOp::MsgUserJoined(S2CUserJoined(duck.u.clone(), timestamp())), with_da).await;
	send_broad(room, ServerOp::MsgUserUpdate(S2CUserUpdate(
		room.users.iter().map(|p| with_da.get(p).unwrap().u.clone()).collect(), ()
	)), with_da).await;
	send_uni(duck, ServerOp::MsgHistory(S2CHistory(room.hist.clone(), ()))).await;
	// send_uni history
	r
}
async fn leave_room(balls: UserID, leaves: RoomID, with_da: &mut SusMap, in_the: &mut SusRoom) -> usize {
	let duck = with_da.get_mut(&balls).expect("how did we get here?");
	let mut room = in_the.get_mut(&leaves).expect("i'm not even in the room");
	let idx = room.users.iter().position(|r| r==&balls);
	if let Some(idx) = idx { room.users.swap_remove(idx); }
	let idx = duck.rooms.iter().position(|r| r==&leaves);
	if let Some(idx) = idx { duck.rooms.swap_remove(idx); }
	let r = duck.rooms.len();
	send_broad(room, ServerOp::MsgUserLeft(S2CUserLeft(balls, timestamp())), with_da).await;
	send_broad(room, ServerOp::MsgUserUpdate(S2CUserUpdate(
		room.users.iter().map(|p| with_da.get(p).unwrap().u.clone()).collect(), ()
	)), with_da).await;
	// TODO: remove room from memory if r == 0
	r
}

async fn send_uni(to: &Susser, c: ServerOp) {
	let _ = to.tx.send(c).await;
}
async fn send_broad(to: &mut Room, c: ServerOp, ducks: &SusMap) {
	match c {
		ServerOp::MsgUserJoined(ref m) => push_history(to, HistEntry::Join(HistJoin {
			ts: m.1,
			nick: m.0.nick.clone(),
			home: m.0.haship,
			color: m.0.color,
			sid: m.0.id
		})),
		_ => ()
	}
	join_all(to.users.iter().map(|id| send_uni(ducks.get(id).unwrap(), c.clone()))).await;
}
fn push_history(t: &mut Room, h: HistEntry) {
	if t.hist.len() == HIST_ENTRY_MAX-1 { t.hist.pop_front(); }
	t.hist.push_back(h);
}

// ========== connection handling side ==========
async fn listen(l: TcpListener, t: Sender<ClientOp>) {
	let mut conn_seq: UserID = UserID(0x48aeb931);
	while let Ok((flow, _)) = l.accept().await {
		spawn(wrap_conn(flow, conn_seq, t.clone()));
		conn_seq.0 += 1984;
	}
}

async fn wrap_conn(y: TcpStream, ee: UserID, t: Sender<ClientOp>) {
	conn(y, ee, &t).await;
}

async fn conn(y: TcpStream, ee: UserID, t: &Sender<ClientOp>) {
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
			color: UserColor(0),
			nick: UserNick("".into()),
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
			msg = messages.recv() => {
				if let Some(msg) = msg {
					if tws.send(Message::Text(match msg {
						ServerOp::Disconnect => break,
						ServerOp::MsgHello(s) =>      format!("HELLO\0{}",       serde_json::to_string(&s).unwrap()),
						ServerOp::MsgMouse(s) =>      format!("MOUSE\0{}",       serde_json::to_string(&s).unwrap()),
						ServerOp::MsgRoom(s) =>       format!("ROOM\0{}",        serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserUpdate(s) => format!("USER_UPDATE\0{}", serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserJoined(s) => format!("USER_JOINED\0{}", serde_json::to_string(&s).unwrap()),
						ServerOp::MsgUserLeft(s) =>   format!("USER_LEFT\0{}",   serde_json::to_string(&s).unwrap()),
						ServerOp::MsgTyping(s) =>     format!("TYPING\0{}",      serde_json::to_string(&s).unwrap()),
						ServerOp::MsgMessage(s) =>    format!("MESSAGE\0{}",     serde_json::to_string(&s).unwrap()),
						ServerOp::MsgHistory(s) =>    format!("HISTORY\0{}",     serde_json::to_string(&s).unwrap()),
					})).await.is_err() { break }
				}
			}
			msg = rws.next() => {
				match msg {
					Some(Ok(Message::Text(str))) => {
						if message(str, ee, t, msg_1st).await.is_none() { break }
					}
					_ => break
				}
				msg_1st = false;
			}
		}
	}
	let _ = t.send(ClientOp::Disconnect(ee)).await;
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
		t.send(match tp {
			"MOUSE"       => ClientOp::MsgMouse  (uid, serde_json::from_str::<C2SMouse>(&rr).ok()?),
			"TYPING"      => ClientOp::MsgTyping (uid, serde_json::from_str::<C2STyping>(&rr).ok()?),
			"MESSAGE"     => ClientOp::MsgMessage(uid, serde_json::from_str::<C2SMessage>(&rr).ok()?),
			"ROOM"        => ClientOp::MsgRoom   (uid, serde_json::from_str::<C2SRoom>(&rr).ok()?),
			//"" => ClientOp::Msg(uid, serde_json::from_str::<C2S>(&rr).ok()?),
			_ => { println!("received {}, which is unimplemented...", tp); return None }
		}).await.ok()?;
	}
	Some(())
}

/*
tws.send(format!("HELLO\0[\"{}\",\"{:0>8x}\"]", srv, seq).into()).await;
leave_room(seq, &ducks, &rooms);

fn leave_room(id: UserID, ducks: &SusMap, rooms: &SusRoom) {
	let mut lurks = ducks.lock().unwrap();
	println!("ducks mutex lock line {}", std::line!());
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
	println!("rooms mutex lock line {}", std::line!());
	let hoho = rooks.get_mut(&room).unwrap();
	hoho.conn_users -= 1;
	if hoho.conn_users != 0 || room == LOBBY_ROOM_NAME { return }
	rooks.remove(&room);
}

fn join_room(room: RoomID, id: UserID, ducks: &SusMap, rooms: &SusRoom) {
	let room = room.to_string();
	let mut lurks = ducks.lock().unwrap();
	println!("ducks mutex lock line {}", std::line!());
	let x = lurks.get_mut(&id).unwrap();
	if room == x.in_room { return }
	drop(lurks);
	leave_room(id, ducks, rooms);
	let mut lurks = ducks.lock().unwrap();
	println!("ducks mutex lock line {}", std::line!());
	let x = lurks.get_mut(&id).unwrap();

	let mut rooks = rooms.lock().unwrap();
	println!("rooms mutex lock line {}", std::line!());
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

*/

/*
fn send_userleave(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	nick: &str, col: UserColor, uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	println!("rooms mutex lock line {}", std::line!());
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::Leave(HistLeave {
		"nick": nick.into(),
		"home": ip,
		"color": col,
		"sid": uid,
		"ts": ts
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, &ducks, "USER_LEFT".into(), array![ sid_to_str(uid), ts ]);
	send_broad(&to_room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &to_room) ]);
}

fn send_userjoin(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	nick: &str, col: UserColor, uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	println!("rooms mutex lock line {}", std::line!());
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::Join(HistJoin {
		"nick": nick.into(),
		"home": ip,
		"color": col,
		"sid": uid,
		"ts": ts
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, &ducks, "USER_JOINED".into(), array![{ sid: sid_to_str(uid), nick: nick.clone(), color: col_to_str(col), time: ts }]);
	send_broad(&to_room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &to_room) ]);
}

fn send_userchnick(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	old_nick: &str, old_col: UserColor, new_nick: &str, new_col: UserColor,
	uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	println!("roomss mutex lock line {}", std::line!());
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::ChNick(HistChNick {
		"old_nick": old_nick.into(),
		"new_nick": new_nick.into(),
		"home": ip,
		"old_color": old_col,
		"new_color": new_col,
		"sid": uid,
		"ts": ts
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, ducks, "USER_CHANGE_NICK".into(), array![sid_to_str(uid), [old_nick,col_to_str(old_col)],[new_nick,col_to_str(new_col)], ts]);
	send_broad(&to_room, &ducks, "USER_UPDATE".into(), array![ ducktosl(ducks, &to_room) ]);
}

fn send_message(to_room: RoomID, ducks: &SusMap, rooms: &SusRoom,
	nick: &str, col: UserColor, content: String, uid: UserID, ip: UserHashedIP, ts: u64) {
	let mut rooks = rooms.lock().unwrap();
	println!("rooms mutex lock line {}", std::line!());
	let hoho = rooks.get_mut(&to_room).unwrap();
	let entry = HistEntry::Message(HistMsg {
		"nick": nick.into(),
		"home": ip,
		"color": col,
		"sid": uid,
		"ts": ts,
		"content": content.clone()
	});
	if hoho.hist.len() >= HIST_ENTRY_MAX {
		let _ = hoho.hist.pop_front();
	}
	hoho.hist.push_back(entry);
	send_broad(&to_room, ducks, "MESSAGE".into(), array![{
		"sid": sid_to_str(uid),
		"time": ts,
		"content": content.clone()
	}]);
}

fn send_broad(to_room: &str, ducks: &SusMap, t: String, val: Value) {
	if t != "MOUSE" {
		if to_room == "" {
			println!("\x1b[33mtx    \x1b[34m[broadcast]\x1b[0m {:?} {}", t, val.dump());
		} else {
			println!("\x1b[33mtx    \x1b[34m[broadcast|\x1b[37m{}\x1b[34m]\x1b[0m {:?} {}", to_room, t, val.dump());
		}
	}
	let ducks = ducks.lock().unwrap();
	println!("ducks mutex lock line {}", std::line!());
	for (_, balls) in ducks.iter() {
		if to_room != "" && balls.in_room != to_room { continue }
		let _ = balls.tx.unbounded_send(Message::Text(format!("{}\0{}", t, val.dump())));
	}
}

fn send_uni(to_id: UserID, ducks: &SusMap, t: String, val: Value) {
	if t != "HISTORY" {
		println!("\x1b[34mtx    \x1b[34m[{:0>8x}]\x1b[0m {:?} {}", to_id, t, val.dump());
	}
	let ducks = ducks.lock().unwrap();
	println!("ducks mutex lock line {}", std::line!());
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
	println!("ducks mutex lock line {}", std::line!());
		let balls = unducks.get_mut(&id).unwrap();

		if balls.in_room != "" { return false }
		let nick = &jv[0];
		let color = &jv[1];
		let room = &jv[2];
		let jl = jv.len();
		if	!(jl == 2 || jl == 3) ||
			!nick.is_string() ||
			!color.is_string() ||
			!(jl == 2 || room.is_string()) { return false }
		let color = str_to_col(color.as_str().unwrap());
		if	color.is_none() { return false }
		let nick = nick.as_str().unwrap().trim();
		let color = color.unwrap();
		let room = room.as_str().unwrap_or(LOBBY_ROOM_NAME).trim();
		if	nick == "" ||
			room == "" { return false }
		balls.nick = nick.to_string();
		balls.color = color;
		// balls.in_room = room.to_string();
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
	println!("ducks mutex lock line {}", std::line!());
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
	println!("ducks mutex lock line {}", std::line!());
					let lel = lel.get_mut(&id).unwrap();
					lel.counter.typing += 1;
					if lel.counter.typing > MAX_TYPING { return false }
					if let Value::Boolean(c) = typing {
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
	println!("ducks mutex lock line {}", std::line!());
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
	println!("ducks mutex lock line {}", std::line!());
				let room = bb.get(&id).unwrap().in_room.clone();
				drop(bb);
				send_broad(&room, ducks, "MOUSE".into(), array![sid_to_str(id), x.clone(), y.clone()]);
			}
			"USER_CHANGE_NICK\0" => {
				let nick = &jv[0];
				let color = &jv[1];
				let jl = jv.len();
				if	jl != 2 ||
					!nick.is_string() ||
					!color.is_string() { return false }
				let color = str_to_col(color.as_str().unwrap());
				if	color.is_none() { return false }
				let nick = nick.as_str().unwrap().trim();
				let color = color.unwrap();
				let room: String;
				let pnick: String;
				let pcol: UserColor;
				let hip: UserHashedIP;
				{
					let mut lel = ducks.lock().unwrap();
	println!("ducks mutex lock line {}", std::line!());
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
*/

/*
fn list_to_json(ducks: &SusMap, room: &RoomID) -> Value {
	let mut arr: Vec<Value> = Vec::with_capacity(ducks.len());
	for (id, balls) in ducks.iter() {
		if room.0 != "" && &balls.in_room != room { continue }
		let o = json!({
			"sid": id.to_string(),
			"nick": balls.nick.0.clone(),
			"color": balls.color.to_string(),
			"home": balls.haship.to_string()
		});
		arr.push(o);
	}
	return Value::Array(arr);
}
*/

fn hist_to_json(v: &VecDeque<HistEntry>) -> Value {
	let mut a: Vec<Value> = Vec::with_capacity(v.len());
	for entry in v.iter() {
		let _ = a.push(match entry {
			HistEntry::Leave(b) => json!({
				"left": true,
				"time": b.ts,
				"home": b.home.to_string(),
				"sid": b.sid.to_string(),
				"nick": b.nick.0.to_string(),
				"color": b.color.to_string()
			}),
			HistEntry::Join(b) => json!({
				"joined": true,
				"time": b.ts,
				"home": b.home.to_string(),
				"sid": b.sid.to_string(),
				"nick": b.nick.0.to_string(),
				"color": b.color.to_string()
			}),
			HistEntry::Message(b) => json!({
				"time": b.ts,
				"home": b.home.to_string(),
				"sid": b.sid.to_string(),
				"nick": b.nick.0.to_string(),
				"color": b.color.to_string(),
				"message": b.content.clone()
			}),
			HistEntry::ChNick(b) => json!({
				"time": b.ts,
				"home": b.home.to_string(),
				"sid": b.sid.to_string(),
				"nick": b.old_nick.0.to_string(),
				"color": b.old_color.to_string(),
				"newnick": b.new_nick.0.to_string(),
				"newcolor": b.new_color.to_string(),
			})
		});
	}
	return Value::Array(a);
}

fn hash_ip(inp: &IpAddr) -> UserHashedIP {
	//todo: make it irreversible
	if let IpAddr::V4(v4) = inp {
		let oc = v4.octets();
		// hash the /16 subnet
		return UserHashedIP(0x19fa920130b0ba21u64 |
			(u64::from(oc[0]) * 10495007) |
			(u64::from(oc[1]) * 39950100));
	} else if let IpAddr::V6(v6) = inp {
		let oc = v6.segments();
		// hash the /48 subnet
		return UserHashedIP(0x481040b16b00b135u64 |
			(u64::from(oc[0]) * 40100233) |
			(u64::from(oc[1]) * 40100100) |
			(u64::from(oc[2]) * 49521111));
	} else {
		panic!("that's not happening");
	}
}

fn timestamp() -> u64 {
	let a = SystemTime::now();
	let b = a.duration_since(UNIX_EPOCH).expect("Time travel can't be supported");
	return b.as_millis().try_into().expect("ARE WE IN THE FUTURE??");
}
