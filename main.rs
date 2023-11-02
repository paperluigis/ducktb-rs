#[macro_use]
mod types;
mod config;
mod conn;

// this is hardcoded; do not change.
const MAX_ROOMS_PER_CLIENT: u8 = 254;

use crate::config::*;
use crate::conn::{listen};
use crate::types::*;
use futures_util::future::join_all;
use nix::sys::socket::{setsockopt, sockopt};
use std::collections::HashMap;
use std::env;
use std::mem::{take, swap};
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
				leave_room_all_duck(uid, &mut ducks, &mut rooms).await;
				ducks.remove(&uid);
			},
			ClientOp::MsgUserJoined(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				mf.u.nick = duck.0;
				mf.u.color = duck.1;
				send_uni(mf, ServerOp::MsgRateLimits(S2CRateLimits::new(MAX_EVENTS.clone(), ()))).await;
				if duck.2.len() > MAX_ROOMS_PER_CLIENT as usize { kill_uni(mf).await; continue }
				send_uni(mf, ServerOp::MsgRoom(S2CRoom::new(duck.2.clone(), ()))).await;
				for a in duck.2 { join_room(uid, a, &mut ducks, &mut rooms, false, false).await; }
			},
			ClientOp::MsgMouse(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf).await; continue }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				ratelimit_check!(mf mouse { kill_uni(mf).await; continue });
				send_broad(rf, SBroadOp::MsgMouse(uid, duck.1, duck.2), &ducks).await
			},
			ClientOp::MsgTyping(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf).await; continue }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				ratelimit_check!(mf typing { kill_uni(mf).await; continue });
				if mf.is_typing[*duck.0 as usize] == duck.1 { continue }
				mf.is_typing[*duck.0 as usize] = duck.1;
				let b = mf.rooms[*duck.0 as usize].clone();
				send_broad(rf, SBroadOp::MsgTyping(rf.users.iter().filter(|p| {
					let mf = ducks.get(p).unwrap();
					let idx = mf.rooms.iter().position(|r| r==&b).unwrap();
					mf.is_typing[idx]
				}).map(|i| i.clone()).collect()), &ducks).await;
			},
			ClientOp::MsgRoomJoin(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				ratelimit_check!(mf room { kill_uni(mf).await; continue });
				if !duck.1 && mf.rooms.len() as u8 >= MAX_ROOMS_PER_CLIENT { kill_uni(mf).await; continue }
				join_room(uid, duck.0.clone(), &mut ducks, &mut rooms, duck.1, true).await;
			},
			ClientOp::MsgRoomLeave(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				ratelimit_check!(mf room { kill_uni(mf).await; continue });
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf).await; continue; }
				leave_room(uid, duck.0, &mut ducks, &mut rooms).await;
			},
			ClientOp::MsgMessage(uid, duck) => {
				// TODO: validate
				let mf = ducks.get_mut(&uid).expect("nope");
				ratelimit_check!(mf message { kill_uni(mf).await; continue });
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf).await; continue; }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				send_broad(rf, SBroadOp::MsgMessage(TextMessage {
					time: timestamp(),
					sid: uid,
					content: duck.1
				}), &ducks).await;
			},
			ClientOp::MsgUserChNick(uid, duck) => {
				let nick: UserNick;
				let color: UserColor;
				let mf = ducks.get_mut(&uid).expect("nope");
				ratelimit_check!(mf chnick { kill_uni(mf).await; continue });
				nick = mf.u.nick.clone();
				color = mf.u.color;
				mf.u.nick = duck.0.clone();
				mf.u.color = duck.1;
				let msg = SBroadOp::MsgUserChNick(uid, (nick, color), (duck.0, duck.1), timestamp());
				for ri in mf.rooms.clone() {
					let rf = rooms.get_mut(&ri).unwrap();
					send_broad(rf, msg.clone(), &ducks).await;
					send_broad(rf, SBroadOp::MsgUserUpdate(
						rf.users.iter().map(|p| ducks.get(p).unwrap().u.clone()).collect()
					), &ducks).await;
				}
			}
			ClientOp::Duck(_) => {
				for (_, sus) in &mut ducks {
					sus.counter.room = 0;
					sus.counter.mouse = 0;
					sus.counter.chnick = 0;
					sus.counter.message = 0;
					sus.counter.typing = 0;
					sus.counter.events = 0;
				}
			}
		}
	}
	let _ = join!(jh1, jh2);
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

async fn join_room(balls: UserID, joins: RoomID, ducks: &mut SusMap, rooms: &mut SusRoom, exclusive: bool, send_rooms: bool) -> RoomHandle {
	if exclusive {
		leave_room_all_duck(balls, ducks, rooms).await;
	}
	{
		let duck = ducks.get(&balls).expect("how did we get here?");
		if let Some(s) = to_room_handle(&duck.rooms, &joins) {
			return s;
		}
	}
	let room = match rooms.get_mut(&joins) {
		None => {
			let room = Room::new(joins.clone());
			rooms.insert(joins.clone(), room);
			rooms.get_mut(&joins)
		}
		a => a,
	}.unwrap();
	room.users.push(balls);
	{
		let b = ducks.get_mut(&balls).expect("how did we get here?");
		b.rooms.push(room.id.clone());
		b.is_typing.push(false);
	}
	let duck = ducks.get(&balls).expect("how did we get here?");
	let r = RoomHandle::new(duck.rooms.len() as u8 - 1);
	if send_rooms { send_uni(duck, ServerOp::MsgRoom(S2CRoom::new(duck.rooms.clone(), ()))).await; }
	send_broad(room, SBroadOp::MsgUserJoined(duck.u.clone(), timestamp()), ducks).await;
	send_broad(room, SBroadOp::MsgUserUpdate(room.users.iter().map(|p| ducks.get(p).unwrap().u.clone()).collect()), ducks).await;
	send_uni(duck, ServerOp::MsgHistory(S2CHistory::new(r, room.hist.clone()))).await;
	r
}
async fn leave_room(balls: UserID, leaves: RoomHandle, ducks: &mut SusMap, rooms: &mut SusRoom) -> usize {
	let duck = ducks.get(&balls).expect("how did we get here?");
	let room = rooms.get_mut(&duck.rooms[*leaves as usize]).unwrap();
	let idx = room.users.iter().position(|r| r==&balls);
	if let Some(idx) = idx { room.users.swap_remove(idx); }
	if room.users.len() == 0 {
		rooms.remove(&duck.rooms[*leaves as usize]);
	} else {
		send_broad(room, SBroadOp::MsgUserLeft(balls, timestamp()), ducks).await;
		send_broad(room, SBroadOp::MsgUserUpdate(room.users.iter().map(|p| ducks.get(p).unwrap().u.clone()).collect()), ducks).await;
	}
	let r = duck.rooms.len()-1;
	let duck = ducks.get_mut(&balls).expect("how did we get here?");
	duck.rooms.swap_remove(*leaves as usize);
	send_uni(duck, ServerOp::MsgRoom(S2CRoom::new(duck.rooms.clone(), ()))).await;
	r
}
async fn leave_room_all_duck(balls: UserID, ducks: &mut SusMap, rooms: &mut SusRoom) {
	let r = {
		let duck = ducks.get_mut(&balls).expect("how did we get here?");
		duck.is_typing.clear();
		take(&mut duck.rooms)
	};
	for d in r {
		let room = rooms.get_mut(&d).unwrap();
		let idx = room.users.iter().position(|r| r==&balls);
		if let Some(idx) = idx { room.users.swap_remove(idx); }
		if room.users.len() == 0 {
			rooms.remove(&d);
		} else {
			send_broad(room, SBroadOp::MsgUserLeft(balls, timestamp()), ducks).await;
			send_broad(room, SBroadOp::MsgUserUpdate(room.users.iter().map(|p| ducks.get(p).unwrap().u.clone()).collect()), ducks).await;
		}
	}
}

async fn kill_uni(to: &mut Susser) {
	let (mut t, _) = channel(1);
	swap(&mut to.tx, &mut t);
}
async fn send_uni(to: &Susser, c: ServerOp) -> Option<()> {
	to.tx.try_send(c).ok()
}
async fn send_broad(to: &mut Room, c: SBroadOp, ducks: &SusMap) {
	if let Some(he) = sbroadop_to_histentry(c.clone(), ducks) {
		push_history(to, he);
	}
	join_all(to.users.iter().map(|id| {
		let duck = ducks.get(id).unwrap();
		let rh = to_room_handle(&duck.rooms, &to.id).unwrap();
		let b = match c.clone() {
			SBroadOp::MsgMessage(t) => ServerOp::MsgMessage(S2CMessage::new(rh, t)),
			SBroadOp::MsgUserJoined(t,k) => ServerOp::MsgUserJoined(S2CUserJoined::new(rh, t, k)),
			SBroadOp::MsgUserLeft(t,o) => ServerOp::MsgUserLeft(S2CUserLeft::new(rh, t,o)),
			SBroadOp::MsgUserChNick(d,u,c,k) => ServerOp::MsgUserChNick(S2CUserChNick::new(rh, d,u,c,k)),
			SBroadOp::MsgUserUpdate(t) => ServerOp::MsgUserUpdate(S2CUserUpdate::new(rh, t)),
			SBroadOp::MsgTyping(t) => ServerOp::MsgTyping(S2CTyping::new(rh, t)),
			SBroadOp::MsgMouse(d,u,k) => ServerOp::MsgMouse(S2CMouse::new(rh, d,u,k))
		};
		send_uni(duck, b)
	})).await;
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
