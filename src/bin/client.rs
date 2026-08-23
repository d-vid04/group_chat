use std::io::{self, BufRead, BufReader, Write};
use std::net::TcpStream;
use std::sync::{Arc, Mutex};

use group_chat::protocol::{ADDRESS, DEFAULT_ROOM, ROOM_PREFIX};


fn main() -> std::io::Result<()> {
    let stream = TcpStream::connect(ADDRESS).expect("Failed to connect");

    // Two handles to the same connection: one to read from, one to write to.
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

    // The prompt shows the room we are in. Both threads need it: the reader
    // thread redraws it, and the keyboard thread prints it before each input.
    // The server tells us the room, so it can change while we are running.
    let prompt = Arc::new(Mutex::new(format!("[{}] ", DEFAULT_ROOM)));

    // --- reader thread ---
    // This blocks on the socket while the main thread blocks on the keyboard.
    // Without it, messages from other people would only appear after you sent
    // something yourself, and the two sides could block waiting on each other.
    let reader_prompt = Arc::clone(&prompt);
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

                    // `\room <name>` is the server telling us where we are.
                    // It is not a chat message, so update the prompt instead
                    // of printing it.
                    if let Some(room) = text.strip_prefix(ROOM_PREFIX) {
                        *reader_prompt.lock().unwrap() = format!("[{}] ", room);
                        continue;
                    }

                    // "\r" jumps back to the start of the line and "\x1b[2K"
                    // erases it, so the message replaces the prompt already on
                    // screen instead of being tacked onto the end of it.
                    print!("\r\x1b[2K{}\n", text);

                    // Give the user a fresh prompt underneath the message, but
                    // only once nothing else is already waiting to be printed,
                    // so a multi-line reply does not draw a prompt per line.
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
        if writeln!(writer, "{}", input).is_err() {
            println!("Lost connection to the server.");
            break;
        }
        if input == "\\quit" {
            break;
        }
    }
    Ok(())
}
