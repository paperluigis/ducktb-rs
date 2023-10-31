mod types;
mod config;
mod conn;

use crate::config::*;
use crate::conn::{listen};
use crate::types::*;
use futures_util::future::join_all;
use nix::sys::socket::{setsockopt, sockopt};
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::os::fd::AsRawFd;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::{spawn, join};
use tokio::net::TcpListener;
use tokio::sync::mpsc::{Sender, channel};
use tokio::time::{Duration, interval};

type SusMap = HashMap<UserID, Susser>;
type SusRoom = HashMap<RoomID, Room>;

#[tokio::main]
async fn main() {
	let bind_to = env::var("BIND").unwrap_or("127.0.0.1:8000".into()).parse::<SocketAddr>().expect("Invalid socket address");
	let listener = TcpListener::bind(&bind_to).await.expect("Failed to bind socket");
	println!("Listening on {}...", bind_to);
	setsockopt(listener.as_raw_fd(), sockopt::ReuseAddr, &true).ok();

	let (tx_msg, mut messages) = channel(300);
	let mut ducks = SusMap::new();
	let mut rooms = SusRoom::new();
	let jh1 = spawn(listen(listener, tx_msg.clone()));
	let jh2 = spawn(timer(tx_msg));
	while let Some(i) = messages.recv().await {
		match i {
			ClientOp::Connection(uid, balls) => {
				println!("\x1b[33mconn+ \x1b[34m[{}|{:?}]\x1b[0m", uid, balls.ip);
				if balls.tx.send(ServerOp::MsgHello(S2CHello::new(HELLO_IDENTITY.into(), uid))).await.is_err() { break }
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
				send_uni(mf, ServerOp::MsgRateLimits(S2CRateLimits::new(MAX_EVENTS.clone(), ()))).await;
				join_room(uid, duck.2, &mut ducks, &mut rooms).await;
			},
			ClientOp::MsgMouse(uid, duck) => {
				let mf = ducks.get(&uid).expect("nope");
				let rf = rooms.get_mut(&mf.rooms[0]).expect("no way");
				send_broad(rf, ServerOp::MsgMouse(S2CMouse::new(uid, duck.0, duck.1)), &ducks, false).await;
			},
			ClientOp::MsgTyping(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				let rf = rooms.get_mut(&mf.rooms[0]).expect("no way");
				mf.counter.typing += 1;
				if mf.counter.typing > MAX_EVENTS.typing { kill_uni(mf).await; continue; }
				if mf.is_typing == duck.0 { kill_uni(mf).await; continue; }
				mf.is_typing = duck.0;
				send_broad(rf, ServerOp::MsgTyping(S2CTyping::new(
					rf.users.iter().filter(|p| ducks.get(p).unwrap().is_typing).map(|i| i.clone()).collect(), ()
				)), &ducks, false).await;
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
				send_broad(rf, ServerOp::MsgMessage(S2CMessage::new(TextMessage { time: timestamp(), sid: uid, content: duck.0 }, ())), &ducks, true).await;
			},
			ClientOp::MsgUserChNick(uid, duck) => {
				// TODO: validate
				let nick: UserNick;
				let color: UserColor;
				let mf = ducks.get_mut(&uid).expect("nope");
				nick = mf.u.nick.clone();
				color = mf.u.color;
				mf.u.nick = duck.0.clone();
				mf.u.color = duck.1;
				let msg = ServerOp::MsgUserChNick(S2CUserChNick::new(uid, (nick, color), (duck.0, duck.1), timestamp()));
				for ri in mf.rooms.clone() {
					let rf = rooms.get_mut(&ri).unwrap();
					send_broad(rf, msg.clone(), &ducks, true).await;
				}
			}
			ClientOp::Duck(i) => {
				for (uid, sus) in &mut ducks {
					sus.counter.mouse = 0;
					sus.counter.chnick = 0;
					sus.counter.message = 0;
					sus.counter.typing = 0;
					sus.counter.events = 0;
				}
			}
		}
	}
	join!(jh1, jh2);
}

async fn timer(t: Sender<ClientOp>) -> Option<()> {
	let mut it = interval(Duration::from_secs(5));
	let mut i = 0u32;
	loop {
		it.tick().await;
		t.send(ClientOp::Duck(i)).await.ok()?;
		i += 1;
	}
}

async fn join_room(balls: UserID, joins: RoomID, with_da: &mut SusMap, in_the: &mut SusRoom) -> usize {
	let room = match in_the.get_mut(&joins) {
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
	send_uni(duck, ServerOp::MsgRoom(S2CRoom::new(joins.clone(), ()))).await;
	send_broad(room, ServerOp::MsgUserJoined(S2CUserJoined::new(duck.u.clone(), timestamp())), with_da, true).await;
	send_broad(room, ServerOp::MsgUserUpdate(S2CUserUpdate::new(
		room.users.iter().map(|p| with_da.get(p).unwrap().u.clone()).collect(), ()
	)), with_da, true).await;
	send_uni(duck, ServerOp::MsgHistory(S2CHistory::new(room.hist.clone(), ()))).await;
	// send_uni history
	r
}
async fn leave_room(balls: UserID, leaves: RoomID, with_da: &mut SusMap, in_the: &mut SusRoom) -> usize {
	let duck = with_da.get_mut(&balls).expect("how did we get here?");
	let room = in_the.get_mut(&leaves).expect("i'm not even in the room");
	let idx = room.users.iter().position(|r| r==&balls);
	if let Some(idx) = idx { room.users.swap_remove(idx); }
	let idx = duck.rooms.iter().position(|r| r==&leaves);
	if let Some(idx) = idx { duck.rooms.swap_remove(idx); }
	let r = duck.rooms.len();
	send_broad(room, ServerOp::MsgUserLeft(S2CUserLeft::new(balls, timestamp())), with_da, true).await;
	send_broad(room, ServerOp::MsgUserUpdate(S2CUserUpdate::new(
		room.users.iter().map(|p| with_da.get(p).unwrap().u.clone()).collect(), ()
	)), with_da, true).await;
	// TODO: remove room from memory if r == 0
	r
}

async fn kill_uni(to: &Susser) {
	send_uni(to, ServerOp::Disconnect).await;
}
async fn send_uni(to: &Susser, c: ServerOp) -> Option<()> {
	to.tx.send(c).await.ok()
}
async fn send_uni_drop(to: &Susser, c: ServerOp) -> Option<()> {
	to.tx.try_send(c).ok()
	//if to.tx.try_send(c).is_err() {
	//	// KILL YOURSELF... NOW!
	//	to.tx.send(ServerOp::Disconnect).await.ok()
	//} else { Some(()) }
}
async fn send_broad(to: &mut Room, c: ServerOp, ducks: &SusMap, reliable: bool) {
	if let Some(he) = serverop_to_histentry(c.clone(), ducks) {
		push_history(to, he);
	}
	if reliable {
		join_all(to.users.iter().map(|id| send_uni(ducks.get(id).unwrap(), c.clone()))).await;
	} else {
		join_all(to.users.iter().map(|id| send_uni_drop(ducks.get(id).unwrap(), c.clone()))).await;
	}
}

fn push_history(t: &mut Room, h: HistEntry) {
	if t.hist.len() == HIST_ENTRY_MAX-1 { t.hist.pop_front(); }
	t.hist.push_back(h);
}

fn timestamp() -> u64 {
	let a = SystemTime::now();
	let b = a.duration_since(UNIX_EPOCH).expect("Time travel can't be supported");
	return b.as_millis().try_into().expect("ARE WE IN THE FUTURE??");
}
