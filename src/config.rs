use crate::types::RatelimitsAvg;

use std::net::SocketAddr;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Deserialize, Clone, PartialEq)]
#[serde(untagged)]
pub enum ListenAddress {
	// try interpreting it as a socketaddr first
	TCP(SocketAddr),
	// then assume it's a path
	Unix(PathBuf),
}

// look at config.toml in the repo for documentation ( :( )
#[derive(Debug, Deserialize, Clone)]
pub struct Config {
	pub listener: ListenerConfig,
	pub talker: TalkerConfig
}

#[derive(Debug, Deserialize, Clone)]
pub struct ListenerConfig {
	pub listen: Vec<ListenAddress>,
	pub trust_real_ip_header: bool,
	pub session_timeout: u16,
	pub ping_interval: u16,
}

#[derive(Debug, Deserialize, Clone)]
pub struct TalkerConfig {
	pub history_max: u16,
	pub ratelimits: RatelimitsAvg,
	pub room_empty_timer: u16,
	pub room_typing_timer: u16,
}

/*

// heheheh it used to be hardcoded

// use a sufficently random 64-bit integer here
//pub const HASHIP_SALT: u64 = 0x30e23bc61058987c;
// should we trust the X-Forwarded-For header?
pub const TRUST_REAL_IP_HEADER: bool = true;
// how many messages should we store in rooms?
pub const HIST_ENTRY_MAX: usize = 512;
// how many events are users allowed to send in a 5-second period?
pub const MAX_EVENTS: &'static RatelimitsAvg = &RatelimitsAvg {
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

*/
