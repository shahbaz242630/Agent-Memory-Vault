//! ADR-008 spike — verify dryoc 0.7's actual API for a single-message
//! encrypt/decrypt round-trip, the shape we'll need in T0.2.9 (sync envelope).
//!
//! Run with:
//!
//! ```text
//! cargo run -p vault-sync --example dryoc_spike
//! ```
//!
//! ## Why this spike exists
//!
//! BRD §11.6 sketched the sync envelope assuming dryoc exposed
//! `crypto_aead_xchacha20poly1305_ietf_encrypt` (libsodium's single-shot
//! AEAD). dryoc 0.7's actual public surface for XChaCha20-Poly1305 is
//! `DryocStream` — a streaming construction (`crypto_secretstream_*`).
//! The classic-API submodules (`dryoc::classic::*`) do not include
//! `crypto_aead_xchacha20poly1305_ietf` either; the only single-shot
//! authenticated cipher exposed is `dryocsecretbox` which uses
//! XSalsa20-Poly1305, a different cipher than BRD §11.6 specified.
//!
//! Three candidate paths from ADR-008:
//!
//! 1. **Wrap streaming as single-message** — one `init_push` → `push(...,
//!    FINAL)` → `init_pull` → `pull(...)` cycle per envelope. Header (24
//!    bytes) bundled into the envelope alongside ciphertext. THIS SPIKE
//!    implements path #1 to verify it works.
//! 2. **Sibling crate** (`orion`, `sodiumoxide`) — single-shot AEAD with
//!    XChaCha20. Would mean evaluating + vetting another crypto crate.
//! 3. **RustCrypto `chacha20poly1305`** — pure-Rust, well-audited,
//!    single-shot XChaCha20-Poly1305 AEAD. Drop dryoc entirely.
//!
//! Output of this spike informs the chosen path in ADR-008's amendment.

use dryoc::dryocstream::{DryocStream, Header, Key, Pull, Push, Tag};
use dryoc::types::{Bytes, NewByteArray};

fn main() {
    println!("=== ADR-008 dryoc 0.7 single-message round-trip spike ===");
    println!();

    // NB: dryoc 0.7's `push_to_vec` / `pull_to_vec` require `Input: Bytes`
    // where the type parameter must be Sized. A bare `&[u8]` slice fails
    // the bound (the slice ref is sized but the inferred Input type
    // becomes `[u8]`, which is unsized). We materialise the plaintext as
    // an owned `Vec<u8>` so Input is inferred as `Vec<u8>` (sized).
    let plaintext: Vec<u8> = b"the quick brown fox jumps over the lazy dog \
                               - vault memory id 01ABCDEF / boundary=work"
        .to_vec();
    println!(
        "plaintext ({} bytes): {}",
        plaintext.len(),
        std::str::from_utf8(&plaintext).unwrap()
    );

    // ----------------------------------------------------------------
    // Encrypt side
    // ----------------------------------------------------------------

    // 1. Generate the symmetric key. In T0.2.9 this is the per-envelope
    //    key derived from the master key via HKDF (BRD §11.3.1).
    let key = Key::gen();
    println!("\n[encrypt] generated 32-byte key (Key::gen)");

    // 2. Initialise the push side. The header (24 bytes) is generated
    //    here and must be sent alongside the ciphertext so the receiver
    //    can initialise the matching pull side.
    let (mut push_stream, header): (DryocStream<Push>, Header) = DryocStream::init_push(&key);
    println!("[encrypt] init_push -> header = {} bytes", header.len());

    // 3. One push with Tag::FINAL = a single-message envelope. AAD (the
    //    second argument) can carry our memory id + boundary for AEAD
    //    binding (BRD §11.3.2 "AAD includes memory ID and boundary"); we
    //    pass None here for the spike but T0.2.9 will populate it.
    let ciphertext = push_stream
        .push_to_vec(&plaintext, None, Tag::FINAL)
        .expect("push_to_vec failed");
    println!(
        "[encrypt] push_to_vec(FINAL) -> ciphertext = {} bytes \
              (overhead = {} bytes vs plaintext)",
        ciphertext.len(),
        ciphertext.len() - plaintext.len()
    );

    // The full envelope on the wire is `header || ciphertext`. We model
    // that bundling here so the round-trip is realistic.
    let envelope: Vec<u8> = {
        let mut v = Vec::with_capacity(header.len() + ciphertext.len());
        v.extend_from_slice(header.as_slice());
        v.extend_from_slice(&ciphertext);
        v
    };
    println!(
        "[encrypt] envelope (header || ciphertext) = {} bytes total",
        envelope.len()
    );

    // ----------------------------------------------------------------
    // Decrypt side (simulates the other device or a re-open)
    // ----------------------------------------------------------------

    // 1. Split the envelope back into header + ciphertext.
    //    `pull_to_vec` requires a Sized `Bytes` input, so the ciphertext
    //    is materialised into an owned `Vec<u8>` rather than passed as
    //    a `&[u8]` slice.
    let header_len = header.len();
    let recovered_header_bytes = &envelope[..header_len];
    let recovered_ciphertext: Vec<u8> = envelope[header_len..].to_vec();
    let recovered_header: Header = recovered_header_bytes
        .try_into()
        .expect("header length mismatch");

    // 2. Initialise the pull side using the same key and the recovered
    //    header. In T0.2.9 the key is re-derived locally from the master
    //    key + envelope id; the header travels in the encrypted blob.
    let mut pull_stream: DryocStream<Pull> = DryocStream::init_pull(&key, &recovered_header);
    println!("\n[decrypt] init_pull (same key + recovered header)");

    // 3. One pull. Returns (plaintext, tag) so the receiver can verify
    //    the message ended with Tag::FINAL — for single-shot envelopes
    //    we always assert FINAL and reject anything else.
    let (recovered_plaintext, tag) = pull_stream
        .pull_to_vec(&recovered_ciphertext, None)
        .expect("pull_to_vec failed");
    println!(
        "[decrypt] pull_to_vec -> plaintext = {} bytes, tag = {:?}",
        recovered_plaintext.len(),
        tag
    );

    // ----------------------------------------------------------------
    // Verify round-trip identity + tag semantics
    // ----------------------------------------------------------------

    assert_eq!(
        recovered_plaintext, plaintext,
        "round-trip identity violated"
    );
    assert_eq!(
        recovered_plaintext.len(),
        plaintext.len(),
        "plaintext length mismatch"
    );
    assert!(
        matches!(tag, Tag::FINAL),
        "single-shot envelope must end with Tag::FINAL"
    );

    println!("\n✓ round-trip identity OK");
    println!("✓ tag is Tag::FINAL");

    // ----------------------------------------------------------------
    // Adversarial: wrong key fails closed.
    // ----------------------------------------------------------------

    let wrong_key = Key::gen();
    let mut wrong_pull: DryocStream<Pull> = DryocStream::init_pull(&wrong_key, &recovered_header);
    let wrong_result = wrong_pull.pull_to_vec(&recovered_ciphertext, None);
    assert!(
        wrong_result.is_err(),
        "decryption with wrong key MUST fail (AEAD authenticity check)"
    );
    println!("✓ wrong-key decryption fails closed (AEAD auth)");

    // ----------------------------------------------------------------
    // Adversarial: tampered ciphertext fails closed.
    // ----------------------------------------------------------------

    let mut tampered = recovered_ciphertext.clone();
    tampered[0] ^= 0x01; // flip one bit in the ciphertext body
    let mut tamper_pull: DryocStream<Pull> = DryocStream::init_pull(&key, &recovered_header);
    let tamper_result = tamper_pull.pull_to_vec(&tampered, None);
    assert!(
        tamper_result.is_err(),
        "decryption of tampered ciphertext MUST fail"
    );
    println!("✓ tampered-ciphertext decryption fails closed");

    println!("\n=== spike PASSED — path #1 (streaming-as-single-message) is viable ===");
}
