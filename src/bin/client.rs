use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use group_chat::protocol::{
    ADDRESS, DEFAULT_ROOM, FROM_PREFIX, MEMBERS_PREFIX, PUBKEY_PREFIX, ROOM_PREFIX, TO_PREFIX,
    decode_public, encode_public, open, seal, shared_key,
};
use x25519_dalek::{PublicKey, StaticSecret};

// Who else is in our room, and the public key we encrypt to for each of them.
// Both threads need it: the reader thread refreshes it from the server's
// \members lines, and the keyboard thread reads it to seal outgoing messages.
type Members = Arc<Mutex<HashMap<String, PublicKey>>>;

fn main() -> std::io::Result<()> {
    // Our long-term keypair. The private half never leaves this process; only
    // the public half is sent to the server.
    let secret = Arc::new(StaticSecret::random_from_rng(&mut rand::rng()));
    let public = PublicKey::from(secret.as_ref());

    let stream = TcpStream::connect(ADDRESS).expect("Failed to connect");
    let mut writer = stream.try_clone().expect("Failed to clone stream");
    let mut reader = BufReader::new(stream);

    // --- username handshake ---
    let mut server_prompt = String::new();
    if reader.read_line(&mut server_prompt)? == 0 {
        println!("Server closed the connection.");
        return Ok(());
    }
    print!("{} ", server_prompt.trim_end());
    io::stdout().flush()?;

    let mut username = String::new();
    if io::stdin().read_line(&mut username)? == 0 {
        return Ok(());
    }
    let username = username.trim().to_string();
    if username.is_empty() {
        println!("A username is required.");
        return Ok(());
    }
    writeln!(writer, "{}", username)?;

    // Publish our public key so other people can encrypt to us.
    writeln!(writer, "{}{}", PUBKEY_PREFIX, encode_public(&public))?;

    let prompt = Arc::new(Mutex::new(format!("[{}] ", DEFAULT_ROOM)));
    let members: Members = Arc::new(Mutex::new(HashMap::new()));

    // --- reader thread ---
    // Blocks on the socket while the main thread blocks on the keyboard, so
    // messages appear as soon as they arrive rather than only after you type.
    let reader_prompt = Arc::clone(&prompt);
    let reader_members = Arc::clone(&members);
    let reader_secret = Arc::clone(&secret);
    let me = username.clone();
    std::thread::spawn(move || {
        let mut line = String::new();
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    println!("\nDisconnected from server.");
                    std::process::exit(0);
                }
                Ok(_) => {
                    let text = line.trim_end();

                    // Control lines are acted on, not displayed.
                    if let Some(room) = text.strip_prefix(ROOM_PREFIX) {
                        *reader_prompt.lock().unwrap() = format!("[{}] ", room);
                        continue;
                    }
                    if let Some(list) = text.strip_prefix(MEMBERS_PREFIX) {
                        let mut fresh = HashMap::new();
                        for entry in list.split_whitespace() {
                            let mut parts = entry.splitn(2, '=');
                            let name = parts.next().unwrap_or("");
                            let encoded = parts.next().unwrap_or("");
                            if name != me
                                && let Some(key) = decode_public(encoded)
                            {
                                fresh.insert(name.to_string(), key);
                            }
                        }
                        *reader_members.lock().unwrap() = fresh;
                        continue;
                    }
                    if let Some(rest) = text.strip_prefix(FROM_PREFIX) {
                        let mut parts = rest.splitn(2, ' ');
                        let sender = parts.next().unwrap_or("").to_string();
                        let sealed = parts.next().unwrap_or("");

                        // Look up the sender's key and open the message.
                        let sender_key = reader_members.lock().unwrap().get(&sender).copied();
                        let shown = match sender_key {
                            Some(key) => match open(&shared_key(&reader_secret, &key), sealed) {
                                Some(message) => format!("{}: {}", sender, message),
                                None => format!("{}: <could not decrypt>", sender),
                            },
                            None => format!("{}: <unknown sender>", sender),
                        };
                        print!("\r\x1b[2K{}\n", shown);
                        if reader.buffer().is_empty() {
                            print!("{}", reader_prompt.lock().unwrap());
                        }
                        let _ = io::stdout().flush();
                        continue;
                    }

                    // Anything else is a plain notice from the server.
                    print!("\r\x1b[2K{}\n", text);
                    if reader.buffer().is_empty() {
                        print!("{}", reader_prompt.lock().unwrap());
                    }
                    let _ = io::stdout().flush();
                }
                Err(error) => {
                    println!("\nConnection error: {}", error);
                    std::process::exit(1);
                }
            }
        }
    });

    // --- keyboard loop ---
    loop {
        // Erase the line first, in case the reader thread already drew a
        // prompt here -- otherwise the two would print side by side.
        print!("\r\x1b[2K{}", prompt.lock().unwrap());
        io::stdout().flush()?;

        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break; // Ctrl-D
        }
        let input = input.trim();
        if input.is_empty() {
            continue;
        }

        // Commands go to the server as-is; it needs to read them to route.
        if input.starts_with('\\') {
            if writeln!(writer, "{}", input).is_err() {
                println!("Lost connection to the server.");
                break;
            }
            if input == "\\quit" {
                break;
            }
            continue;
        }

        // A chat message is sealed once per recipient. There is no shared room
        // key, so nobody outside the current member list can read it -- which
        // includes the server, and anyone who has already left the room.
        let recipients: Vec<(String, PublicKey)> = members
            .lock()
            .unwrap()
            .iter()
            .map(|(name, key)| (name.clone(), *key))
            .collect();

        if recipients.is_empty() {
            println!("(nobody else is in this room yet)");
            continue;
        }
        for (name, key) in recipients {
            if let Some(sealed) = seal(&shared_key(&secret, &key), input)
                && writeln!(writer, "{}{} {}", TO_PREFIX, name, sealed).is_err()
            {
                println!("Lost connection to the server.");
                return Ok(());
            }
        }
    }
    Ok(())
}
