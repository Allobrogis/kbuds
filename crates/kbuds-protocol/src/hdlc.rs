// SPDX-License-Identifier: GPL-3.0-or-later
//! pw_hdlc UI frames.
//!
//! ```text
//! 7E | address (one-terminated varint) | 03 | payload | CRC-32 (zlib, LE) | 7E
//! ```
//! `7E` and `7D` inside the frame (CRC included) are escaped as `7D, b ^ 0x20`.

use crate::address::Endpoint;

pub const FLAG: u8 = 0x7E;
pub const ESCAPE: u8 = 0x7D;
/// Control byte of an unnumbered-information frame.
pub const CONTROL_UI: u8 = 0x03;
/// Largest unescaped frame (address + control + payload + CRC) the buds accept.
pub const MAX_FRAME: usize = 4096;
/// Smallest unescaped frame: 1 address byte, control, 4 CRC bytes.
const MIN_FRAME: usize = 6;
/// Longest address varint the decoder accepts.
const MAX_ADDRESS_BYTES: usize = 10;

/// HDLC address for a frame from `src` to `dst`: source in bits 6–9, destination in bits 10–13.
pub const fn address(src: Endpoint, dst: Endpoint) -> u64 {
    (((src as u64) & 15) << 6) | (((dst as u64) & 15) << 10)
}

/// Source endpoint id (bits 6–9) of an address.
pub const fn address_src(addr: u64) -> u8 {
    ((addr >> 6) & 15) as u8
}

/// Destination endpoint id (bits 10–13) of an address.
pub const fn address_dst(addr: u64) -> u8 {
    ((addr >> 10) & 15) as u8
}

fn push_escaped(out: &mut Vec<u8>, b: u8) {
    if b == FLAG || b == ESCAPE {
        out.push(ESCAPE);
        out.push(b ^ 0x20);
    } else {
        out.push(b);
    }
}

/// pw_hdlc one-terminated varint: 7 bits per byte shifted left by one, bit 0 set on the last byte.
pub fn encode_address(mut addr: u64, out: &mut Vec<u8>) {
    loop {
        let b = ((addr & 0x7F) as u8) << 1;
        addr >>= 7;
        if addr == 0 {
            out.push(b | 1);
            return;
        }
        out.push(b);
    }
}

/// Encodes one complete frame, flags included.
pub fn encode(addr: u64, payload: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(payload.len() + 8);
    encode_address(addr, &mut body);
    body.push(CONTROL_UI);
    body.extend_from_slice(payload);
    let crc = crc32fast::hash(&body).to_le_bytes();
    let mut out = Vec::with_capacity(body.len() * 2 + 6);
    out.push(FLAG);
    for &b in body.iter().chain(crc.iter()) {
        push_escaped(&mut out, b);
    }
    out.push(FLAG);
    out
}

/// A decoded frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub address: u64,
    pub control: u8,
    pub payload: Vec<u8>,
}

impl Frame {
    pub fn src(&self) -> u8 {
        address_src(self.address)
    }
    pub fn dst(&self) -> u8 {
        address_dst(self.address)
    }
}

/// Why bytes between two flags were dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DecodeError {
    TooShort,
    TooLong,
    BadCrc,
    /// Unterminated, longer than 10 bytes, or zero.
    BadAddress,
    /// An escape byte right before the closing flag.
    DanglingEscape,
}

/// Item produced by the [`Reassembler`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Decoded {
    Frame(Frame),
    /// A run of bytes between flags that is not a valid frame (the raw unescaped bytes).
    Invalid(DecodeError, Vec<u8>),
}

/// Parses the unescaped content between two flags.
pub fn parse_frame(buf: &[u8]) -> Result<Frame, DecodeError> {
    if buf.len() < MIN_FRAME {
        return Err(DecodeError::TooShort);
    }
    if buf.len() > MAX_FRAME {
        return Err(DecodeError::TooLong);
    }
    let (body, crc) = buf.split_at(buf.len() - 4);
    if crc32fast::hash(body).to_le_bytes() != crc {
        return Err(DecodeError::BadCrc);
    }
    let mut addr: u64 = 0;
    let mut i = 0;
    let mut terminated = false;
    while i < body.len() && i < MAX_ADDRESS_BYTES {
        let b = body[i];
        addr |= u64::from(b >> 1) << (7 * i);
        i += 1;
        if b & 1 == 1 {
            terminated = true;
            break;
        }
    }
    // The control byte must follow the address.
    if !terminated || addr == 0 || i >= body.len() {
        return Err(DecodeError::BadAddress);
    }
    Ok(Frame { address: addr, control: body[i], payload: body[i + 1..].to_vec() })
}

/// Streaming frame decoder. Feed it raw socket reads; frames may be split or batched arbitrarily.
///
/// Idle flags are skipped and a closing flag may open the next frame.
#[derive(Debug, Default)]
pub struct Reassembler {
    buf: Vec<u8>,
    in_frame: bool,
    escaped: bool,
    overflow: bool,
}

impl Reassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// Consumes `data` and returns every complete item.
    pub fn feed(&mut self, data: &[u8]) -> Vec<Decoded> {
        let mut out = Vec::new();
        for &b in data {
            if b == FLAG {
                if self.in_frame && (!self.buf.is_empty() || self.escaped) {
                    let buf = std::mem::take(&mut self.buf);
                    let item = if self.escaped {
                        Err(DecodeError::DanglingEscape)
                    } else if self.overflow {
                        Err(DecodeError::TooLong)
                    } else {
                        parse_frame(&buf)
                    };
                    out.push(match item {
                        Ok(f) => Decoded::Frame(f),
                        Err(e) => Decoded::Invalid(e, buf),
                    });
                }
                self.buf.clear();
                self.in_frame = true;
                self.escaped = false;
                self.overflow = false;
                continue;
            }
            if !self.in_frame {
                continue;
            }
            let byte = if self.escaped {
                self.escaped = false;
                b ^ 0x20
            } else if b == ESCAPE {
                self.escaped = true;
                continue;
            } else {
                b
            };
            if self.buf.len() < MAX_FRAME + 1 {
                self.buf.push(byte);
            } else {
                self.overflow = true;
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::address::Endpoint::*;

    fn h(s: &str) -> Vec<u8> {
        hex::decode(s.replace(' ', "")).unwrap()
    }

    #[test]
    fn address_bytes() {
        let enc = |s, d| {
            let mut v = Vec::new();
            encode_address(address(s, d), &mut v);
            v
        };
        assert_eq!(enc(MaestroA, LeftBtCore), h("00 3b"));
        assert_eq!(enc(MaestroA, RightBtCore), h("00 4b"));
        assert_eq!(enc(LeftBtCore, MaestroA), h("80 a3"));
        assert_eq!(enc(RightBtCore, MaestroA), h("00 a5"));
    }

    #[test]
    fn worked_example() {
        // WriteSetting anc_state = ACTIVE, MAESTRO_A → LEFT, channel 19.
        let packet = h("10 13 1d ea 71 de 7e 25 1d 9a 8c 9e 2a 04 22 02 68 02");
        let frame = encode(address(MaestroA, LeftBtCore), &packet);
        assert_eq!(
            frame,
            h("7e 00 3b 03 10 13 1d ea 71 de 7d 5e 25 1d 9a 8c 9e 2a 04 22 02 68 02 42 d8 d5 f5 7e")
        );
        let mut r = Reassembler::new();
        let items = r.feed(&frame);
        assert_eq!(
            items,
            vec![Decoded::Frame(Frame {
                address: address(MaestroA, LeftBtCore),
                control: CONTROL_UI,
                payload: packet
            })]
        );
    }

    #[test]
    fn transport_vector_split_and_idle_flags() {
        // Confirmed on hardware: GetSoftwareInfo to RIGHT, channel 21.
        let frame = h("7e 00 4b 03 10 15 1d ea 71 de 7d 5e 25 44 fa 99 71 2b 4b c2 42 7e");
        let mut stream = h("7e 7e");
        stream.extend_from_slice(&frame);
        stream.extend_from_slice(&frame[1..]); // closing flag opens the next frame
        let mut r = Reassembler::new();
        let mut items = Vec::new();
        for b in &stream {
            items.extend(r.feed(std::slice::from_ref(b)));
        }
        assert_eq!(items.len(), 2);
        for item in items {
            let Decoded::Frame(f) = item else { panic!("{item:?}") };
            assert_eq!(f.src(), MaestroA as u8);
            assert_eq!(f.dst(), RightBtCore as u8);
            assert_eq!(f.payload, h("10 15 1d ea 71 de 7e 25 44 fa 99 71"));
        }
    }

    #[test]
    fn rejects_bad_frames() {
        let good = encode(address(LeftBtCore, MaestroA), &[0x08, 0x01]);
        let mut bad_crc = good.clone();
        let n = bad_crc.len();
        bad_crc[n - 2] ^= 1;
        let mut r = Reassembler::new();
        assert!(matches!(r.feed(&bad_crc)[..], [Decoded::Invalid(DecodeError::BadCrc, _)]));

        // Address 0.
        let mut body = vec![0x01, CONTROL_UI, 0x08];
        body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
        assert_eq!(parse_frame(&body), Err(DecodeError::BadAddress));
        // Unterminated address (no byte with bit 0 set before the end).
        let mut body = vec![0x02; 4];
        body.extend_from_slice(&crc32fast::hash(&body).to_le_bytes());
        assert_eq!(parse_frame(&body), Err(DecodeError::BadAddress));
        // Too short.
        assert_eq!(parse_frame(&[1, 3, 0, 0]), Err(DecodeError::TooShort));
        // Oversize.
        let big = encode(address(LeftBtCore, MaestroA), &vec![0u8; MAX_FRAME]);
        assert!(matches!(r.feed(&big)[..], [Decoded::Invalid(DecodeError::TooLong, _)]));
        // The reassembler recovers after garbage.
        let mut stream = h("01 02 7e ff 7d");
        stream.extend_from_slice(&good);
        let items = r.feed(&stream);
        assert!(matches!(items.last(), Some(Decoded::Frame(_))));
    }

    #[test]
    fn escapes_crc_and_payload() {
        let payload = [0x7e, 0x7d, 0x00];
        let frame = encode(address(MaestroB, RightBtCore), &payload);
        assert!(!frame[1..frame.len() - 1].contains(&FLAG));
        let items = Reassembler::new().feed(&frame);
        let [Decoded::Frame(f)] = &items[..] else { panic!() };
        assert_eq!(f.payload, payload);
    }
}
