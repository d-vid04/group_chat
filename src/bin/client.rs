use std::io::{self, Read, Write};
use std::net::TcpStream;

fn main() -> std::io::Result<()> {
    let mut stream = TcpStream::connect("127.0.0.1:8080").expect("Failed to connect");
    loop
    {
        let mut response_buffer = [0; 1024];
        let n = stream.read(&mut response_buffer)?;
        println!("Server replied: {}", String::from_utf8_lossy(&response_buffer[..n]));
        io::stdout().flush().expect("Failed to flush stdout");

        print!("Send a message to the server: ");
        io::stdout().flush().expect("Failed to flush stdout");
        let mut request_buffer = [0; 1024];
        let m = io::stdin().read(&mut request_buffer).expect("Failed to read stdin");
        if m == 0 ||request_buffer.starts_with(b"exit"){
            break;
        }
        stream.write_all(&request_buffer[..m]).expect("Failed to write to server.");
    }

    Ok(())
}