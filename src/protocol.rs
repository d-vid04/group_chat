/// Where the server listens and the client connects.
pub const ADDRESS: &str = "127.0.0.1:8080";

/// The room everyone starts in. It is never destroyed, even when empty.
pub const DEFAULT_ROOM: &str = "general";

/// Control line the server sends to tell a client which room it is in, for
/// example `\room general`. The client uses it to update its prompt and does
/// not show it to the user.
pub const ROOM_PREFIX: &str = "\\room ";
