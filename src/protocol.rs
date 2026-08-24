//! Values and helpers the server and the client both have to agree on.
//!
//! Keeping a single definition means the two programs cannot drift apart: if
//! one of these changes, both are rebuilt against the new value instead of one
//! side silently disagreeing at runtime.

use base64::Engine;
use base64::engine::general_purpose::STANDARD as B64;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use sha2::{Digest, Sha256};
use x25519_dalek::{PublicKey, StaticSecret};

/// Where the server listens and the client connects.
pub const ADDRESS: &str = "127.0.0.1:8080";

/// The room everyone starts in. It is never destroyed, even when empty.
pub const DEFAULT_ROOM: &str = "general";

/// Which address to use, taken from the command line.
///
/// ```text
/// ./server                 the default in ADDRESS
/// ./server 9000            port 9000 on the default host
/// ./server 1.2.3.4:9000    that exact address
/// ```
pub fn address_from_args() -> String {
    resolve_address(std::env::args().nth(1))
}

// Split out from address_from_args so it can be tested without touching the
// real command line.
fn resolve_address(argument: Option<String>) -> String {
    let host = ADDRESS.split(':').next().unwrap_or("127.0.0.1");
    match argument {
        None => ADDRESS.to_string(),
        // Anything containing a colon is treated as a whole address.
        Some(text) if text.contains(':') => text,
        Some(text) => match text.parse::<u16>() {
            Ok(port) => format!("{}:{}", host, port),
            Err(_) => {
                eprintln!("'{}' is not a port number; using {}", text, ADDRESS);
                ADDRESS.to_string()
            }
        },
    }
}

// --- control lines -------------------------------------------------------
// These are not chat. The client acts on them and does not display them.

/// Server -> client: which room you are in, e.g. `\room general`.
pub const ROOM_PREFIX: &str = "\\room ";

/// Client -> server: my public key, e.g. `\pubkey <base64>`.
pub const PUBKEY_PREFIX: &str = "\\pubkey ";

/// Server -> client: who is in your room and their public keys, e.g.
/// `\members alice=<base64> bob=<base64>`.
pub const MEMBERS_PREFIX: &str = "\\members ";

/// Client -> server: deliver this sealed blob to one person, e.g.
/// `\to alice <base64>`. The server cannot read the blob.
pub const TO_PREFIX: &str = "\\to ";

/// Server -> client: a sealed blob from someone, e.g. `\from bob <base64>`.
pub const FROM_PREFIX: &str = "\\from ";

/// The nonce length used by XChaCha20-Poly1305.
const NONCE_LEN: usize = 24;

/// Turn a public key into text that can travel on a line-based connection.
pub fn encode_public(key: &PublicKey) -> String {
    B64.encode(key.as_bytes())
}

/// Parse a public key back out of that text. `None` if it is not a valid key.
pub fn decode_public(text: &str) -> Option<PublicKey> {
    let bytes = B64.decode(text).ok()?;
    let array: [u8; 32] = bytes.try_into().ok()?;
    Some(PublicKey::from(array))
}

/// Work out the key shared with one other person.
///
/// X25519 gives both sides the same secret from their own private key and the
/// other person's public key, without that secret ever crossing the network.
/// It is hashed before use because the raw output is not uniformly random, and
/// the label keeps this key distinct from any other use of the same pair.
pub fn shared_key(secret: &StaticSecret, their_public: &PublicKey) -> [u8; 32] {
    let shared = secret.diffie_hellman(their_public);
    let mut hasher = Sha256::new();
    hasher.update(b"group_chat v1 message key");
    hasher.update(shared.as_bytes());
    hasher.finalize().into()
}

/// Seal a message so only the holder of the matching key can read it.
///
/// The nonce is random and must never repeat for one key, which is why this
/// uses XChaCha20-Poly1305: its 24-byte nonce is large enough that random
/// values are safe. The result is `nonce || ciphertext`, base64 encoded so it
/// can travel on a line-based connection.
pub fn seal(key: &[u8; 32], message: &str) -> Option<String> {
    let cipher = XChaCha20Poly1305::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::fill(&mut nonce_bytes);
    let nonce = XNonce::from(nonce_bytes);

    let ciphertext = cipher.encrypt(&nonce, message.as_bytes()).ok()?;
    let mut wire = nonce_bytes.to_vec();
    wire.extend_from_slice(&ciphertext);
    Some(B64.encode(wire))
}

/// Open a sealed message. `None` if it was corrupted, tampered with, or is not
/// for us -- Poly1305 authenticates the ciphertext, so a modified message
/// fails to open rather than decrypting to garbage.
pub fn open(key: &[u8; 32], sealed: &str) -> Option<String> {
    let wire = B64.decode(sealed).ok()?;
    if wire.len() < NONCE_LEN {
        return None;
    }
    let nonce = XNonce::try_from(&wire[..NONCE_LEN]).ok()?;
    let cipher = XChaCha20Poly1305::new(key.into());
    let plaintext = cipher.decrypt(&nonce, &wire[NONCE_LEN..]).ok()?;
    String::from_utf8(plaintext).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keypair() -> (StaticSecret, PublicKey) {
        let secret = StaticSecret::random_from_rng(&mut rand::rng());
        let public = PublicKey::from(&secret);
        (secret, public)
    }

    #[test]
    fn both_sides_derive_the_same_key() {
        let (alice_secret, alice_public) = keypair();
        let (bob_secret, bob_public) = keypair();
        assert_eq!(
            shared_key(&alice_secret, &bob_public),
            shared_key(&bob_secret, &alice_public)
        );
    }

    #[test]
    fn a_sealed_message_opens_again() {
        let (alice_secret, alice_public) = keypair();
        let (bob_secret, bob_public) = keypair();
        let sealed = seal(&shared_key(&alice_secret, &bob_public), "hello david").unwrap();
        let opened = open(&shared_key(&bob_secret, &alice_public), &sealed);
        assert_eq!(opened.as_deref(), Some("hello david"));
    }

    #[test]
    fn the_same_message_seals_differently_every_time() {
        // A fresh random nonce each time, so identical messages do not produce
        // identical ciphertext and leak that they were the same.
        let (alice_secret, _) = keypair();
        let (_, bob_public) = keypair();
        let key = shared_key(&alice_secret, &bob_public);
        assert_ne!(seal(&key, "same text").unwrap(), seal(&key, "same text").unwrap());
    }

    #[test]
    fn a_tampered_message_will_not_open() {
        let (alice_secret, alice_public) = keypair();
        let (bob_secret, bob_public) = keypair();
        let sealed = seal(&shared_key(&alice_secret, &bob_public), "transfer $10").unwrap();

        // Flip one byte of the ciphertext.
        let mut wire = B64.decode(&sealed).unwrap();
        let last = wire.len() - 1;
        wire[last] ^= 0x01;
        let tampered = B64.encode(&wire);

        // Poly1305 authenticates the ciphertext, so this fails outright rather
        // than decrypting to something the attacker chose.
        assert_eq!(open(&shared_key(&bob_secret, &alice_public), &tampered), None);
    }

    #[test]
    fn the_wrong_key_will_not_open_it() {
        let (alice_secret, _) = keypair();
        let (_, bob_public) = keypair();
        let (eve_secret, _) = keypair();
        let (_, eve_target) = keypair();

        let sealed = seal(&shared_key(&alice_secret, &bob_public), "private").unwrap();
        assert_eq!(open(&shared_key(&eve_secret, &eve_target), &sealed), None);
    }

    #[test]
    fn public_keys_survive_a_round_trip_through_text() {
        let (_, public) = keypair();
        assert_eq!(decode_public(&encode_public(&public)), Some(public));
    }

    #[test]
    fn the_address_comes_from_the_argument() {
        assert_eq!(resolve_address(None), ADDRESS);
        assert_eq!(resolve_address(Some("9000".to_string())), "127.0.0.1:9000");
        assert_eq!(
            resolve_address(Some("1.2.3.4:9000".to_string())),
            "1.2.3.4:9000"
        );
        // Not a port and not an address: fall back rather than crash.
        assert_eq!(resolve_address(Some("banana".to_string())), ADDRESS);
    }

    #[test]
    fn rubbish_does_not_panic() {
        let (secret, _) = keypair();
        let (_, public) = keypair();
        let key = shared_key(&secret, &public);
        assert_eq!(open(&key, "not base64 at all !!!"), None);
        assert_eq!(open(&key, ""), None);
        assert_eq!(decode_public("nonsense"), None);
    }
}
