use anyhow::{Context, Result};
use std::io::Read;

pub const MAX_FRAME_BYTES: u32 = 16 * 1024 * 1024;

pub fn encode_frame(body: &[u8]) -> Result<Vec<u8>> {
    anyhow::ensure!(
        body.len() <= MAX_FRAME_BYTES as usize,
        "Guest frame exceeds 16 MiB"
    );
    let mut out = Vec::with_capacity(4 + body.len());
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(body);
    Ok(out)
}

pub fn read_frame(reader: &mut impl Read) -> Result<Vec<u8>> {
    let mut header = [0_u8; 4];
    reader
        .read_exact(&mut header)
        .context("Guest closed the socket")?;
    let size = u32::from_be_bytes(header);
    anyhow::ensure!(
        size <= MAX_FRAME_BYTES,
        "Guest frame length {size} exceeds 16 MiB"
    );
    let mut body = vec![0_u8; size as usize];
    reader.read_exact(&mut body)?;
    Ok(body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn round_trips_a_length_prefixed_frame() {
        let encoded = encode_frame(b"{\"ok\":true}").unwrap();
        assert_eq!(&encoded[..4], 11_u32.to_be_bytes());
        let decoded = read_frame(&mut Cursor::new(encoded)).unwrap();
        assert_eq!(decoded, b"{\"ok\":true}");
    }
}
