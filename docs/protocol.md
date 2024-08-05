# The DuckTB protocol
This is a chatbox thingy. (like IRC) (but IRC is kinda better)

<!-- TODO: make this better? -->

## Data types
| Type            | Description |
|:--------------- |:----------- |
| `u64`           | An unsigned 64-bit integer. |
| `f32`           | An IEEE 754 single-precision floating-point number. |
| `str`           | A UTF-8 encoded string. |
| `bytes`         | A byte sequence. Represented as a base64 encoded string in JSON. |
| `RoomHandle`    | An 8-bit integer. Represents an index into the "rooms" array. |
| `HomeID`        | A 64-bit integer, representing the user's IP address. Represented as a 16-digit hex string in JSON. |
| `UserID`        | A 32-bit integer. Represented as a 8-digit hex string in JSON. |
| `UserNick`      | A UTF-8 encoded string. May not be empty. |
| `UserColor`     | A 24-bit integer representing an sRGB color (0xRRGGBB). Represented as a hex color string in JSON. |
| `Ratelimits`    | An object (see below). Represents the number of times you can do things in 5 second intervals. |
| `User`          | An object (see below). |
| `TextMessage`   | An object (see below). |
| `TextMessageDM` | An object (see below). |

## Data structures
The following are TypeScript-ish type definitions. The order of fields is guaranteed.
```ts
type User = {
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	home: HomeID
};
type TextMessage = {
	time: u64,
	sid: UserID,
	content: str,
};
type TextMessageDM = {
	time: u64,
	sid: UserID,
	sent_to: UserID,
	content: str,
};
type HistEntry = {
	type: "message",
	home: HomeID,
	sid: UserID,
	content: str,
	nick: UserNick,
	color: UserColor,
	ts: u64
} | {
	type: "join"
	home: HomeID,
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	ts: u64
} | {
	type: "leave"
	home: HomeID,
	sid: UserID,
	nick: UserNick,
	color: UserColor,
	ts: u64
} | {
	type: "chnick",
	home: HomeID,
	sid: UserID,
	old_nick: UserNick,
	old_color: UserColor,
	new_nick: UserNick,
	new_color: UserColor,
	ts: u64
};
type Ratelimits = {
	mouse: number,
	chnick: number,
	room: number,
	message: number,
	message_dm: number,
	typing: number,
	events: number
};
```


## Server-bound messages (C2S)
| json-v2            | msgpack-v1 | argument structure |
|:------------------ |:---------- |:------------------ |
| `USER_JOINED`      | `0x13` | `UserNick, UserColor, RoomID[]` |
| `MOUSE`            | `0x10` | `RoomHandle, x: f32, y: f32` |
| `TYPING`           | `0x16` | `RoomHandle, is_typing: bool` |
| `MESSAGE`          | `0x17` | `RoomHandle, content: str` |
| `MESSAGE_DM`       | `0x18` | `RoomHandle, content: str, send_to: UserID` |
| `ROOM_JOIN`        | `0x11` | `RoomID, exclusive: bool` |
| `ROOM_LEAVE`       | `0x12` | `RoomHandle` |
| `USER_CHANGE_NICK` | `0x14` | `UserNick, UserColor` |
| `CUSTOM_R`         | `0x21` | `RoomHandle, type: str, data: bytes` |
| `CUSTOM_U`         | `0x22` | `RoomHandle, send_to: UserID, type: str, data: bytes` |

## Client-bound messages (S2C)
| json-v2            | msgpack-v1 | argument structure |
|:------------------ |:---------- |:------------------ |
| `HELLO`            | `0xff` | `str, UserID` |
| `MOUSE`            | `0x10` | `RoomHandle, UserID, x: f32, y: f32` |
| `ROOM`             | `0x11` | `RoomID[]` |
| `USER_UPDATE`      | `0x12` | `RoomHandle, User[]` |
| `USER_JOINED`      | `0x13` | `RoomHandle, User, timestamp: u64` |
| `USER_CHANGE_NICK` | `0x14` | `RoomHandle, UserID, prev: (UserNick, UserColor), curr: (UserNick, UserColor), timestamp: u64` |
| `USER_LEFT`        | `0x15` | `RoomHandle, UserID, timestamp: u64` |
| `TYPING`           | `0x16` | `RoomHandle, UserID[]` |
| `MESSAGE`          | `0x17` | `RoomHandle, TextMessage` |
| `MESSAGE_DM`       | `0x18` | `RoomHandle, TextMessageDM` |
| `HISTORY`          | `0x19` | `RoomHandle, HistoryEntry[]` |
| `RATE_LIMITS`      | `0x20` | `Ratelimits` |
| `CUSTOM_R`         | `0x21` | `RoomHandle, UserID, type: String, data: bytes` |
| `CUSTOM_U`         | `0x22` | `RoomHandle, UserID, type: String, data: bytes` |
