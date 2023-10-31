use derive_new::new;
use derive_more::{Display, Deref, From};
use nutype::nutype;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde::de::Error;
use std::collections::{HashMap, VecDeque};
use std::fmt;
use std::net::IpAddr;
use std::str::FromStr;
use tokio::sync::mpsc::Sender;

#[nutype(default="_" sanitize(trim) validate(max_len=40, not_empty))]
#[derive(PartialEq, Clone, Debug, Display, Deref, Serialize, Deserialize, TryFrom, Default)]
pub struct UserNick(String);
// format is 0rgb
#[nutype(default=0 sanitize(with=|r| r&0xffffffu32))]
#[derive(Clone, Copy, PartialEq, Debug, Deref, TryFrom, Default)]
pub struct UserColor(u32);
#[derive(new, Hash, Eq, Clone, Copy, PartialEq, Debug, Deref)]
pub struct UserID(u32);
#[nutype(sanitize(trim) validate(max_len=40, not_empty))]
#[derive(Clone, Hash, Eq, PartialEq, Debug, Display, Deref, Serialize, Deserialize, TryFrom)]
pub struct RoomID(String);
#[derive(Clone, Copy, PartialEq, Debug, Deref, From)]
pub struct UserHashedIP(u64);

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
			s.serialize_u32(**self)
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
		} else {
			let b = u32::deserialize(d)?;
			Ok(Self::new(b))
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
		write!(f, "#{:0>6x}", **self)
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

pub struct SillyParsingError;
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
			return Ok(UserColor::new(u32::from_str_radix(&inp[1..], 16).map_err(|_| SillyParsingError)?));
		} else {
			return Err(SillyParsingError);
		}
	}
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all="lowercase", tag="type")]
pub enum HistEntry {
	Message {
		home: UserHashedIP,
		sid: UserID,
		content: String,
		nick: UserNick,
		color: UserColor,
		ts: u64
	},
	Join {
		home: UserHashedIP,
		sid: UserID,
		nick: UserNick,
		color: UserColor,
		ts: u64
	},
	Leave {
		home: UserHashedIP,
		sid: UserID,
		nick: UserNick,
		color: UserColor,
		ts: u64
	},
	ChNick {
		home: UserHashedIP,
		sid: UserID,
		old_nick: UserNick,
		old_color: UserColor,
		new_nick: UserNick,
		new_color: UserColor,
		ts: u64
	}
}

#[derive(Debug, new)]
pub struct Room {
	#[new(default)]
	pub hist: VecDeque<HistEntry>,
	pub id: RoomID,
	#[new(default)]
	pub users: Vec<UserID>
}

#[derive(Debug, Clone, Serialize, new)]
pub struct SusRate {
	#[new(value="0")]
	pub mouse: u8,
	#[new(value="0")]
	pub chnick: u8,
	#[new(value="0")]
	pub room: u8,
	#[new(value="0")]
	pub message: u8,
	#[new(value="0")]
	pub typing: u8,
	#[new(value="0")]
	pub events: u16
}

#[derive(Debug)]
pub struct Susser {
	pub counter: SusRate,
	pub ip: IpAddr,
	pub tx: Sender<ServerOp>,
	pub is_typing: bool,
	pub rooms: Vec<RoomID>,
	pub u: User
}

#[derive(Debug, Clone, Serialize)]
pub struct User {
	#[serde(rename="sid")]
	pub id: UserID,
	pub nick: UserNick,
	pub color: UserColor,
	#[serde(rename="home")]
	pub haship: UserHashedIP
}

#[derive(Debug, Clone, Serialize)]
pub struct TextMessage {
	pub time: u64,
	pub sid: UserID,
	pub content: String
}

// `, #[serde(skip)] ()` is a workaround to treat a newtype struct as a tuple with one element
// this is done for consistency with all other messages
#[derive(Debug, Deserialize)]
pub struct C2SUserJoined(pub UserNick, pub UserColor, pub RoomID);
#[derive(Debug, Deserialize)]
pub struct C2SUserChNick(pub UserNick, pub UserColor);
#[derive(Debug, Deserialize)]
pub struct C2SRoom(pub RoomID, #[serde(skip)] ());
#[derive(Debug, Deserialize)]
pub struct C2SMessage(pub String, #[serde(skip)] ());
#[derive(Debug, Deserialize)]
pub struct C2STyping(pub bool, #[serde(skip)] ());
#[derive(Debug, Deserialize)]
pub struct C2SMouse(pub f32, pub f32);

#[derive(Debug, Clone, Serialize, new)]
pub struct S2CHello(String, UserID);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CRoom(RoomID, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CHistory(VecDeque<HistEntry>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserJoined(User, u64);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserLeft(UserID, u64);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserChNick(UserID, (UserNick, UserColor), (UserNick, UserColor), u64);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CMouse(UserID, f32, f32);
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CUserUpdate(Vec<User>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CTyping(Vec<UserID>, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CMessage(TextMessage, #[serde(skip)] ());
#[derive(Debug, Clone, Serialize, new)]
pub struct S2CRateLimits(SusRate, #[serde(skip)] ());

#[derive(Debug)]
pub enum ClientOp {
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
pub enum ServerOp {
	// server be like "you should kill yourself NOW"
	Disconnect,
	// server messages
	MsgHello(S2CHello),
	MsgRoom(S2CRoom),
	MsgHistory(S2CHistory),
	MsgUserJoined(S2CUserJoined),
	MsgUserLeft(S2CUserLeft),
	MsgUserChNick(S2CUserChNick),
	MsgMouse(S2CMouse),
	MsgUserUpdate(S2CUserUpdate),
	MsgTyping(S2CTyping),
	MsgMessage(S2CMessage),
	MsgRateLimits(S2CRateLimits)
}


pub fn serverop_to_histentry(c: ServerOp, ducks: &HashMap<UserID, Susser>) -> Option<HistEntry> {
	match c {
		ServerOp::MsgUserJoined(ref m) => Some(HistEntry::Join {
			ts: m.1,
			nick: m.0.nick.clone(),
			home: m.0.haship,
			color: m.0.color,
			sid: m.0.id
		}),
		ServerOp::MsgUserLeft(ref m) => { let mf = ducks.get(&m.0).unwrap(); Some(HistEntry::Leave {
			ts: m.1,
			nick: mf.u.nick.clone(),
			home: mf.u.haship,
			color: mf.u.color,
			sid: mf.u.id
		}) },
		ServerOp::MsgUserChNick(ref m) => { let mf = ducks.get(&m.0).unwrap(); Some(HistEntry::ChNick {
			ts: m.3,
			home: mf.u.haship,
			sid: mf.u.id,
			old_nick: m.1.0.clone(),
			new_nick: m.2.0.clone(),
			old_color: m.1.1,
			new_color: m.2.1
		}) },
		ServerOp::MsgMessage(ref m) => { let mf = ducks.get(&m.0.sid).unwrap(); Some(HistEntry::Message {
			ts: m.0.time,
			nick: mf.u.nick.clone(),
			home: mf.u.haship,
			color: mf.u.color,
			sid: mf.u.id,
			content: m.0.content.clone()
		}) },
		_ => None
	}
}

pub fn hash_ip(inp: &IpAddr) -> UserHashedIP {
	//todo: make it irreversible
	UserHashedIP(match inp {
		IpAddr::V4(v4) => {
			let oc = v4.octets();
			// hash the /16 subnet
			0x19fa920130b0ba21u64 |
				(u64::from(oc[0]) * 10495007) |
				(u64::from(oc[1]) * 39950100)
		},
		IpAddr::V6(v6) => {
			let oc = v6.segments();
			// hash the /48 subnet
			0x481040b16b00b135u64 |
				(u64::from(oc[0]) * 40100233) |
				(u64::from(oc[1]) * 40100100) |
				(u64::from(oc[2]) * 49521111)
		}
	})
}
