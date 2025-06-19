# The DuckTB protocol
This is a chatbox thingy. (like IRC) (but IRC is kinda better)

## Message format
For `json-v2`, messages are a concatenation of:
- the message type
- a null byte
- optionally a request id as a 32-bit unsigned integer in decimal (up to 10 bytes) followed by a null byte
- message contents as a JSON array

For `msgpack-v1`, messages are a concatenation of:
- optionally a null byte followed by a 32-bit request id
- message type as a single byte
- MessagePack-encoded message contents as a sequence

<!-- TODO: make this better? -->

## Data types
| Type            | Description |
|:--------------- |:----------- |
| `u64`           | An unsigned 64-bit integer. |
| `f32`           | An IEEE 754 single-precision floating-point number. |
| `str`           | A UTF-8 encoded string. |
| `bytes`         | A byte sequence. Represented as a base64 encoded string in JSON. It may also be `null` for the server to expect a binary message right after. |
| `RoomHandle`    | An 8-bit integer. Represents an index into the "rooms" array. |
| `HomeID`        | A 64-bit integer, representing the user's IP address. Represented as a 16-digit hex string in JSON. |
| `UserID`        | A 32-bit integer. Represented as a 8-digit hex string in JSON. |
| `MessageID`     | A 32-bit integer. Represented as a 8-digit hex string in JSON. |
| `UserNick`      | A UTF-8 encoded string. May not be empty. |
| `UserColor`     | A 24-bit integer representing an sRGB color (0xRRGGBB). Represented as a hex color string in JSON. |
| `Ratelimits`    | An object (see below). Represents the number of times you can do things in 5 second intervals. |
| `User`          | An object (see below). |
| `TextMessage`   | An object (see below). |

## Data structures
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
	sent_to?: UserID,
	id: MessageID,
} & (
	| { content_type: "message",      content: string /* markdown */ },
	| { content_type: "user_joined",  content: User },
	| { content_type: "user_left",    content: User },
	| { content_type: "user_ch_nick", content: [User, User] },
);
type HistEntry = {
	message: TextMessage,
	from?: User, // null if it's a system message
	to?: User    // null if it's not a DM
}
type Ratelimits = {
	mouse: number, // per room
	chnick: number,
	room: number,
	message: number, // per room
	message_dm: number, // per room
	typing: number, // per room
	events: number
};
```

## Response codes
| code | description           |
| ----:|:----------------------|
|    0 | success               |
|    1 | general failure       |
|    2 | message parsing error |
|    3 | rate limit exceeded   |
|    8 | too many rooms joined |
|    9 | invalid room handle   |
|   10 | no such userid        |
|  255 | internal server error |

## Server-bound messages (C2S)
| json-v2            | msgpack-v1 | argument structure | response type |
|:------------------ |:---------- |:------------------ |:--------------|
|                    |        |                                                       | |
| `USER_JOINED`      | `0x13` | `UserNick, UserColor, RoomID[]`                       | `User` |
| `MOUSE`            | `0x10` | `RoomHandle, x: f32, y: f32`                          | |
| `TYPING`           | `0x16` | `RoomHandle, is_typing: bool`                         | |
| `MESSAGE`          | `0x17` | `RoomHandle, content: str`                            | `MessageID` |
| `MESSAGE_DM`       | `0x18` | `RoomHandle, content: str, send_to: UserID`           | `MessageID` |
| `ROOM_JOIN`        | `0x11` | `RoomID, exclusive: bool`                             | `RoomHandle` |
| `ROOM_LEAVE`       | `0x12` | `RoomHandle`                                          | |
| `USER_CHANGE_NICK` | `0x14` | `UserNick, UserColor`                                 | `User` |
| `CUSTOM_R`         | `0x21` | `RoomHandle, type: str, data: bytes`                  | |
| `CUSTOM_U`         | `0x22` | `RoomHandle, send_to: UserID, type: str, data: bytes` | |

note: with `USER_JOINED` a code of 8 is still treated as a success except that you don't join any rooms
note: with `USER_JOINED` the roomid array is stably deduplicated before being used, but i'd suggest not having duplicated room ids in the in the first place

## Client-bound messages (S2C)
| json-v2            | msgpack-v1 | argument structure |
|:------------------ |:---------- |:------------------ |
| `RESPONSE`         | `0xfe` | `RequestID, u8, String` |
| `HELLO`            | `0xff` | `resume_string: str, UserID` |
| `MOUSE`            | `0x10` | `RoomHandle, UserID, x: f32, y: f32` |
| `ROOM`             | `0x11` | `(RoomID | null)[]` |
| `USER_UPDATE`      | `0x12` | `RoomHandle, User[]` |
| `USER_JOINED`      | `0x13` | `RoomHandle, User` |
| `USER_CHANGE_NICK` | `0x14` | `RoomHandle, UserID, UserNick, UserColor` |
| `USER_LEFT`        | `0x15` | `RoomHandle, UserID` |
| `TYPING`           | `0x16` | `RoomHandle, UserID[]` |
| `MESSAGE`          | `0x17` | `RoomHandle, TextMessage` |
| `MESSAGE_DM`       | `0x18` | `RoomHandle, TextMessage` |
| `HISTORY`          | `0x19` | `RoomHandle, HistoryEntry[]` |
| `RATE_LIMITS`      | `0x20` | `Ratelimits` |
| `CUSTOM_R`         | `0x21` | `RoomHandle, UserID, type: String, data: bytes` |
| `CUSTOM_U`         | `0x22` | `RoomHandle, UserID, type: String, data: bytes` |
