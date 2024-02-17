use crate::types::SusRate;

// use a sufficently random 64-bit integer here
pub const HASHIP_SALT: u64 = 0x30e23bc61058987c;
// what should we send as the "server" field in the hello message?
pub const HELLO_IDENTITY: &str = "never liked ur smile brah";
// should we trust the X-Forwarded-For header?
pub const TRUST_REAL_IP_HEADER: bool = true;
// what room should we consider the default?
pub const LOBBY_ROOM_NAME: &str = "lobby";
// what rooms should we keep in memory even if there aren't any users in them?
pub const KEEP_ROOMS: [&'static str; 2] = ["lobby", "duck-room"];
// how many messages should we store in rooms?
pub const HIST_ENTRY_MAX: usize = 512;
// how many events are users allowed to send in a 5-second period?
pub const MAX_EVENTS: &'static SusRate = &SusRate {
	mouse: 100,
	chnick: 1,
	room: 2,
	message: 5,
	message_dm: 10,
	typing: 12,
	events: 150
};

// after how many 5-second intervals do sessions without an active connection die?
pub const SESSION_TIMEOUT: i32 = 4;
// interval between pings in seconds
pub const PING_INTERVAL: u64 = 20;
