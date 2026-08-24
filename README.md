# group_chat

An end-to-end encrypted group chat server and client, written in Rust with only
the standard library for networking and threading.

Messages are encrypted on the sending client and decrypted on the receiving
client. The server routes sealed blobs it has no key for — it can see who is
online and who is talking to whom, but not what anyone says.

## Features

- **Multi-client TCP server**, one thread per connection
- **End-to-end encryption** — X25519 key exchange, XChaCha20-Poly1305 AEAD
- **Chat rooms** you can create and join, with automatic cleanup when empty
- **Hidden rooms** that do not appear in any listing
- **Live message delivery** — messages arrive while you are typing, not only
  after you send something
- **Unit-tested cryptography** — `cargo test`

## Requirements

Rust 1.85 or newer (the project uses the 2024 edition). Install from
[rustup.rs](https://rustup.rs).

## Build

```bash
git clone <your-repo-url>
cd group_chat
cargo build
```

## Running

You need the server plus at least two clients — one client on its own has nobody
to talk to.

### Start the server

Open a terminal:

```bash
cargo run --bin server
```

```
[debug] server listening on 127.0.0.1:8080
```

Logging goes to **stderr**, so it can be separated from anything else:

```bash
cargo run --bin server 2> debug.log
```

### Start a client

In a second terminal:

```bash
cargo run --bin client
```

```
Enter a username: david
Welcome david! You are in the 'general' room.
Available commands:
  ...
[general] 
```

Open a third terminal and start another client with a different username. Both
begin in the `general` room and can see each other's messages immediately.

The prompt shows the room you are in, and changes when you move rooms.

## Arguments

Both binaries take an optional address. With no argument they use the default in
`src/protocol.rs`, so they always agree.

| Command | Effect |
|---|---|
| `cargo run --bin server` | listen on `127.0.0.1:8080` |
| `cargo run --bin server -- 9123` | listen on port `9123` |
| `cargo run --bin client` | connect to `127.0.0.1:8080` |
| `cargo run --bin client -- 9123` | connect to port `9123` |
| `cargo run --bin client -- 192.168.1.5:9123` | connect to another machine |

An argument that is neither a port nor an address prints a warning and falls back
to the default rather than failing:

```
$ cargo run --bin server -- banana
'banana' is not a port number; using 127.0.0.1:8080
```

The `--` tells cargo the argument is for your program, not for cargo. Running the
compiled binary directly does not need it:

```bash
./target/debug/server 9123
./target/debug/client 9123
```

## Commands

Anything that does not begin with `\` is sent as an encrypted message to everyone
in your current room.

| Command | What it does |
|---|---|
| `\help` | show the command list |
| `\list_rooms` | list the rooms and how many people are in each |
| `\list_all` | list the rooms and who is in each one |
| `\people` | list the people in your current room |
| `\create <room>` | create a room and join it |
| `\create_hidden <room>` | create a room that does not appear in listings |
| `\join <room>` | join a room that already exists |
| `\quit` | leave the chat |

`Ctrl-D` also exits.

### Rooms

- Everyone starts in **`general`**.
- You are in exactly **one room at a time**. `\create` and `\join` both move you,
  which means leaving the room you were in.
- A room exists for as long as somebody is in it. When the last person leaves it
  is **destroyed** — except `general`, which always exists.
- Messages only reach people in the same room.

### Hidden rooms

`\create_hidden <room>` makes a room that `\list_rooms` and `\list_all` leave out.
Anyone who knows the name can still `\join` it, and `\people` works normally once
you are inside, so members can see who else is there.

Hidden rooms are hidden from **everyone**, including their own members — your
prompt tells you where you are, but the room will not show up in your own
`\list_rooms`.

### Usernames

Each username must be unique among connected users. Taken names are rejected and
the connection closed. Names may not be empty or begin with `\`.

## How the encryption works

Every client generates an X25519 keypair at startup. The private half never
leaves the process; the public half is sent to the server, which passes it on to
other people in the room.

To send a message the client seals it **once per recipient** with
XChaCha20-Poly1305 and sends one sealed blob per person. The server forwards each
blob to its named recipient without being able to open it.

There is no shared room key, which means there is nothing to redistribute when
someone joins and nothing to rotate when someone leaves — a sender simply
encrypts to whoever is on the current member list. Someone who has left the room
is not encrypted to, so they cannot read anything sent afterwards.

### What this does and does not protect

**Protected:** message contents, against the server and against anyone watching
the network.

**Not protected:**

- **Metadata.** The server knows who is online, who is in which room, when people
  join and leave, and who is messaging whom.
- **Past traffic if a key is stolen.** Keys are long-term, so there is no forward
  secrecy. Fixing this needs a ratchet such as Signal's Double Ratchet or MLS.
- **A malicious server.** Public keys are distributed by the server, so a hostile
  one could substitute its own and read everything. Real systems solve this with
  out-of-band key verification.
- **Quantum attackers.** X25519 is classical. Post-quantum resistance would need
  a hybrid X25519 + ML-KEM handshake.

## Project layout

```
src/
├── lib.rs            declares the protocol module
├── protocol.rs       shared constants, crypto helpers, and their tests
└── bin/
    ├── server.rs     accepts connections, routes sealed messages, tracks rooms
    └── client.rs     encrypts, decrypts, and draws the prompt
```

`protocol.rs` holds everything the two programs must agree on. Keeping one
definition means a change on one side cannot silently break the other.

## Tests

```bash
cargo test
```

Covers the cryptography and address parsing: both sides derive the same key,
sealed messages open again, identical text seals differently every time, tampered
messages fail to open, the wrong key fails to open, public keys survive the text
round trip, malformed input returns `None` instead of panicking, and command-line
addresses resolve correctly.

## Notes

- The server logs to stderr, never to stdout.
- The client runs two threads: one blocked on the socket, one blocked on the
  keyboard. That is why messages appear as soon as they arrive.
- If a message arrives while you are half-way through typing, the line is redrawn
  and your typed characters stay in the terminal's buffer — press enter and they
  still send, they are just no longer shown.