use crate::types::SusRate;


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
	typing: 8,
	events: 150
};

