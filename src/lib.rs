#![cfg_attr(not(feature = "std"), no_std)]
#![doc = include_str!("../README.md")]
#![warn(
    missing_docs,
    rust_2018_idioms,
    trivial_casts,
    unused_lifetimes,
    unused_qualifications,
    missing_debug_implementations,
    clippy::cognitive_complexity,
    clippy::missing_const_for_fn,
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::semicolon_if_nothing_returned
)]

use crate::aegis_128l::Aegis128L;

#[cfg(feature = "std")]
use std::io::{self, Read, Write};

use cmov::CmovEq;
#[cfg(feature = "hedge")]
use rand_core::{CryptoRng, RngCore};
use sha2::digest::FixedOutputReset;
use sha2::{Digest, Sha256};

#[cfg(feature = "docs")]
#[doc = include_str!("../design.md")]
pub mod design {}

#[cfg(feature = "docs")]
#[doc = include_str!("../perf.md")]
pub mod perf {}

mod aegis_128l;
mod integration_tests;

/// The length of an authentication tag in bytes.
pub const TAG_LEN: usize = aegis_128l::AES_BLOCK_LEN;

/// A stateful object providing fine-grained symmetric-key cryptographic services like hashing,
/// message authentication codes, pseudo-random functions, authenticated encryption, and more.
#[derive(Debug, Clone)]
pub struct Protocol {
    state: Sha256,
}

impl Protocol {
    /// Create a new protocol with the given domain.
    #[inline]
    pub fn new(domain: &'static str) -> Protocol {
        // Create a protocol with a fresh SHA-256 instance.
        let mut protocol = Protocol { state: Sha256::new() };

        // Update the protocol with the domain and the INIT operation.
        protocol.process(domain.as_bytes(), Operation::Init);

        protocol
    }

    /// Mixes the given slice into the protocol state.
    #[inline]
    pub fn mix(&mut self, data: &[u8]) {
        // Update the state with the data and operation code.
        self.process(data, Operation::Mix);
    }

    /// Mixes the contents of the reader into the protocol state.
    ///
    /// # Errors
    ///
    /// Returns any errors returned by the reader or writer.
    #[cfg(feature = "std")]
    pub fn mix_stream(&mut self, reader: impl Read) -> io::Result<u64> {
        self.copy_stream(reader, io::sink())
    }

    /// Mixes the contents of the reader into the protocol state while copying them to the writer.
    ///
    /// # Errors
    ///
    /// Returns any errors returned by the reader or writer.
    #[cfg(feature = "std")]
    pub fn copy_stream(
        &mut self,
        mut reader: impl Read,
        mut writer: impl Write,
    ) -> io::Result<u64> {
        let mut buf = [0u8; 64 * 1024];
        let mut n = 0;

        loop {
            match reader.read(&mut buf) {
                Ok(0) => break, // EOF
                Ok(x) => {
                    let block = &buf[..x];
                    self.state.update(block);
                    writer.write_all(block)?;
                    n += u64::try_from(x).expect("usize should be <= u64");
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }

        // Update the state with the operation code and byte count.
        self.end_op(Operation::Mix, n);

        Ok(n)
    }

    /// Derive output from the protocol's current state and fill the given slice with it.
    #[inline]
    pub fn derive(&mut self, out: &mut [u8]) {
        // Chain the protocol's key and generate an output key.
        let mut output = self.chain(Operation::Derive);

        // Fill the buffer with PRF output.
        output.prf(out);

        // Update the state with the output length and the operation code.
        self.process(&(out.len() as u64).to_le_bytes(), Operation::Derive);
    }

    /// Derive output from the protocol's current state and return it as an array.
    #[inline]
    pub fn derive_array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0u8; N];
        self.derive(&mut out);
        out
    }

    /// Encrypt the given slice in place.
    #[inline]
    pub fn encrypt(&mut self, in_out: &mut [u8]) {
        // Chain the protocol's key and generate an output key.
        let mut output = self.chain(Operation::Crypt);

        // Encrypt the plaintext.
        output.encrypt(in_out);

        // Calculate the 256-bit tag.
        let (_, l_tag) = output.finalize();

        // Update the state with the tag and the operation code.
        self.process(&l_tag, Operation::Crypt);
    }

    /// Decrypt the given slice in place.
    #[inline]
    pub fn decrypt(&mut self, in_out: &mut [u8]) {
        // Chain the protocol's key and generate an output key.
        let mut output = self.chain(Operation::Crypt);

        // Decrypt the plaintext.
        output.decrypt(in_out);

        // Calculate the 256-bit tag.
        let (_, l_tag) = output.finalize();

        // Update the state with the tag and the operation code.
        self.process(&l_tag, Operation::Crypt);
    }

    /// Seals the given mutable slice in place.
    ///
    /// The last `TAG_LEN` bytes of the slice will be overwritten with the authentication tag.
    #[inline]
    pub fn seal(&mut self, in_out: &mut [u8]) {
        // Split the buffer into plaintext and tag.
        let (in_out, tag_out) = in_out.split_at_mut(in_out.len() - TAG_LEN);

        // Chain the protocol's key and generate an output key.
        let mut output = self.chain(Operation::AuthCrypt);

        // Encrypt the plaintext.
        output.encrypt(in_out);

        // Calculate the short and long tags.
        let (s_tag, l_tag) = output.finalize();

        // Append the short tag to the ciphertext.
        tag_out.copy_from_slice(&s_tag);

        // Update the state with the long tag and the operation code.
        self.process(&l_tag, Operation::AuthCrypt);
    }

    /// Opens the given mutable slice in place. Returns the plaintext slice of `in_out` if the input
    /// was authenticated. The last `TAG_LEN` bytes of the slice will be unmodified.
    #[inline]
    #[must_use]
    pub fn open<'a>(&mut self, in_out: &'a mut [u8]) -> Option<&'a [u8]> {
        // Split the buffer into ciphertext and tag.
        let (in_out, s_tag) = in_out.split_at_mut(in_out.len() - TAG_LEN);

        // Chain the protocol's key and generate an output key.
        let mut output = self.chain(Operation::AuthCrypt);

        // Decrypt the plaintext.
        output.decrypt(in_out);

        // Calculate the counterfactual short and long tags.
        let (s_tag_p, l_tag) = output.finalize();

        // Update the state with the long tag and the operation code.
        self.process(&l_tag, Operation::AuthCrypt);

        // Check the tag against the counterfactual short tag in constant time.
        let mut eq = 0u8;
        s_tag.cmoveq(&s_tag_p, 1, &mut eq);
        if eq == 1 {
            // If the tag is verified, then the ciphertext is authentic. Return the slice of the
            // input which contains the plaintext.
            Some(in_out)
        } else {
            // Otherwise, the ciphertext is inauthentic and we zero out the inauthentic plaintext to
            // avoid bugs where the caller forgets to check the return value of this function and
            // discloses inauthentic plaintext.
            in_out.fill(0);
            None
        }
    }

    /// Modifies the protocol's state irreversibly, preventing rollback.
    pub fn ratchet(&mut self) {
        // Chain the protocol's key, ignoring the PRF output.
        let _ = self.chain(Operation::Ratchet);

        // Update the state with the operation code and zero length.
        self.end_op(Operation::Ratchet, 0);
    }

    /// Clones the protocol and mixes `secrets` plus 64 random bytes into the clone. Passes the
    /// clone to `f` and if `f` returns `Some(R)`, returns `R`. Iterates until a value is returned.
    #[cfg(feature = "hedge")]
    #[must_use]
    pub fn hedge<R>(
        &self,
        mut rng: impl RngCore + CryptoRng,
        secrets: &[impl AsRef<[u8]>],
        f: impl Fn(&mut Self) -> Option<R>,
    ) -> R {
        for _ in 0..10_000 {
            // Clone the protocol's state.
            let mut clone = self.clone();

            // Mix each secret into the clone.
            for s in secrets {
                clone.mix(s.as_ref());
            }

            // Mix a random value into the clone.
            let mut r = [0u8; 64];
            rng.fill_bytes(&mut r);
            clone.mix(&r);

            // Call the given function with the clone and return if the function was successful.
            if let Some(r) = f(&mut clone) {
                return r;
            }
        }

        unreachable!("unable to hedge a valid value in 10,000 tries");
    }

    /// Replace the protocol's state with derived output and return an operation-specific AEGIS-128L
    /// instance.
    #[inline]
    #[must_use]
    fn chain(&mut self, operation: Operation) -> Aegis128L {
        // Finalize the current state and reset it to an uninitialized state.
        let hash = self.state.finalize_fixed_reset();

        // Split the hash into a key and nonce and initialize an AEGIS-128L instance for PRF output.
        let (prf_key, prf_nonce) = hash.split_at(16);
        let mut prf = Aegis128L::new(
            &prf_key.try_into().expect("should be 16 bytes"),
            &prf_nonce.try_into().expect("should be 16 bytes"),
        );

        // Use the AEGIS-128L instance to generate a chain key and an output key and nonce.
        let mut prf_out = [0u8; 64];
        prf.prf(&mut prf_out);
        let (chain_key, output_key) = prf_out.split_at_mut(32);
        let (output_key, output_nonce) = output_key.split_at_mut(16);

        // Initialize the current state with the chain key.
        self.process(chain_key, Operation::Chain);

        // Set the first byte of the output nonce to the operation code.
        output_nonce[0] = operation as u8;

        // Return a AEGIS-128L instance keyed with the output key and nonce.
        Aegis128L::new(
            &output_key.try_into().expect("should be 16 bytes"),
            &output_nonce.try_into().expect("should be 16 bytes"),
        )
    }

    // Process a single piece of input for an operation.
    #[inline]
    fn process(&mut self, input: &[u8], operation: Operation) {
        // Update the state with the input.
        self.state.update(input);

        // End the operation with the operation code and input length.
        self.end_op(operation, input.len() as u64);
    }

    /// End an operation, including the number of bytes processed.
    #[inline]
    fn end_op(&mut self, operation: Operation, n: u64) {
        // Allocate a buffer for output.
        let mut buffer = [0u8; 10];
        let (re_x, re_n) = buffer.split_at_mut(8);
        let (re_n, op) = re_n.split_at_mut(1);

        // Encode the number of bytes processed using NIST SP-800-185's right_encode.
        re_x.copy_from_slice(&n.to_be_bytes());
        let offset = re_x.iter().position(|i| *i != 0).unwrap_or(7);
        re_n[0] = 8 - offset as u8;

        // Set the last byte to the operation code.
        op[0] = operation as u8;

        // Update the state with the length and operation code.
        self.state.update(&buffer[offset..]);
    }
}

/// A primitive operation in a protocol with a unique 1-byte code.
#[derive(Debug, Clone, Copy)]
enum Operation {
    Init = 0x01,
    Mix = 0x02,
    Derive = 0x03,
    Crypt = 0x04,
    AuthCrypt = 0x05,
    Ratchet = 0x06,
    Chain = 0x07,
}

#[cfg(all(test, feature = "std"))]
mod tests {
    use std::io::Cursor;

    use expect_test::expect;

    use super::*;

    #[test]
    fn known_answers() {
        let mut protocol = Protocol::new("com.example.kat");
        protocol.mix(b"one");
        protocol.mix(b"two");

        expect!["3f6d24ea37711c9e"].assert_eq(&hex::encode(protocol.derive_array::<8>()));

        let mut plaintext = b"this is an example".to_vec();
        protocol.encrypt(&mut plaintext);
        expect!["534f4064af0c07bf6bd8e93e8d39b38c3bc0"].assert_eq(&hex::encode(plaintext));

        protocol.ratchet();

        let plaintext = b"this is an example";
        let mut sealed = vec![0u8; plaintext.len() + TAG_LEN];
        sealed[..plaintext.len()].copy_from_slice(plaintext);
        protocol.seal(&mut sealed);

        expect!["e7cc92b86d79f182b58b778492ad3169d090eddf089710e19b2edeea75da5e3d9628"]
            .assert_eq(&hex::encode(sealed));

        expect!["395ffb61c78bd8c0"].assert_eq(&hex::encode(protocol.derive_array::<8>()));
    }

    #[test]
    fn streams() {
        let mut slices = Protocol::new("com.example.streams");
        slices.mix(b"one");
        slices.mix(b"two");

        let mut streams = Protocol::new("com.example.streams");
        streams.mix_stream(Cursor::new(b"one")).expect("cursor writes should be infallible");

        let mut output = Vec::new();
        streams
            .copy_stream(Cursor::new(b"two"), &mut output)
            .expect("cursor writes should be infallible");

        assert_eq!(slices.derive_array::<16>(), streams.derive_array::<16>());
        assert_eq!(b"two".as_slice(), &output);
    }

    #[test]
    fn hedging() {
        let mut hedger = Protocol::new("com.example.hedge");
        hedger.mix(b"one");
        let tag = hedger.hedge(rand::thread_rng(), &[b"two"], |clone| {
            let tag = clone.derive_array::<16>();
            (tag[0] == 0).then_some(tag)
        });

        assert_eq!(tag[0], 0);
    }

    #[test]
    fn edge_case() {
        let mut sender = Protocol::new("");
        let mut message = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
        sender.encrypt(&mut message);
        let tag_s = sender.derive_array::<TAG_LEN>();

        let mut receiver = Protocol::new("");
        receiver.decrypt(&mut message);
        let tag_r = receiver.derive_array::<TAG_LEN>();

        assert_eq!(message, [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(tag_s, tag_r);
    }
}
