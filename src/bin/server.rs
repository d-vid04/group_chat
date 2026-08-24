use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

// The server builds \room, \members and \from lines, so it needs those
// prefixes. It matches bare command words like "\pubkey" and "\to" directly,
// which is why those two prefix constants are only used by the client.
use group_chat::protocol::{
    DEFAULT_ROOM, FROM_PREFIX, MEMBERS_PREFIX, ROOM_PREFIX, address_from_args,
};


const HELP: &str = r"Available commands:
  \help                   show this message
  \list_rooms             list the rooms and how many people are in each
  \list_all               list the rooms and who is in each one
  \people                 list the people in your current room
  \create <room>          create a new room and join it
  \create_hidden <room>   create a room that does not appear in listings
  \join <room>            join a room that already exists
  \quit                   leave the chat";

// One connected user. `stream` is a write handle other threads use to send to
// this user, and `room` is the single room they are currently in.
struct Client {
    stream: TcpStream,
    room: String,
    // Whether the room this user is in is hidden from listings. It is stored
    // per person because a room only exists while somebody is in it, so the
    // flag disappears together with the room when the last person leaves.
    room_hidden: bool,
    // This user's X25519 public key, base64 encoded. The server passes it on
    // to other members so they can encrypt to this user; it never holds a
    // private key and never sees a plaintext message.
    pubkey: Option<String>,
}

type Clients = Arc<Mutex<HashMap<String, Client>>>;

// All server logging goes to stderr, so stdout stays clean.
// Run `./server 2> debug.log` to send it to a file instead.
fn debug(message: &str) {
    eprintln!("[debug] {}", message);
}

// Every server -> client message is exactly one line. The client reads with
// read_line, so the newline is what tells it a message has ended.
fn send_line(stream: &mut TcpStream, message: &str) -> std::io::Result<()> {
    writeln!(stream, "{}", message)
}

// Tell one client which room they are in. The client uses this to update its
// prompt and does not show it to the user.
fn send_room(stream: &mut TcpStream, room: &str) {
    let _ = send_line(stream, &format!("{}{}", ROOM_PREFIX, room));
}

// Read one line from a client. None means the connection is finished, either
// because the client disconnected or because the read failed.
fn read_line(reader: &mut BufReader<TcpStream>) -> Option<String> {
    let mut line = String::new();
    match reader.read_line(&mut line) {
        Ok(0) => None,
        Ok(_) => Some(line.trim().to_string()),
        Err(error) => {
            debug(&format!("read error: {}", error));
            None
        }
    }
}

// Send a message to everyone in `room` except `sender`.
// The write handles are collected while the lock is held, then written to
// after it is released, so one slow client cannot block the whole server.
fn broadcast(clients: &Clients, room: &str, sender: &str, message: &str) {
    let mut targets = Vec::new();
    {
        let guard = clients.lock().unwrap();
        for (name, client) in guard.iter() {
            // A "let chain": the `let Ok(handle) = ...` only runs if the
            // conditions before it were true, and `handle` is in scope for
            // the body. Stable since the 2024 edition.
            if client.room == room && name != sender && let Ok(handle) = client.stream.try_clone() {
                targets.push(handle);
            }
        }
    }
    for mut target in targets {
        let _ = send_line(&mut target, message);
    }
}

// Send one line to one named person, but only if they are in `room`. This is
// what stops a client from delivering blobs to people outside its own room.
fn send_to_member(clients: &Clients, room: &str, recipient: &str, message: &str) -> bool {
    let handle = {
        let guard = clients.lock().unwrap();
        match guard.get(recipient) {
            Some(client) if client.room == room => client.stream.try_clone().ok(),
            _ => None,
        }
    };
    match handle {
        Some(mut stream) => send_line(&mut stream, message).is_ok(),
        None => false,
    }
}

// Tell everyone in a room who else is there, and what their public keys are.
// Clients need this to encrypt: there is one ciphertext per recipient, so a
// sender has to know exactly who is listening right now. Sent whenever the
// membership of a room changes.
fn send_member_list(clients: &Clients, room: &str) {
    let (line, targets) = {
        let guard = clients.lock().unwrap();
        let mut entries = Vec::new();
        let mut targets = Vec::new();
        for (name, client) in guard.iter() {
            if client.room == room {
                if let Some(key) = &client.pubkey {
                    entries.push(format!("{}={}", name, key));
                }
                if let Ok(handle) = client.stream.try_clone() {
                    targets.push(handle);
                }
            }
        }
        entries.sort();
        (format!("{}{}", MEMBERS_PREFIX, entries.join(" ")), targets)
    };
    for mut target in targets {
        let _ = send_line(&mut target, &line);
    }
}

// Which room is this user in right now?
fn room_of(clients: &Clients, username: &str) -> String {
    let guard = clients.lock().unwrap();
    match guard.get(username) {
        Some(client) => client.room.clone(),
        None => DEFAULT_ROOM.to_string(),
    }
}

// A room exists for as long as at least one person is in it. `general` is
// always treated as existing, which is what keeps it alive when it empties.
fn room_exists(clients: &Clients, room: &str) -> bool {
    if room == DEFAULT_ROOM {
        return true;
    }
    let guard = clients.lock().unwrap();
    guard.values().any(|client| client.room == room)
}

// Is this room hidden? A room is hidden when the people in it say it is.
// `general` can never be hidden.
fn room_is_hidden(clients: &Clients, room: &str) -> bool {
    if room == DEFAULT_ROOM {
        return false;
    }
    let guard = clients.lock().unwrap();
    guard
        .values()
        .any(|client| client.room == room && client.room_hidden)
}

// The people in one room, sorted. Works for hidden rooms too, which is what
// lets \people show you who is in the private room you are actually in.
fn people_in(clients: &Clients, room: &str) -> Vec<String> {
    let guard = clients.lock().unwrap();
    let mut names: Vec<String> = guard
        .iter()
        .filter(|(_, client)| client.room == room)
        .map(|(name, _)| name.clone())
        .collect();
    names.sort();
    names
}

// Every room that shows up in a listing, with the people in it.
// Hidden rooms are left out entirely. `general` is always included, even empty.
fn visible_rooms(clients: &Clients) -> Vec<(String, Vec<String>)> {
    let mut rooms: HashMap<String, Vec<String>> = HashMap::new();
    rooms.insert(DEFAULT_ROOM.to_string(), Vec::new());
    {
        let guard = clients.lock().unwrap();
        for (name, client) in guard.iter() {
            if client.room_hidden {
                continue;
            }
            rooms.entry(client.room.clone()).or_default().push(name.clone());
        }
    }
    let mut list: Vec<(String, Vec<String>)> = rooms.into_iter().collect();
    for (_, members) in list.iter_mut() {
        members.sort();
    }
    list.sort();
    list
}

// Move a user to another room and tell both rooms about it.
fn move_to_room(clients: &Clients, username: &str, new_room: &str, hidden: bool) {
    let old_room = room_of(clients, username);
    if old_room == new_room {
        return;
    }

    broadcast(clients, &old_room, username, &format!("* {} left the room", username));
    {
        let mut guard = clients.lock().unwrap();
        if let Some(client) = guard.get_mut(username) {
            client.room = new_room.to_string();
            client.room_hidden = hidden;
        }
    }
    broadcast(clients, new_room, username, &format!("* {} joined the room", username));

    send_member_list(clients, &old_room);
    send_member_list(clients, new_room);

    debug(&format!("{} moved from '{}' to '{}'", username, old_room, new_room));
    if old_room != DEFAULT_ROOM && !room_exists(clients, &old_room) {
        debug(&format!("room '{}' is empty and was destroyed", old_room));
    }
}

fn handle_client(stream: TcpStream, clients: Clients) {
    // Two handles to the same connection: one to read from, one to write to.
    let mut writer = match stream.try_clone() {
        Ok(handle) => handle,
        Err(error) => {
            debug(&format!("could not clone stream: {}", error));
            return;
        }
    };
    let mut reader = BufReader::new(stream);

    // --- ask for a username ---
    if send_line(&mut writer, "Enter a username:").is_err() {
        return;
    }
    let username = match read_line(&mut reader) {
        Some(name) => name,
        None => return,
    };
    if username.is_empty() || username.starts_with('\\') {
        let _ = send_line(&mut writer, "That username is not allowed. Disconnecting.");
        return;
    }

    // --- register ---
    // The "is it taken?" check and the insert happen under a single lock, so
    // two people picking the same name at the same time cannot both get in.
    let registered = {
        let mut guard = clients.lock().unwrap();
        if guard.contains_key(&username) {
            false
        } else {
            match writer.try_clone() {
                Ok(handle) => {
                    guard.insert(
                        username.clone(),
                        Client {
                            stream: handle,
                            room: DEFAULT_ROOM.to_string(),
                            room_hidden: false,
                            pubkey: None,
                        },
                    );
                    true
                }
                Err(error) => {
                    debug(&format!("could not clone stream for {}: {}", username, error));
                    false
                }
            }
        }
    };
    if !registered {
        let _ = send_line(
            &mut writer,
            &format!("Username {} is already taken. Disconnecting.", username),
        );
        return;
    }

    debug(&format!("{} connected", username));
    send_room(&mut writer, DEFAULT_ROOM);
    let _ = send_line(
        &mut writer,
        &format!("Welcome {}! You are in the '{}' room.", username, DEFAULT_ROOM),
    );
    let _ = send_line(&mut writer, HELP);
    broadcast(&clients, DEFAULT_ROOM, &username, &format!("* {} joined the room", username));

    // --- main loop: one line in, one action ---
    while let Some(line) = read_line(&mut reader) {
        if line.is_empty() {
            continue;
        }

        // Chat never arrives as plaintext: the client seals every message and
        // sends it as a `\to` line. Anything else here is a client that does
        // not speak the protocol, and relaying it would break the promise that
        // the server cannot read messages.
        if !line.starts_with('\\') {
            let _ = send_line(
                &mut writer,
                "Messages must be encrypted. Use the client, not a raw connection.",
            );
            continue;
        }

        // Split "\join general" into the command and its argument.
        let mut parts = line.splitn(2, ' ');
        let command = parts.next().unwrap_or("");
        let argument = parts.next().unwrap_or("").trim().to_string();

        match command {
            // The client registers its public key once, just after connecting.
            "\\pubkey" => {
                {
                    let mut guard = clients.lock().unwrap();
                    if let Some(client) = guard.get_mut(&username) {
                        client.pubkey = Some(argument.clone());
                    }
                }
                send_member_list(&clients, &room_of(&clients, &username));
            }
            // Relay one sealed blob to one person in the same room. The
            // argument is "<recipient> <base64>", and the server treats the
            // base64 as opaque -- it has no key and cannot open it.
            "\\to" => {
                let mut parts = argument.splitn(2, ' ');
                let recipient = parts.next().unwrap_or("").to_string();
                let sealed = parts.next().unwrap_or("");
                let room = room_of(&clients, &username);
                let delivered = send_to_member(
                    &clients,
                    &room,
                    &recipient,
                    &format!("{}{} {}", FROM_PREFIX, username, sealed),
                );
                if !delivered {
                    let _ = send_line(
                        &mut writer,
                        &format!("Could not deliver to '{}' -- not in your room.", recipient),
                    );
                }
            }
            "\\help" => {
                let _ = send_line(&mut writer, HELP);
            }
            "\\list_rooms" => {
                let _ = send_line(&mut writer, "Rooms:");
                for (room, members) in visible_rooms(&clients) {
                    let _ = send_line(
                        &mut writer,
                        &format!("  {} ({} online)", room, members.len()),
                    );
                }
            }
            "\\list_all" => {
                let _ = send_line(&mut writer, "Rooms:");
                for (room, members) in visible_rooms(&clients) {
                    if members.is_empty() {
                        let _ = send_line(&mut writer, &format!("  {} (empty)", room));
                    } else {
                        let _ = send_line(
                            &mut writer,
                            &format!("  {} ({} online): {}", room, members.len(), members.join(", ")),
                        );
                    }
                }
            }
            "\\people" => {
                let room = room_of(&clients, &username);
                let members = people_in(&clients, &room);
                let _ = send_line(
                    &mut writer,
                    &format!("People in '{}' ({}): {}", room, members.len(), members.join(", ")),
                );
            }
            "\\create" | "\\create_hidden" => {
                let hidden = command == "\\create_hidden";
                if argument.is_empty() {
                    let _ = send_line(&mut writer, &format!("Usage: {} <room>", command));
                } else if room_exists(&clients, &argument) {
                    let _ = send_line(
                        &mut writer,
                        &format!(r"Room '{}' already exists. Use \join instead.", argument),
                    );
                } else {
                    move_to_room(&clients, &username, &argument, hidden);
                    send_room(&mut writer, &argument);
                    let kind = if hidden { "hidden room" } else { "room" };
                    let _ = send_line(
                        &mut writer,
                        &format!("Created {} '{}' and joined it.", kind, argument),
                    );
                }
            }
            "\\join" => {
                if argument.is_empty() {
                    let _ = send_line(&mut writer, r"Usage: \join <room>");
                } else if !room_exists(&clients, &argument) {
                    let _ = send_line(
                        &mut writer,
                        &format!(r"Room '{}' does not exist. Use \create to make it.", argument),
                    );
                } else if argument == room_of(&clients, &username) {
                    let _ = send_line(&mut writer, &format!("You are already in '{}'.", argument));
                } else {
                    // A room keeps whatever visibility it already had, so joining
                    // a hidden room does not accidentally expose it in listings.
                    let hidden = room_is_hidden(&clients, &argument);
                    move_to_room(&clients, &username, &argument, hidden);
                    send_room(&mut writer, &argument);
                    let _ = send_line(&mut writer, &format!("Joined '{}'.", argument));
                }
            }
            "\\quit" => break,
            _ => {
                let _ = send_line(
                    &mut writer,
                    &format!(r"Unknown command '{}'. Type \help for the list.", command),
                );
            }
        }
    }

    // --- cleanup: runs however the loop ended ---
    let last_room = {
        let mut guard = clients.lock().unwrap();
        guard.remove(&username).map(|client| client.room)
    };
    if let Some(room) = last_room {
        send_member_list(&clients, &room);
        broadcast(&clients, &room, &username, &format!("* {} left the chat", username));
        debug(&format!("{} disconnected (was in '{}')", username, room));
        if room != DEFAULT_ROOM && !room_exists(&clients, &room) {
            debug(&format!("room '{}' is empty and was destroyed", room));
        }
    }
}

fn main() -> std::io::Result<()> {
    // ./server            listen on the default address
    // ./server 9000       listen on port 9000
    let address = address_from_args();
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let listener = TcpListener::bind(&address).expect("Failed to bind to address");
    debug(&format!("server listening on {}", address));

    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let clients = Arc::clone(&clients);
                std::thread::spawn(move || handle_client(stream, clients));
            }
            Err(error) => debug(&format!("connection failed: {}", error)),
        }
    }
    Ok(())
}
