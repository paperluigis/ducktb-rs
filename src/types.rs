// protocol types and stuff

use concat_idents::concat_idents;
use derive_new::new;
use derive_more::{Display, Deref, From};
use nutype::nutype;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Error;
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use base64::engine::{DecodePaddingMode, Engine};
use base64::engine::general_purpose::{GeneralPurpose, GeneralPurposeConfig};
use base64::alphabet;

#[derive(Debug, Clone, Copy)]
pub enum RemoteAddress {
	IP(std::net::IpAddr),
	Unix(tokio::net::unix::UCred),
}

impl fmt::Display for RemoteAddress {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::IP(ip) => write!(f, "{}", ip),
			Self::Unix(ucred) => {
				write!(f, "(unix uid={} gid={}", ucred.uid(), ucred.gid())?;
				if let Some(pid) = ucred.pid() {
					write!(f, " pid={}", pid)?;
				}
				write!(f, ")")?;
				Ok(())
			}
		}
	}
}

impl From<std::net::IpAddr> for UserHome {
	fn from(inp: std::net::IpAddr) -> Self {
		RemoteAddress::IP(inp).into()
	}
}
impl From<RemoteAddress> for UserHome {
	fn from(inp: RemoteAddress) -> Self {
		Self(match inp {
			RemoteAddress::IP(IpAddr::V4(v4)) => {
				let oc = v4.octets();
				// who cares
				0x00000000ffff0000u64
					| (u64::from(oc[0]) << 56)
					| (u64::from(oc[1]) << 48)
					| (u64::from(oc[2]) << 40)
					| (u64::from(oc[3]) << 32)
			},
			RemoteAddress::IP(IpAddr::V6(v6)) => {
				let oc = v6.segments();
				0u64
					| (u64::from(oc[0]) <<  0)
					| (u64::from(oc[1]) << 16)
					| (u64::from(oc[2]) << 32)
					| (u64::from(oc[3]) << 48)
			}
			RemoteAddress::Unix(ucred) => {
				0x00000000fffe0000u64
					| (u64::from(ucred.gid()) << 48)
			}
		})
	}
}

const BASE64: GeneralPurpose = GeneralPurpose::new(&alphabet::URL_SAFE, GeneralPurposeConfig::new()
	.with_encode_padding(false)
	.with_decode_padding_mode(DecodePaddingMode::Indifferent));

#[derive(new, Hash, Eq, Clone, Copy, PartialEq, Debug, Deref, Serialize, Deserialize)]
pub struct RequestID(u32);

#[nutype(default="_" sanitize(trim) validate(max_len=80, not_empty))]
#[derive(PartialEq, Clone, Debug, Display, Deref, Serialize, Deserialize, TryFrom, Default)]
pub struct UserNick(String);
/// format is 0rgb
#[nutype(default=0 sanitize(with=|r| r&0xffffffu32))]
#[derive(Clone, Copy, PartialEq, Debug, Deref, TryFrom, Default)]
pub struct UserColor(u32);
#[derive(new, Hash, Eq, Clone, Copy, PartialEq, Debug, Deref)]
pub struct UserID(u32);
impl UserID {
	// user that sends system messages, serializes as "system"
	pub const SYSTEM: UserID = UserID(0xffffffff);
}
#[derive(new, Hash, Eq, Clone, Copy, PartialEq, Debug, Deref)]
pub struct MessageID(u32);
#[derive(new, Hash, Eq, Clone, Copy, PartialEq, PartialOrd, Debug, Deref, Serialize, Deserialize)]
pub struct RoomHandle(u8);
#[nutype(sanitize(trim) validate(max_len=80, not_empty))]
#[derive(Clone, Hash, Eq, PartialEq, Debug, Display, Deref, Serialize, Deserialize, TryFrom)]
pub struct RoomID(String);
#[derive(Clone, Copy, PartialEq, Debug, Deref, From)]
pub struct UserHome(u64);
#[derive(Clone, Debug)]
pub enum UserCustomData {
	Data(Vec<u8>),
	/// can safely be ignored outside of protocol handlers
	Placeholder,
}

impl Serialize for UserID {
	fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S: Serializer {
		if s.is_human_readable() {
			s.serialize_str(&self.to_string())
		} else {
			s.serialize_u32(self.0)
		}
	}
}
impl Serialize for MessageID {
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
			s.serialize_u32(**self)
		}
	}
}
impl Serialize for UserHome {
	fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S: Serializer {
		if s.is_human_readable() {
			s.serialize_str(&self.to_string())
		} else {
			s.serialize_u64(self.0)
		}
	}
}
impl Serialize for UserCustomData {
	fn serialize<S>(&self, s: S) -> Result<S::Ok, S::Error> where S: Serializer {
		match self {
			Self::Placeholder => s.serialize_none(),
			Self::Data(bytes) => {
				if s.is_human_readable() {
					s.serialize_str(&BASE64.encode(&bytes))
				} else {
					s.serialize_bytes(&bytes)
				}
			}
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
impl<'de> Deserialize<'de> for MessageID {
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
		} else {
			let b = u32::deserialize(d)?;
			Ok(Self::new(b))
		}
	}
}
impl<'de> Deserialize<'de> for UserCustomData {
	fn deserialize<D>(d: D) -> Result<Self, D::Error> where D: Deserializer<'de> {
		if d.is_human_readable() {
			let s = Option::<String>::deserialize(d)?;
			if let Some(s) = s {
				Ok(Self::Data(BASE64.decode(&s).map_err(D::Error::custom)?))
			} else {
				Ok(Self::Placeholder)
			}
		} else {
			let b = Option::<Vec<u8>>::deserialize(d)?;
			if let Some(b) = b  {
				Ok(Self::Data(b))
			} else {
				Ok(Self::Placeholder)
			}
		}
	}
}
/*impl Deserialize<'de> for UserHome {
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
		write!(f, "#{:0>6x}", **self)
	}
}
impl fmt::Display for UserID {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		if self == &UserID::SYSTEM {
			write!(f, "system")
		} else {
			write!(f, "{:0>8x}", self.0)
		}
	}
}
impl fmt::Display for MessageID {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:0>8x}", self.0)
	}
}
impl fmt::Display for UserHome {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{:0>16x}", self.0)
	}
}

pub struct SillyParsingError(pub String);
impl fmt::Display for SillyParsingError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.0)
	}
}

impl FromStr for UserID {
	type Err = SillyParsingError;
	fn from_str(inp: &str) -> Result<Self, Self::Err> {
		if inp == "system" {
			Ok(UserID::SYSTEM)
		} else if inp.len() == 8 {
			let val = u32::from_str_radix(&inp, 16);
			if let Ok(s) = val {
				Ok(UserID::new(s))
			} else {
				Err(SillyParsingError("expected 8 hex characters".into()))
			}
		} else {
			Err(SillyParsingError("expected 8 hex characters".into()))
		}
	}
}
impl FromStr for MessageID {
	type Err = SillyParsingError;
	fn from_str(inp: &str) -> Result<Self, Self::Err> {
		if inp.len() == 8 {
			let val = u32::from_str_radix(&inp, 16);
			if let Ok(s) = val {
				Ok(MessageID::new(s))
			} else {
				Err(SillyParsingError("expected 8 hex characters".into()))
			}
		} else {
			Err(SillyParsingError("expected 8 hex characters".into()))
		}
	}
}
impl FromStr for UserColor {
	type Err = SillyParsingError;
	fn from_str(inp: &str) -> Result<Self, Self::Err> {
		if inp.len() == 7 && inp.starts_with("#") {
			let val = u32::from_str_radix(&inp[1..], 16);
			if let Ok(s) = val {
				Ok(UserColor::new(s))
			} else {
				Err(SillyParsingError("expected hash symbol followed by 6 hex characters".into()))
			}
		} else {
			Err(SillyParsingError("expected hash symbol followed by 6 hex characters".into()))
		}
	}
}

#[derive(Debug, Clone, Serialize)]
pub struct HistEntry {
	pub message: TextMessage,
	// None if system user
	pub from: Option<User>,
	pub to: Option<User>
}

pub const RATELIMIT_WINDOW: usize = 5; // seconds

macro_rules! define_ratelimit_type {
	($name:ident; $($i:ident : $t:ty),*) => {
		#[derive(Debug, Default, new)]
		pub struct $name {
			$(			
			#[new(default)]
			pub $i: [$t; RATELIMIT_WINDOW],
			)*
		}

		impl $name {
			pub fn reset_idx(&mut self, idx: usize) {
				assert!(idx < RATELIMIT_WINDOW);
				// it's safe, right? we've already done bounds checking
				unsafe {
					$(
						*self.$i.get_unchecked_mut(idx) = 0;
					)*
				}
			}
			
			$( concat_idents!(_fn_name = check_add_, $i {
				pub fn _fn_name(&mut self, cfg: &RatelimitsAvg, idx: usize) -> bool {
					if self.$i.iter().sum::<$t>() > cfg.$i {
						false
					} else {
						self.$i[idx] += 1;
						true
					}
				}
			} ); )*
		}
	}
}

define_ratelimit_type!(GlobalRatelimits;
	chnick: u8,
	room: u8,
	events: u16);
define_ratelimit_type!(RoomRatelimits;
	mouse: u8,
	message: u8,
	message_dm: u8,
	typing: u8);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RatelimitsAvg {
	pub mouse: u8,
	pub chnick: u8,
	pub room: u8,
	pub message: u8,
	pub message_dm: u8,
	pub typing: u8,
	pub events: u16
}

/*
#[derive(Debug)]
pub struct UserState {
	pub counter: Ratelimits,
	pub ip: IpAddr,
	pub tx: Sender<ServerOp>,
	pub is_typing: Vec<bool>,
	pub rooms: Vec<RoomID>,
	pub u: User
}

#[derive(Debug)]
pub struct ConnState {
	pub user_id: UserID,
	pub rx: Receiver<ServerOp>,
	pub disconnect_timer: i32
}

impl UserState {
	pub fn new(id: UserID, ip: IpAddr, tx: Sender<ServerOp>) -> Self {
		UserState {
			counter: Ratelimits::new(),
			ip: ip,
			tx: tx,
			is_typing: vec![],
			rooms: vec![],
			u: User::new(id, ip.into())
		}
	}
}
*/


#[derive(new, Debug, Clone, Serialize)]
pub struct User {
	pub sid: UserID,
	#[new(default)]
	pub nick: UserNick,
	#[new(default)]
	pub color: UserColor,
	pub home: UserHome
}

#[derive(Debug, Clone, Serialize)]
pub struct TextMessage {
	pub time: u64,
	pub id: MessageID,
	pub sid: UserID,
	#[serde(flatten)]
	pub content: TextMessageType,
	#[serde(skip_serializing_if="Option::is_none")]
	pub sent_to: Option<UserID>,

}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="snake_case", tag="content_type", content="content")]
pub enum TextMessageType {
	Message(String),
	UserJoined(User),
	UserLeft(User),
	UserChNick(User, User)
}

// `, #[serde(skip)] pub ()` is a workaround to treat a newtype struct as a tuple with one element
// this is done for consistency with all other messages
#[derive(Debug, Clone, Deserialize)]
pub struct C2SUserJoined(pub UserNick, pub UserColor, pub Vec<RoomID>);
#[derive(Debug, Clone, Deserialize)]
pub struct C2SUserChNick(pub UserNick, pub UserColor);
// boolean for exclusive (leave all other rooms)
#[derive(Debug, Clone, Deserialize)]
pub struct C2SRoomJoin  (pub RoomID, pub bool);
#[derive(Debug, Clone, Deserialize)]
pub struct C2SRoomLeave (pub RoomHandle, #[serde(skip)] pub ());
#[derive(Debug, Clone, Deserialize)]
pub struct C2SMessage   (pub RoomHandle, pub String);
#[derive(Debug, Clone, Deserialize)]
pub struct C2SMessageDM (pub RoomHandle, pub String, pub UserID);
#[derive(Debug, Clone, Deserialize)]
pub struct C2STyping    (pub RoomHandle, pub bool);
#[derive(Debug, Clone, Deserialize)]
pub struct C2SMouse     (pub RoomHandle, pub f32, pub f32);
#[derive(Debug, Clone, Deserialize)]
pub struct C2SCustomR   (pub RoomHandle, pub String, pub UserCustomData);
#[derive(Debug, Clone, Deserialize)]
pub struct C2SCustomU   (pub RoomHandle, pub UserID, pub String, pub UserCustomData);

#[derive(Debug, Clone, Serialize, new)]
pub struct S2CHello     (pub String, pub UserID);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CRoom      (pub Vec<Option<RoomID>>, #[serde(skip)] pub ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CHistory   (pub RoomHandle, pub Vec<HistEntry>);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserJoined(pub RoomHandle, pub User);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserLeft  (pub RoomHandle, pub UserID);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserChNick(pub RoomHandle, pub UserID, pub UserNick, pub UserColor);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CMouse     (pub RoomHandle, pub UserID, pub f32, pub f32);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserUpdate(pub RoomHandle, pub Vec<User>);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CTyping    (pub RoomHandle, pub Vec<UserID>);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CMessage   (pub RoomHandle, pub TextMessage);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CMessageDM (pub RoomHandle, pub TextMessage);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CRateLimits(pub RatelimitsAvg, #[serde(skip)] pub ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CCustomR   (pub RoomHandle, pub UserID, pub String, pub UserCustomData);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CCustomU   (pub RoomHandle, pub UserID, pub String, pub UserCustomData);
// a request id of None will cause the connection to close
// with the provided string as the reason if the code isn't 0
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CResponse  (pub Option<RequestID>, pub ResponseCodes, pub String, pub Option<ResponseTypes>);


#[derive(Debug, Clone)]
pub enum C2S {
	UserJoined(C2SUserJoined),
	UserChNick(C2SUserChNick),
	RoomJoin(C2SRoomJoin),
	RoomLeave(C2SRoomLeave),
	Message(C2SMessage),
	MessageDM(C2SMessageDM),
	Typing(C2STyping),
	Mouse(C2SMouse),
	CustomR(C2SCustomR),
	CustomU(C2SCustomU),
}

#[derive(Debug, Clone, Serialize,)]
#[serde(untagged)]
pub enum ResponseTypes {
	UserJoined(User),
	UserChNick(User),
	RoomJoin(RoomHandle),
	//RoomLeave,
	//Message,
	//MessageDM,
	//Typing,
	//Mouse,
	//CustomR,
	//CustomU,
}

#[derive(serde_repr::Serialize_repr, Debug, Clone, Copy, PartialEq)]
#[repr(u8)]
pub enum ResponseCodes {
	Success = 0,
	Failure = 1,
	ParseError = 2,
	RateLimitExceeded = 3,
	TooManyRooms = 8,
	BadRoomHandle = 9,
	RoomAlreadyJoined = 10,
	UnknownUserId = 11,
	ServerError = 255,
}

impl fmt::Display for ResponseCodes {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", match self {
			Self::Success => "success",
			Self::Failure => "unknown failure",
			Self::ParseError => "parsing error",
			Self::RateLimitExceeded => "rate limit exceeded",
			Self::TooManyRooms => "joined too many rooms",
			Self::BadRoomHandle => "bad room handle",
			Self::RoomAlreadyJoined => "room already joined",
			Self::UnknownUserId => "unknown user id",
			Self::ServerError => "internal server error",
		})
	}
}

#[derive(Debug, Clone)]
pub enum S2C {
	Hello(S2CHello),
	Room(S2CRoom),
	History(S2CHistory),
	UserJoined(S2CUserJoined),
	UserLeft(S2CUserLeft),
	UserChNick(S2CUserChNick),
	Mouse(S2CMouse),
	UserUpdate(S2CUserUpdate),
	Typing(S2CTyping),
	Message(S2CMessage),
	MessageDM(S2CMessageDM),
	RateLimits(S2CRateLimits),
	CustomR(S2CCustomR),
	CustomU(S2CCustomU),
	Response(S2CResponse),
}

#[derive(Debug, Clone)]
pub enum S2CBroad {
	UserJoined(User),
	UserLeft(UserID),
	UserChNick(UserID, UserNick, UserColor),
	Mouse(UserID, f32, f32),
	Typing(Vec<UserID>),
	Message(TextMessage),
	UserUpdate(Vec<User>),
	CustomR(UserID, String, UserCustomData),
}

impl S2CBroad {
	pub fn to_s2c(&self, rh: RoomHandle) -> S2C {
		match self {
			S2CBroad::UserJoined(a0)         => S2C::UserJoined(S2CUserJoined(rh, a0.clone())),
			S2CBroad::UserLeft(a0)           => S2C::UserLeft(S2CUserLeft(rh, *a0)),
			S2CBroad::UserChNick(a0, a1, a2) => S2C::UserChNick(S2CUserChNick(rh, *a0, a1.clone(), a2.clone())),
			S2CBroad::Mouse(a0, a1, a2)      => S2C::Mouse(S2CMouse(rh, *a0, *a1, *a2)),
			S2CBroad::Typing(a0)             => S2C::Typing(S2CTyping(rh, a0.clone())),
			S2CBroad::Message(a0)            => S2C::Message(S2CMessage(rh, a0.clone())),
			S2CBroad::UserUpdate(a0)         => S2C::UserUpdate(S2CUserUpdate(rh, a0.clone())),
			S2CBroad::CustomR(a0, a1, a2)    => S2C::CustomR(S2CCustomR(rh, *a0, a1.clone(), a2.clone())),
		}
	}
}

/*
#[derive(Debug)]
pub enum C2SOp {
	// client connection states
	Connection(UserID, UserState, String),
	Resume(UserID),
	Disconnect(UserID),
	// client messages
	Message(UserID, C2S)
}

#[derive(Debug, Clone)]
pub enum S2COp {
	Result(RequestID, u8, String),
	Message(RequestID, C2S)
}

#[derive(Debug, Clone)]
pub enum S2CBroadOp {
	MsgUserJoined(User, u64),
	MsgUserLeft(UserID, u64),
	MsgUserChNick(UserID, (UserNick, UserColor), (UserNick, UserColor), u64),
	MsgMouse(UserID, f32, f32),
	MsgTyping(Vec<UserID>),
	MsgMessage(TextMessage),
	MsgUserUpdate(Vec<User>),
	MsgCustomR(UserID, String, UserCustomData),
}


// ========== utility types for functions ==========
pub fn sbroadop_to_histentry(c: SBroadOp, ducks: &HashMap<UserID, UserState>) -> Option<HistEntry> {
	match c {
		SBroadOp::MsgUserJoined(ref m, o) => Some(HistEntry::Join {
			ts: o,
			nick: m.nick.clone(),
			home: m.home,
			color: m.color,
			sid: m.id
		}),
		SBroadOp::MsgUserLeft(m, o) => { let mf = ducks.get(&m).unwrap(); Some(HistEntry::Leave {
			ts: o,
			nick: mf.u.nick.clone(),
			home: mf.u.home,
			color: mf.u.color,
			sid: mf.u.id
		}) },
		SBroadOp::MsgUserChNick(m, a, n, y) => { let mf = ducks.get(&m).unwrap(); Some(HistEntry::ChNick {
			ts: y,
			home: mf.u.home,
			sid: mf.u.id,
			old_nick: a.0.clone(),
			new_nick: n.0.clone(),
			old_color: a.1,
			new_color: n.1
		}) },
		SBroadOp::MsgMessage(m) => { let mf = ducks.get(&m.sid).unwrap(); Some(HistEntry::Message {
			ts: m.time,
			nick: mf.u.nick.clone(),
			home: mf.u.home,
			color: mf.u.color,
			sid: mf.u.id,
			content: m.content.clone()
		}) },
		_ => None
	}
}

pub fn to_room_handle(rv: &Vec<RoomID>, tf: &RoomID) -> Option<RoomHandle> {
	Some(RoomHandle(rv.iter().position(|r| r==tf)? as u8))
}
*/

/*
macro_rules! ratelimit_check {
	($i:ident $n:ident) => { {
		$i.counter.$n += 1;
		if $i.counter.$n > MAX_EVENTS.$n {
			let _ = $i.tx.send(ServerOp::UsageError(format!("exceeded rate limit: {}", stringify!($n)))).await;
			true
		} else { false } 
	} };
	($i:ident $n:ident [ $d:expr ]) => { {
		$i.counter.$n[$d] += 1;
		if $i.counter.$n[$d] > MAX_EVENTS.$n {
			let _ = $i.tx.send(ServerOp::UsageError(format!("exceeded rate limit: {}", stringify!($n)))).await;
			true
		} else { false }
	} };
*/
			//println!("whoops {} ran out of {} events", $i.u.id, stringify!($n));
			//$b
//}
