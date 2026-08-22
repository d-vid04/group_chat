use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};

// the shared roster: username -> a write handle for that client
type Clients = Arc<Mutex<HashMap<String, TcpStream>>>;

fn handle_client(mut stream : TcpStream, clients: Clients){

    let respone = format!("Enter a username: ");
    stream.write_all(respone.as_bytes()).expect("Failed to write respone to client");
    let mut buffer = [0; 1024];
        let _n = match stream.read(&mut buffer){
            Ok(0) => return,
            Ok(_n) => _n,
            Err(e) =>{
                eprintln!("client read error: {}", e);
                return
            }
        };
        let username = String::from_utf8_lossy(&buffer[.._n]).trim().to_string();
        if username == "exit"{
            return;
        }
        if clients.lock().unwrap().contains_key(&username){
            let respone = format!("Username {} is already taken. Disconnecting.", username);
            stream.write_all(respone.as_bytes()).expect("Failed to write respone to client");
            return;
        }
    // register. try_clone gives the map its own handle, so other threads can
    // write to this client while this thread keeps `stream` for reading.
    match stream.try_clone() {
        Ok(handle) => {
            clients.lock().unwrap().insert(username.clone(), handle);
        }
        Err(e) => {
            eprintln!("failed to clone stream for {}: {}", username, e);
            return;
        }
    }
    println!("{} joined ({} online)", username, clients.lock().unwrap().len());
    stream.write_all(format!("{} connected to the server.\n", username).as_bytes()).expect("Failed to write connection message to client");

    loop{
        let mut buffer = [0; 1024];
        let _n = match stream.read(&mut buffer){
            Ok(0) => break,
            Ok(_n) => _n,
            Err(e) =>{
                eprintln!("client read error: {}", e);
                break;
            }
        };
        let request = String::from_utf8_lossy(&buffer[.._n]).to_string();
        if request.trim() == "exit"{
            break;
        }
        let respone = format!("You wrote: {}", request);
        if let Err(e) = stream.write_all(respone.as_bytes()) {
            eprintln!("client write error: {}", e);
            break;
        }
    }

    // deregister on every exit path out of the loop
    clients.lock().unwrap().remove(&username);
    println!("{} left ({} online)", username, clients.lock().unwrap().len());
}


fn main() -> std::io::Result<()>{
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let listener = TcpListener::bind("127.0.0.1:8080").expect("Failed to bind to address");
    println!("Server listening on port 8080");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let clients = Arc::clone(&clients);
                std::thread::spawn(move || handle_client(stream, clients));
            }
            Err(e) => {
                eprintln!("Connection failed: {}", e)
            }
        }
    }
    Ok(())
}
