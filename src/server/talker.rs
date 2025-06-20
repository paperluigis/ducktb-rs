use crate::config::TalkerConfig;
use crate::types::*;

use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use concat_idents::concat_idents;
use derive_new::new;
use futures::future::FutureExt;
use futures_util::future::join_all;
use ringmap::RingMap;
use tokio::sync::mpsc::{Sender, Receiver};
use tokio::select;

// some fields from S2CResponse
type Response = Option<(ResponseCodes, Option<String>, Option<ResponseTypes>)>;

struct UserRoomJoinCompletionData {
	// data for HISTORY message
	hist: Vec<HistEntry>,
	// data for USER_UPDATE message
	uobjs: Vec<User>,
	// the user that joined
	uobj: User,
	// room id where the user joined
	rid: RoomID,
	rh: RoomHandle,
	// message id for the "user joined" text message
	mid: MessageID,
}

/// c2s
#[derive(Debug, Clone)]
pub enum TalkerRxOp {
	ReloadCfg(TalkerConfig),
	Shutdown,
	Request(UserID, Option<RequestID>, C2S),
	Connect(UserID, RemoteAddress),
	Disconnect(UserID)
}

/// s2c
#[derive(Debug, Clone)]
pub enum TalkerTxOp {
	Event(UserID, S2C),
}

pub struct Talker {
	config: TalkerConfig,
	tx: Sender<TalkerTxOp>,
	users: HashMap<UserID, UserState>,
	rooms: HashMap<RoomID, RoomState>,
	ratelimit_idx: usize, // 0..RATELIMIT_WINDOW
	msgid_counter: u32,
}

#[derive(new, Debug)]
struct UserState {
	sid: UserID,
	addr: RemoteAddress,
	#[new(default)]
	nick: UserNick,
	#[new(default)]
	color: UserColor,

	#[new(default)]
	in_rooms: Vec<Option<RoomID>>,
	#[new(value="UserStateE::WaitingForUserJoined")]
	state: UserStateE,

	#[new(default)]
	ratelimits: GlobalRatelimits,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum UserStateE {
	WaitingForUserJoined,
	Ok
}

#[derive(new, Debug)]
struct RoomState {
	id: RoomID,
	#[new(default)]
	users: RingMap<UserID, RoomMemberState>,
	#[new(default)]
	messages: RingMap<MessageID, HistEntry>,
	#[new(default)]
	empty_timer: u16,
}

#[derive(new, Debug)]
struct RoomMemberState {
	#[new(default)]
	typing_timer: u16,
	#[new(default)]
	ratelimits: RoomRatelimits,
	// maybe permissions are next
}

impl RoomState {
	fn get_typing(&self) -> Vec<UserID> {
		self.users.iter().filter_map(|(k, v)| if v.typing_timer != 0 { Some(*k) } else { None }).collect()
	}
	fn get_users(&self, users: &HashMap<UserID, UserState>) -> Vec<User> {
		self.users.keys().map(|e| { users.get(e).unwrap().get_obj() }).collect()
	}
	fn get_history(&self) -> Vec<HistEntry> {
		self.messages.values().filter(|e| e.message.sent_to.is_none()).cloned().collect()
	}
}


impl UserState {
	fn get_obj(&self) -> User {
		User {
			nick: self.nick.clone(),
			color: self.color,
			home: self.addr.into(),
			sid: self.sid
		}
	}
}



macro_rules! ratelimit_check {
	// check global rate limit
	($s:ident, $sid:ident; $name:ident) => {
		let user = $s.users.get_mut(&$sid).expect("user should never be undefined");
		let check = concat_idents!(_fn_name = check_add_, $name {
			user.ratelimits._fn_name(&$s.config.ratelimits, $s.ratelimit_idx)
		});
		if ! check {
			return Some((ResponseCodes::RateLimitExceeded, None, None));
		}
	};
	// check per-room rate limit
	($s:ident, $sid:ident, $room:ident; $name:ident) => {
		let user = $room.users.get_mut(&$sid).expect("user should never be undefined");
		let check = concat_idents!(_fn_name = check_add_, $name {
			user.ratelimits._fn_name(&$s.config.ratelimits, $s.ratelimit_idx)
		});
		if ! check {
			return Some((ResponseCodes::RateLimitExceeded, None, None));
		}
	};
}

/// validates the passed room handle, either returning a mutable user and room borrow (or the cloned RoomID if + is present)
macro_rules! validate_rh {
	// this variant is probably unused becuase ratelimit checking uses the room variable
	($s:ident, $sid:ident, $rh:ident +) => { {
		let user = $s.users.get_mut(&$sid).expect("user should never be undefined");
		let rid = user.in_rooms.get(*$rh as usize).and_then(|e| e.as_ref());
		if rid.is_none() {
			return Some((ResponseCodes::BadRoomHandle, None, None));
		}
		rid.clone()
	} };
	($s:ident, $sid:ident, $rh:ident) => { {
		let user = $s.users.get_mut(&$sid).expect("user should never be undefined");
		let rid = user.in_rooms.get(*$rh as usize).and_then(|e| e.as_ref());
		let room = rid.as_ref().and_then(|e| $s.rooms.get_mut(e));
		if room.is_none() {
			return Some((ResponseCodes::BadRoomHandle, None, None));
		}
		let room = room.unwrap();
		(user, room)
	} };
}

impl Talker {
	pub async fn run(config: TalkerConfig, tx: Sender<TalkerTxOp>, mut rx: Receiver<TalkerRxOp>) -> Result<(), ()> {
		let mut s = Self {
			config, tx,
			users: HashMap::new(),
			rooms: HashMap::new(),
			ratelimit_idx: 0,
			msgid_counter: 0,
		};

		let mut timer = tokio::time::interval(Duration::from_secs(1));
		timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

		loop {
			select! {
				msg = rx.recv() => {
					if let None = msg { break }
					let msg = msg.unwrap();
					if let TalkerRxOp::Shutdown = msg { break }
					s.on_message(msg).await;
				}
				_ = timer.tick() => {
					s.every_second().await;
				}
			}
		}

		info!("talker thread exiting");
		Ok(())
	}
	async fn on_message(&mut self, msg: TalkerRxOp) {
		match msg {
			TalkerRxOp::Shutdown => return,
			TalkerRxOp::ReloadCfg(_new_config) => todo!("config reload"),
			TalkerRxOp::Connect(sid, addr) => self.on_connect(sid, addr).await,
			TalkerRxOp::Disconnect(sid) => self.on_disconnect(sid).await,
			TalkerRxOp::Request(sid, rid, data) => {
				let (code, reason, result) = match data {
					C2S::UserJoined(a) => self.req_user_joined (sid, a.0, a.1, a.2).await,
					C2S::UserChNick(a) => self.req_user_ch_nick(sid, a.0, a.1).await,
					C2S::RoomJoin(a) =>   self.req_room_join   (sid, a.0, a.1).await,
					C2S::RoomLeave(a) =>  self.req_room_leave  (sid, a.0).await,
					C2S::Message(a) =>    self.req_message     (sid, a.0, a.1).await,
					C2S::MessageDM(a) =>  self.req_message_dm  (sid, a.0, a.1, a.2).await,
					C2S::Typing(a) =>     self.req_typing      (sid, a.0, a.1).await,
					C2S::Mouse(a) =>      self.req_mouse       (sid, a.0, a.1, a.2).await,
					C2S::CustomR(a) =>    self.req_custom_r    (sid, a.0, a.1, a.2).await,
					C2S::CustomU(a) =>    self.req_custom_u    (sid, a.0, a.1, a.2, a.3).await,
				}.unwrap_or((ResponseCodes::Success, None, None));
				let reason = if let Some(r) = reason {
					r
				} else if code == ResponseCodes::Success {
					"".into()
				} else {
					format!("{}", code)
				};
				let response_msg = S2C::Response(S2CResponse(rid, code, reason, result));
				self.send_uni(sid, response_msg).await;
			}
		}
	}

	async fn send_to_room(&mut self, rid: &RoomID, msg: S2CBroad) {
		//info!("room {:?} broad msg {:?}", **rid, &msg);
		// what is this..?
		// doing this because borrowck is a silly goober
		let msgs: Vec<(UserID, S2C)> = {
			let room = self.rooms.get_mut(rid).expect("the room should exist");
			let mut vec = Vec::with_capacity(room.users.len());
			//info!("broadcasting to {:?}", room.users);
			for sid in room.users.keys() {
				let user = self.users.get(sid).unwrap();
				let rh = room_get_rh_by_id(&user.in_rooms, rid)
					.expect("users in the room should have the room in their state");
				vec.push((*sid, msg.to_s2c(rh)));
			}
			vec
		};
		join_all(msgs.into_iter().map(|(sid, msg)| self.send_uni(sid, msg))).await;
	}
	async fn send_uni(&self, sid: UserID, msg: S2C) {
		// who cares if it fails amirite
		let _ = self.tx.send(TalkerTxOp::Event(sid, msg)).await.expect("sending message to main thread shouldn't fail");
	}

	/// sends message to a room and saves it in the room's history
	async fn send_message(&mut self, rid: &RoomID, msg: TextMessage) -> ResponseCodes {
		if let Some(target) = msg.sent_to {
			let target_rh = self.users.get_mut(&target).and_then(|e| room_get_rh_by_id(&e.in_rooms, rid));
			let target_rh = match target_rh { Some(s) => s, None => return ResponseCodes::UnknownUserId };
			self.send_uni(target, S2C::MessageDM(S2CMessageDM(target_rh, msg.clone()))).await;
			if target != msg.sid && msg.sid != UserID::SYSTEM {
				let source_rh = room_get_rh_by_id(&self.users.get_mut(&msg.sid).unwrap().in_rooms, rid).expect("user sending the DM should be in the room");
				self.send_uni(msg.sid, S2C::MessageDM(S2CMessageDM(source_rh, msg.clone()))).await;
			}
		} else {
			self.send_to_room(rid, S2CBroad::Message(msg.clone())).await;
		}

		let room = self.rooms.get_mut(rid).unwrap();
		let user_from = self.users.get_mut(&msg.sid).and_then(|e| Some(e.get_obj()));
		let user_to = msg.sent_to.and_then(|e| self.users.get_mut(&e)).and_then(|e| Some(e.get_obj()));

		if room.messages.len() >= self.config.history_max.into() {
			let _ = room.messages.pop_front();
		}
		room.messages.push_back(msg.id, HistEntry { message: msg, from: user_from, to: user_to });

		ResponseCodes::Success
	}


	fn ensure_unique_nick(&mut self, nick: UserNick) -> UserNick {
		// TODO: actually ensure the thing that the function name implies
		// but nothing breaks because of not-unique nicks so whatever
		nick
	}

	fn ensure_room(&mut self, rid: RoomID) {
		if let Some(room) = self.rooms.get_mut(&rid) {
			room.empty_timer = self.config.room_empty_timer;
			return
		}
		info!("creating room {:?}", &*rid);
		let mut room = RoomState::new(rid.clone());
		room.empty_timer = self.config.room_empty_timer;
		self.rooms.insert(rid, room);
	}

	async fn user_room_join(&mut self, sid: UserID, rid: &RoomID) -> Result<RoomHandle, ResponseCodes> {
		let (rh, data) = self.user_room_join_unimmediate(sid, rid).await?;
		let in_rooms = self.users.get_mut(&sid).unwrap().in_rooms.clone();
		self.send_uni(sid, S2C::Room(S2CRoom(in_rooms, ()))).await;
		self.user_room_join_unimmediate_complete(data).await;
		Ok(rh)
	}
	/// user_room_join_unimmediate_complete should be called after sending a ROOM message to the user :P
	/// server state should not be modified between calls to this and the complete function
	async fn user_room_join_unimmediate(&mut self, sid: UserID, rid: &RoomID)
		-> Result<(RoomHandle, UserRoomJoinCompletionData), ResponseCodes>  {
		self.ensure_room(rid.clone());
		info!("user  sid={}  joining room {:?}", sid, **rid);
		let (uobj, rh): (User, RoomHandle) = {
			let user = self.users.get_mut(&sid).expect("user should never be undefined");
			let len = user.in_rooms.len();
			if user.in_rooms.iter().any(|e| e.as_ref().is_some_and(|e| { e==rid })) {
				info!("...but they were already in it");
				return Err(ResponseCodes::RoomAlreadyJoined);
			}
			let rh = if let Some(rh) = room_get_free_rh(&user.in_rooms) {
				user.in_rooms[*rh as usize] = Some(rid.clone());
				rh
			} else if len >= (u8::MAX as usize) { // roomhandle type is u8
				info!("...but they were in too many rooms");
				return Err(ResponseCodes::TooManyRooms);
			} else {
 				user.in_rooms.push(Some(rid.clone()));
				RoomHandle::new(len as u8)
			};
			(user.get_obj(), rh)
		};
		let (uobjs, hist) = {
			let room = self.rooms.get_mut(&rid).expect("we ensured that it exists");
			room.users.insert(sid, RoomMemberState::new());
			( room.get_users(&self.users), room.get_history() )
		};
		let rid = rid.clone();
		let mid = MessageID::new(self.msgid_counter);
		self.msgid_counter = self.msgid_counter.wrapping_add(1);
		Ok((rh, UserRoomJoinCompletionData {
			uobjs, uobj, hist, rid, mid, rh
		}))
		//Ok((rh, Box::new(|s| {
		//	s.send_uni(S2C::History(S2CHistory(rh, hist)))
		//		// i would also send S2CBroad::UserJoined(uobj.clone()) but that's redundant
		//		.map(|_| s.send_to_room(&rid, S2CBroad::UserUpdate(uobjs)))
		//		.map(|_| s.send_message(&rid, TextMessage {
		//			content: TextMessageType::UserJoined(uobj), sent_to: None, sid, id, time: timestamp() }))
		//		.boxed()
		//})))
	}

	async fn user_room_join_unimmediate_complete(&mut self, data: UserRoomJoinCompletionData) {
		let sid = data.uobj.sid;
		self.send_uni(sid, S2C::History(S2CHistory(data.rh, data.hist))).await;
		self.send_to_room(&data.rid, S2CBroad::UserJoined(data.uobj.clone())).await;
		self.send_uni(sid, S2C::UserUpdate(S2CUserUpdate(data.rh, data.uobjs))).await;
		self.send_message(&data.rid, TextMessage {
			content: TextMessageType::UserJoined(data.uobj), sent_to: None, sid: UserID::SYSTEM, id: data.mid, time: timestamp()
		}).await;
	}

	async fn user_room_leave(&mut self, sid: UserID, rid: &RoomID) {
		info!("user  sid={}  leaving room {:?}", sid, **rid);
		let (uobj, uobjs) = {
			let room = self.rooms.get_mut(&rid).expect("user tried to leave non-existent room?");
			room.users.remove(&sid);
			let user = self.users.get_mut(&sid).expect("user shouldn't be undefined");
			for rid2 in &mut user.in_rooms {
				if rid2.as_ref().is_some_and(|val| val == rid) {
					*rid2 = None;
				}
			}
			// removes the None elements at the end
			if let Some(pos) = user.in_rooms.iter().rposition(|x| x.is_some()) {
				user.in_rooms.truncate(pos+1);
			} else {
				// all of them are None so we empty it
				user.in_rooms.clear();
			}
			(user.get_obj(), room.get_users(&self.users))
		};
		let id = MessageID::new(self.msgid_counter);
		self.msgid_counter = self.msgid_counter.wrapping_add(1);
		self.send_message(rid, TextMessage {
			content: TextMessageType::UserLeft(uobj), sent_to: None,
			sid: UserID::SYSTEM, id, time: timestamp() }).await;
		self.send_to_room(rid, S2CBroad::UserLeft(sid)).await;
		// redundant
		//self.send_to_room(rid, S2CBroad::UserUpdate(uobjs)).await;
	}

	async fn on_connect(&mut self, sid: UserID, addr: RemoteAddress) {
		info!("user  sid={}  connected", sid);
		self.send_uni(sid, S2C::Hello(S2CHello("".into(), sid))).await;
		self.send_uni(sid, S2C::RateLimits(S2CRateLimits(self.config.ratelimits.clone(), ()))).await;
		
		self.users.insert(sid, UserState::new(sid, addr));
	}
	async fn on_disconnect(&mut self, sid: UserID) {
		info!("user  sid={}  disconnected", sid);
		let user = self.users.remove(&sid).expect("user shouldn't be undefined");
		let uobj = user.get_obj();
		// TODO: this may be replaced with calls to user_room_leave probably
		for rid in user.in_rooms {
			if let Some(rid) = rid {
				let uobjs = {
					let room = self.rooms.get_mut(&rid).expect("user was in non-existent room?");
					room.users.remove(&sid);
					room.get_users(&self.users)
				};
				let id = MessageID::new(self.msgid_counter);
				self.msgid_counter = self.msgid_counter.wrapping_add(1);
				self.send_message(&rid, TextMessage {
					content: TextMessageType::UserLeft(uobj.clone()), sent_to: None,
					sid: UserID::SYSTEM, id, time: timestamp() }).await;
				self.send_to_room(&rid, S2CBroad::UserLeft(sid)).await;
				// redundant
				//self.send_to_room(&rid, S2CBroad::UserUpdate(uobjs)).await;
			}
		}
	}

	async fn every_second(&mut self) {
		self.ratelimit_idx = (self.ratelimit_idx + 1) % RATELIMIT_WINDOW;
		for user in self.users.values_mut() {
			user.ratelimits.reset_idx(self.ratelimit_idx);
		}
		let mut rooms_to_remove = Vec::new();
		let mut rooms_to_update_typing = Vec::new();
		for room in self.rooms.values_mut() {
			if room.users.len() == 0 {
				room.empty_timer = room.empty_timer.saturating_sub(1);
				if room.empty_timer == 0 {
					info!("dropping room {:?}", *room.id);
					rooms_to_remove.push(room.id.clone());
				}
				continue
			}
			let mut do_typing_update = false;
			for member in room.users.values_mut() {
				member.ratelimits.reset_idx(self.ratelimit_idx);

				if member.typing_timer != 0 {
					member.typing_timer -= 1;
					if member.typing_timer == 0 { do_typing_update = true }
				}
			}
			if do_typing_update {
				rooms_to_update_typing.push((room.id.clone(), room.get_typing()));
			}
		}
		for rid in &rooms_to_remove {
			self.rooms.remove(rid);
		}
		for (rid, typing) in rooms_to_update_typing {
			self.send_to_room(&rid, S2CBroad::Typing(typing)).await;
		}
		//info!("second.. {}", self.ratelimit_idx);
		//if self.ratelimit_idx >= RATELIMIT_WINDOW {
		//	self.ratelimit_idx = 0;
		//}
	}

	async fn req_user_joined (&mut self, sid: UserID, nick: UserNick, color: UserColor, mut init_rooms: Vec<RoomID>) -> Response {
		init_rooms.dedup();
		let uobj = {
			let nick = self.ensure_unique_nick(nick);
			let user = self.users.get_mut(&sid).expect("user should never be undefined");
			if user.state == UserStateE::Ok {
				return Some((ResponseCodes::Failure, Some("you already sent a user_joined message".into()), None));
			}
			info!("user  sid={}  joined addr={} nick={:?} color={}", sid, user.addr, *nick, color);
			user.nick = nick;
			user.color = color;
			user.state = UserStateE::Ok;
			user.get_obj()
		};
		if init_rooms.len() > u8::MAX as usize {
			return Some((ResponseCodes::TooManyRooms, None, None));
		}
		let mut msgs = Vec::with_capacity(init_rooms.len());
		for a in init_rooms.iter() {
			// no concurrency here
			let (_, data) = self.user_room_join_unimmediate(sid, a).await.unwrap();
			msgs.push(data);
		}
		let in_rooms = self.users.get_mut(&sid).unwrap().in_rooms.clone();
		self.send_uni(sid, S2C::Room(S2CRoom(in_rooms.clone(), ()))).await;
		for data in msgs {
			self.user_room_join_unimmediate_complete(data).await;
		}
		Some((ResponseCodes::Success, None, Some(ResponseTypes::UserJoined(uobj))))
	}
	async fn req_user_ch_nick(&mut self, sid: UserID, nick: UserNick, color: UserColor) -> Response {
		ratelimit_check!(self, sid; chnick);

		info!("user  sid={}  renamed nick={:?} color={}", sid, *nick, color);
		let user = self.users.get_mut(&sid).expect("user should never be undefined");

		if user.nick == nick && user.color == color {
			return None
		}

		let prev_uobj = user.get_obj();

		user.nick = nick.clone();
		user.color = color;

		let uobj = user.get_obj();

		let rooms = user.in_rooms.clone();
		for rid in rooms.iter().flatten() {
			let uobjs = self.rooms.get(rid).unwrap().get_users(&self.users);
			self.send_to_room(rid, S2CBroad::UserChNick(sid, nick.clone(), color)).await;
			let id = MessageID::new(self.msgid_counter);
			self.msgid_counter = self.msgid_counter.wrapping_add(1);
			self.send_message(rid, TextMessage {
				content: TextMessageType::UserChNick(prev_uobj.clone(), uobj.clone()),
				time: timestamp(), sent_to: None, sid: UserID::SYSTEM, id
			}).await;
			// redundant
			//self.send_to_room(rid, S2CBroad::UserUpdate(uobjs)).await;
		}

		Some((ResponseCodes::Success, None, Some(ResponseTypes::UserChNick(uobj))))
	}
	async fn req_room_join   (&mut self, sid: UserID, rid: RoomID, exclusive: bool) -> Response {
		ratelimit_check!(self, sid; room);

		if exclusive {
			let user = self.users.get_mut(&sid).expect("user should never be undefined");
			let mut should_join = true;
			let mut rooms_to_leave = Vec::with_capacity(user.in_rooms.len());
			for i in &user.in_rooms {
				if let Some(rid2) = i {
					if rid2 == &rid {
						should_join = false;
					} else {
						rooms_to_leave.push(rid2.clone());
					}
				}
			}
			// the compiler expects us to not have a mutable ref to self here
			for i in &rooms_to_leave {
				self.user_room_leave(sid, i).await;
			}
			// so we get a new one here to not cause an E0499
			if should_join {
				let user = self.users.get_mut(&sid).expect("user should never be undefined");
				user.in_rooms.clear();
			}
		}
		match self.user_room_join(sid, &rid).await {
			Ok(rh) => {
				//let in_rooms = self.users.get_mut(&sid).unwrap().in_rooms.clone();
				//self.send_uni(sid, S2C::Room(S2CRoom(in_rooms, ()))).await;
				Some((ResponseCodes::Success, None, Some(ResponseTypes::RoomJoin(rh))))
			}
			Err(code) => Some((code, None, None))
		}
	}
	async fn req_room_leave  (&mut self, sid: UserID, rh: RoomHandle) -> Response {
		let (_user, room) = validate_rh!(self, sid, rh);
		ratelimit_check!(self, sid; room);
		// again, borrowck is a silly goober so we do this
		let rid = room.id.clone();
		self.user_room_leave(sid, &rid).await;
		let in_rooms = self.users.get_mut(&sid).unwrap().in_rooms.clone();
		self.send_uni(sid, S2C::Room(S2CRoom(in_rooms, ()))).await;
		None
	}
	async fn req_message     (&mut self, sid: UserID, rh: RoomHandle, content: String) -> Response {
		// TODO: no checks on message content for now
		let (_user, room) = validate_rh!(self, sid, rh);
		ratelimit_check!(self, sid, room; message);
		let rid = room.id.clone();
		//info!("user  sid={} rid={:?}  message {:?}", sid, *rid, &content);
		let id = MessageID::new(self.msgid_counter);
		self.msgid_counter = self.msgid_counter.wrapping_add(1);
		let code = self.send_message(&rid, TextMessage {
			time: timestamp(),
			content: TextMessageType::Message(content),
			sid, id, sent_to: None
		}).await;
		Some((code, None, None))
	}
	async fn req_message_dm  (&mut self, sid: UserID, rh: RoomHandle, content: String, target: UserID) -> Response {
		// TODO: no checks on message content for now
		let (_user, room) = validate_rh!(self, sid, rh);
		ratelimit_check!(self, sid, room; message_dm);
		let id = MessageID::new(self.msgid_counter);
		self.msgid_counter = self.msgid_counter.wrapping_add(1);
		let rid = room.id.clone();
		//info!("user  sid={} rid={:?}  message_dm target={} {:?}", sid, *room.id, target, &content);
		let code = self.send_message(&rid, TextMessage {
			time: timestamp(),
			content: TextMessageType::Message(content),
			sid, id, sent_to: Some(target),
		}).await;
		Some((code, None, None))
	}
	async fn req_typing      (&mut self, sid: UserID, rh: RoomHandle, is_typing: bool) -> Response {
		// TODO: typing time out
		let (_user, room) = validate_rh!(self, sid, rh);
		ratelimit_check!(self, sid, room; typing);
		let prev_timer = {
			let ustate = room.users.get_mut(&sid).expect("user should never be undefined");
			let timer = ustate.typing_timer;
			ustate.typing_timer = if is_typing { self.config.room_typing_timer } else { 0 };
			timer
		};
		if (prev_timer != 0) != is_typing {
			let rid = room.id.clone();
			let typing = room.get_typing();
			self.send_to_room(&rid, S2CBroad::Typing(typing)).await;
		}
		None
	}
	async fn req_mouse       (&mut self, sid: UserID, rh: RoomHandle, x: f32, y: f32) -> Response {
		let (_user, room) = validate_rh!(self, sid, rh);
		ratelimit_check!(self, sid, room; mouse);
		let rid = room.id.clone();
		self.send_to_room(&rid, S2CBroad::Mouse(sid, x, y)).await;
		None
	}
	async fn req_custom_r    (&mut self, sid: UserID, rh: RoomHandle, mtype: String, data: UserCustomData) -> Response {
		// TODO: no checks on message content for now
		let (_user, room) = validate_rh!(self, sid, rh);
		//ratelimit_check!(self, sid, room; message);
		let rid = room.id.clone();
		self.send_to_room(&rid, S2CBroad::CustomR(sid, mtype, data)).await;
		None
	}
	async fn req_custom_u    (&mut self, sid: UserID, rh: RoomHandle, target: UserID, mtype: String, data: UserCustomData) -> Response {
		let (_user, room) = validate_rh!(self, sid, rh);
		//ratelimit_check!(self, sid, room; message);

		if sid == target {
			return Some((ResponseCodes::Failure, Some("please don't send custom messages to yourself...".into()), None))
		}
		if let Some(tuser) = self.users.get(&target) {
			if let Some(trh) = tuser.in_rooms.iter().position(|e| e.as_ref().is_some_and(|e| { e==&room.id })) {
				let trh = RoomHandle::new(trh as u8);
				self.send_uni(target, S2C::CustomU(S2CCustomU(trh, sid, mtype, data))).await;
				return None
			}
		}
		Some((ResponseCodes::UnknownUserId, None, None))
	}
}
// some utility functions
fn room_get_rh_by_id(rooms: &Vec<Option<RoomID>>, rid: &RoomID) -> Option<RoomHandle> {
	Some(RoomHandle::new(rooms.iter().position(|r| {
		if let Some(r) = r { r == rid }
		else { false }
	})? as u8))
}
// None = one should append instead
fn room_get_free_rh(rooms: &Vec<Option<RoomID>>) -> Option<RoomHandle> {
	Some(RoomHandle::new(rooms.iter().position(|r| r.is_none())? as u8))
}

fn timestamp() -> u64 {
	let a = SystemTime::now();
	// this would fail for dates before January 1, 1970 and after ...year 584942417?
	let b = a.duration_since(UNIX_EPOCH).expect("unexpected rtc time..?");
	return b.as_millis().try_into().expect("unexpected rtc time..?");
}


/*

type SusMap = HashMap<UserID, Susser>;
type SusRoom = HashMap<RoomID, Room>;
type ResumptionData = HashMap<String, ConnState>;


async fn _main() {
	//let bind_to = env::var("BIND").unwrap_or("127.0.0.1:8000".into()).parse::<SocketAddr>().expect("Invalid socket address");
	let args = Args::parse();
	let listener = TcpListener::bind(&args.bind).await.expect("Failed to bind socket");
	println!("Listening on {}...", &args.bind);
	setsockopt(listener.as_raw_fd(), sockopt::ReuseAddr, &true).ok();

	let (tx_msg, mut messages) = channel(300);
	let mut ducks = SusMap::new();
	let mut rooms = SusRoom::new();
	let sessions: Arc<Mutex<_>> = Arc::new(Mutex::new(ResumptionData::new()));
	let jh1 = spawn(listen(listener, tx_msg.clone(), sessions.clone()));
	let jh2 = spawn(timer(tx_msg.clone()));
	while let Some(i) = messages.recv().await {
		match i {
			ClientOp::Connection(uid, balls, rid) => {
				println!("\x1b[32mconn+ \x1b[34m[{}|{:?}]\x1b[0m", uid, balls.ip);
				if balls.tx.send(ServerOp::MsgHello(S2CHello::new(rid, uid))).await.is_err() { break }
				ducks.insert(uid, balls);
			},
			ClientOp::Resume(uid) => {
				println!("\x1b[36mconn~ \x1b[34m[{}]\x1b[0m", uid);
				let mf = ducks.get_mut(&uid).expect("nope");
				send_uni(mf, ServerOp::MsgRoom(S2CRoom::new(mf.rooms.clone(), ()))).await;
			},
			ClientOp::Disconnect(uid) => {
			    println!("\x1b[31mconn- \x1b[34m[{}]\x1b[0m", uid);
				leave_room_all_duck(uid, &mut ducks, &mut rooms).await;
				ducks.remove(&uid);
			},
			ClientOp::MsgUserJoined(uid, duck) => {
				let n = ensure_unique_nick(&ducks, duck.0);
				let mf = ducks.get_mut(&uid).expect("nope");
				mf.u.nick = n;
				mf.u.color = duck.1;
				send_uni(mf, ServerOp::MsgRateLimits(S2CRateLimits::new(MAX_EVENTS.clone(), ()))).await;
				if duck.2.len() > MAX_ROOMS_PER_CLIENT as usize { kill_uni(mf, "too many rooms joined".into()).await; continue }
				send_uni(mf, ServerOp::MsgRoom(S2CRoom::new(duck.2.clone(), ()))).await;
				for a in duck.2 { join_room(uid, a, &mut ducks, &mut rooms, false, false).await; }
			},
			ClientOp::MsgMouse(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				if ratelimit_check!(mf mouse[*duck.0 as usize]) { continue }
				send_broad(rf, SBroadOp::MsgMouse(uid, duck.1, duck.2), &ducks).await
			},
			ClientOp::MsgTyping(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				if ratelimit_check!(mf typing[*duck.0 as usize]) { continue }
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
				if ratelimit_check!(mf room) { continue }
				if !duck.1 && mf.rooms.len() as u8 >= MAX_ROOMS_PER_CLIENT { kill_uni(mf, "too many rooms joined".into()).await; continue }
				join_room(uid, duck.0.clone(), &mut ducks, &mut rooms, duck.1, true).await;
			},
			ClientOp::MsgRoomLeave(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if ratelimit_check!(mf room) { continue }
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue; }
				leave_room(uid, duck.0, &mut ducks, &mut rooms).await;
			},
			ClientOp::MsgMessage(uid, duck) => {
				// TODO: validate
				let mf = ducks.get_mut(&uid).expect("nope");
				if ratelimit_check!(mf message[*duck.0 as usize]) { continue }
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue; }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				send_broad(rf, SBroadOp::MsgMessage(TextMessage {
					time: timestamp(),
					sid: uid,
					content: duck.1
				}), &ducks).await;
			},
			ClientOp::MsgMessageDM(uid, duck) => {
				// TODO: validate
				let mf = ducks.get_mut(&uid).expect("nope");
				if ratelimit_check!(mf message_dm[*duck.0 as usize]) { continue }
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue; }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				let a = rf.users.iter().find(|&&x| x==duck.2);
				if a.is_none() { kill_uni(mf, "invalid user id".into()).await; continue; }
				let a = a.unwrap();
				let mf = ducks.get(&uid).expect("nope");
				let tf = ducks.get(&a).expect("nope??");
				let rh = to_room_handle(&tf.rooms, &rf.id).unwrap();
				let b = timestamp();
				send_uni(mf, ServerOp::MsgMessageDM(S2CMessageDM(duck.0, TextMessageDM { time: b, sid: uid, sent_to: *a, content: duck.1.clone() }))).await;
				// if user is not dming themselves for some reason
				if &uid != a {
					send_uni(tf, ServerOp::MsgMessageDM(S2CMessageDM(rh, TextMessageDM { time: b, sid: uid, sent_to: *a, content: duck.1 }))).await;
				}
			},
			ClientOp::MsgUserChNick(uid, duck) => {
				let nick: UserNick;
				let color: UserColor;
				let n = ensure_unique_nick(&ducks, duck.0.clone());
				let mf = ducks.get_mut(&uid).expect("nope");
				if ratelimit_check!(mf chnick) { continue }
				nick = mf.u.nick.clone();
				color = mf.u.color;
				mf.u.nick = n;
				mf.u.color = duck.1;
				let msg = SBroadOp::MsgUserChNick(uid, (nick, color), (duck.0, duck.1), timestamp());
				for ri in mf.rooms.clone() {
					let rf = rooms.get_mut(&ri).unwrap();
					send_broad(rf, msg.clone(), &ducks).await;
					send_broad(rf, SBroadOp::MsgUserUpdate(
						rf.users.iter().map(|p| ducks.get(p).unwrap().u.clone()).collect()
					), &ducks).await;
				}
			},
			ClientOp::MsgCustomR(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue; }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				// hardcoded byte limit
				if duck.1.len() > 128 { kill_uni(mf, "data type too long (>128)".into()).await; continue; }
				if duck.2.len() > 16384 { kill_uni(mf, "data too long (>16kb)".into()).await; continue; }
				let msg = SBroadOp::MsgCustomR(uid, duck.1, duck.2);
				send_broad(rf, msg, &ducks).await;
			},
			ClientOp::MsgCustomU(uid, duck) => {
				let mf = ducks.get_mut(&uid).expect("nope");
				if *duck.0 as usize >= mf.rooms.len() { kill_uni(mf, "invalid room handle".into()).await; continue; }
				let rf = rooms.get_mut(&mf.rooms[*duck.0 as usize]).expect("no way");
				// hardcoded byte limit
				if duck.2.len() > 128 { kill_uni(mf, "data type too long (>128)".into()).await; continue; }
				if duck.3.len() > 65536 { kill_uni(mf, "data too long (>64kb)".into()).await; continue; }

				let a = rf.users.iter().find(|&&x| x==duck.1);
				if a.is_none() { kill_uni(mf, "invalid user id".into()).await; continue; }
				let a = a.unwrap();
				let tf = ducks.get(&a).expect("nope??");
				let rh = to_room_handle(&tf.rooms, &rf.id).unwrap();
				send_uni(tf, ServerOp::MsgCustomU(S2CCustomU(rh, uid, duck.2, duck.3))).await;
			},
			ClientOp::Duck(_) => {
				for (_, sus) in &mut ducks {
					sus.counter.room = 0;
					sus.counter.mouse.fill(0);
					sus.counter.chnick = 0;
					sus.counter.message.fill(0);
					sus.counter.message_dm.fill(0);
					sus.counter.typing.fill(0);
					sus.counter.events = 0;
				}
				let mut ss = sessions.lock().expect("please");
				let mut uids = Vec::new();
				ss.retain(|_, v| {
					v.disconnect_timer -= 1;
					if v.disconnect_timer <= 0 {
						uids.push(v.user_id);
						false
					} else { true }
				});
				for k in uids { let _ = tx_msg.send(ClientOp::Disconnect(k)).await; }
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
		b.counter.message.push(0);
		b.counter.message_dm.push(0);
		b.counter.typing.push(0);
		b.counter.mouse.push(0);
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
	duck.counter.message.swap_remove(*leaves as usize);
	duck.counter.message_dm.swap_remove(*leaves as usize);
	duck.counter.typing.swap_remove(*leaves as usize);
	duck.counter.mouse.swap_remove(*leaves as usize);
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

async fn kill_uni(to: &mut Susser, reason: String) {
	let _ = to.tx.send(ServerOp::UsageError(reason)).await;
	//let (mut t, _) = channel(1);
	//swap(&mut to.tx, &mut t);
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
			SBroadOp::MsgMouse(d,u,k) => ServerOp::MsgMouse(S2CMouse::new(rh, d,u,k)),
			SBroadOp::MsgCustomR(d,u,k) => ServerOp::MsgCustomR(S2CCustomR::new(rh,d,u,k))
		};
		send_uni(duck, b)
	})).await;
}

//fn ceil_char_boundary(&self: str, index: usize) -> usize {
//	if index > self.len() {
//		self.len()
//	} else {
//		let upper_bound = Ord::min(index + 4, self.len());
//		self.as_bytes()[index..upper_bound]
//			.iter()
//			.position(|b| b.is_utf8_char_boundary())
//			.map_or(upper_bound, |pos| pos + index)
//	}
//}

fn ensure_unique_nick(u: &SusMap, n: UserNick) -> UserNick {
	// TODO: make the code below work
	return n;

//	let mut x = 0u16;
//	let mut un = n.clone();
//'a:	loop {
//		for (_, a) in u {
//			if a.u.nick == un {
//				x += 1;
//				// this string can get too long
//				un = UserNick::new(format!("{n}{x}")).expect("this would never be invalid... right?");
//				continue 'a;
//			}
//		}
//		return un;
//	}
}

fn push_history(t: &mut Room, h: HistEntry) {
	if t.hist.len() == HIST_ENTRY_MAX-1 { t.hist.pop_front(); }
	t.hist.push_back(h);
}

fn timestamp() -> u64 {
	let a = SystemTime::now();
	// this would fail for dates before January 1, 1970 and after ...year 584942417?
	let b = a.duration_since(UNIX_EPOCH).expect("Time travel can't be supported");
	return b.as_millis().try_into().expect("ARE WE IN THE FUTURE??");
}

*/
