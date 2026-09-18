//! Read-only registry probe. Binds no shell globals and creates no windows.
//! The two core requests used here never pass file descriptors.
use anyhow::{Context, Result, bail};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    os::unix::net::UnixStream,
    path::PathBuf,
    time::{Duration, Instant},
};
fn word(bytes: &[u8]) -> Result<u32> {
    Ok(u32::from_ne_bytes(
        bytes.try_into().context("Truncated Wayland word")?,
    ))
}
pub fn probe() -> Result<BTreeSet<String>> {
    let display = std::env::var_os("WAYLAND_DISPLAY").context("No WAYLAND_DISPLAY")?;
    let path = PathBuf::from(display);
    let path = if path.is_absolute() {
        path
    } else {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").context("No XDG_RUNTIME_DIR")?).join(path)
    };
    let stream = UnixStream::connect(path)?;
    stream.set_read_timeout(Some(Duration::from_millis(300)))?;
    stream.set_write_timeout(Some(Duration::from_millis(300)))?;
    registry(stream)
}
fn registry(mut stream: impl Read + Write) -> Result<BTreeSet<String>> {
    for words in [[1u32, (12 << 16) | 1, 2], [1u32, 12 << 16, 3]] {
        for w in words {
            stream.write_all(&w.to_ne_bytes())?;
        }
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut globals = BTreeSet::new();
    for _ in 0..1024 {
        if Instant::now() > deadline {
            bail!("Wayland registry probe timed out");
        }
        let mut header = [0u8; 8];
        stream.read_exact(&mut header)?;
        let object = word(&header[..4])?;
        let op = word(&header[4..])?;
        let size = (op >> 16) as usize;
        let opcode = op & 0xffff;
        if !(8..=65532).contains(&size) || !size.is_multiple_of(4) {
            bail!("Invalid Wayland message size");
        }
        let mut body = vec![0u8; size - 8];
        stream.read_exact(&mut body)?;
        if object == 3 && opcode == 0 {
            return Ok(globals);
        }
        if object == 1 && opcode == 0 {
            bail!("Wayland server returned a protocol error");
        }
        if object == 2 && opcode == 0 {
            if body.len() < 12 {
                bail!("Truncated registry event");
            }
            let len = word(&body[4..8])? as usize;
            if len == 0 || len > body.len().saturating_sub(12) || body[8 + len - 1] != 0 {
                bail!("Invalid registry interface string");
            }
            globals.insert(std::str::from_utf8(&body[8..8 + len - 1])?.into());
        }
    }
    bail!("Wayland registry exceeded event budget")
}
#[cfg(test)]
mod tests {
    use super::*;
    struct Fake(std::io::Cursor<Vec<u8>>);
    impl Read for Fake {
        fn read(&mut self, b: &mut [u8]) -> std::io::Result<usize> {
            self.0.read(b)
        }
    }
    impl Write for Fake {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn decodes_registry_and_sync() {
        let name = b"zwlr_layer_shell_v1\0";
        let padded = (name.len() + 3) & !3;
        let size = 8 + 12 + padded;
        let mut data = vec![];
        for w in [2u32, (size as u32) << 16, 1, name.len() as u32] {
            data.extend(w.to_ne_bytes());
        }
        data.extend(name);
        data.resize(16 + padded, 0);
        data.extend(4u32.to_ne_bytes());
        for w in [3u32, 12 << 16, 0] {
            data.extend(w.to_ne_bytes());
        }
        assert!(
            registry(Fake(std::io::Cursor::new(data)))
                .unwrap()
                .contains("zwlr_layer_shell_v1")
        );
    }
    #[test]
    fn malformed_packet_rejected() {
        assert!(registry(Fake(std::io::Cursor::new(vec![1, 0, 0, 0, 0, 0, 4, 0]))).is_err());
    }
}
