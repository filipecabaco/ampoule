//! Just enough Mach-O to carry a payload in a section: find a section in a 64-bit Mach-O's bytes,
//! and write one into a stub with `libsui`, which places it before `__LINKEDIT` and signs the
//! result ad hoc (see `pack`).

use anyhow::{bail, Result};

/// The segment libsui writes its sections to.
pub const SEGMENT: &str = "__SUI";

const MH_MAGIC_64: u32 = 0xfeed_facf;
const CPU_TYPE_ARM64: u32 = 0x0100_000c;
const LC_SEGMENT_64: u32 = 0x19;
const HEADER: usize = 32;
const SEGMENT_CMD: usize = 72;
const SECTION_HDR: usize = 80;

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

fn name_is(field: &[u8], name: &str) -> bool {
    let end = field.iter().position(|&c| c == 0).unwrap_or(field.len());
    &field[..end] == name.as_bytes()
}

/// A 64-bit little-endian Mach-O for arm64: the only kind given a payload section.
pub fn is_arm64(b: &[u8]) -> bool {
    u32_at(b, 0) == Some(MH_MAGIC_64) && u32_at(b, 4) == Some(CPU_TYPE_ARM64)
}

/// A section header: (segment name, section name, file offset, size).
type Section<'a> = (&'a [u8], &'a [u8], u64, u64);

/// Every section header.
fn sections(b: &[u8]) -> Option<Vec<Section<'_>>> {
    if u32_at(b, 0)? != MH_MAGIC_64 {
        return None;
    }
    let ncmds = u32_at(b, 16)? as usize;
    let mut at = HEADER;
    let mut out = Vec::new();
    for _ in 0..ncmds {
        let (cmd, size) = (u32_at(b, at)?, u32_at(b, at + 4)? as usize);
        if size < 8 {
            return None;
        }
        if cmd == LC_SEGMENT_64 {
            let nsects = u32_at(b, at + 64)? as usize;
            for i in 0..nsects {
                let s = at + SEGMENT_CMD + i * SECTION_HDR;
                let hdr = b.get(s..s + SECTION_HDR)?;
                out.push((&hdr[16..32], &hdr[..16], u32_at(hdr, 48)? as u64, u64_at(hdr, 40)?));
            }
        }
        at = at.checked_add(size)?;
    }
    Some(out)
}

/// The contents of section `sect` in segment `seg`, by its file offset in `b`.
pub fn section(b: &'static [u8], seg: &str, sect: &str) -> Option<&'static [u8]> {
    let (_, _, off, size) = sections(b)?.into_iter().find(|(g, s, _, _)| name_is(g, seg) && name_is(s, sect))?;
    let end = off.checked_add(size)?;
    b.get(usize::try_from(off).ok()?..usize::try_from(end).ok()?)
}

/// Zeroed bytes between the end of the load commands and the first section's contents: the room a
/// new load command has. libsui writes into it without checking, so a stub without enough would
/// lose the start of its code.
fn header_room(b: &[u8]) -> Option<usize> {
    let end = HEADER + u32_at(b, 20)? as usize;
    let first = sections(b)?.into_iter().filter(|&(_, _, off, size)| off > 0 && size > 0).map(|(_, _, off, _)| off as usize).min()?;
    let gap = b.get(end..first)?;
    Some(gap.iter().take_while(|&&c| c == 0).count())
}

/// `stub` with `data` as section `name` of `SEGMENT`, signed ad hoc.
pub fn with_section(stub: &[u8], name: &str, data: Vec<u8>) -> Result<Vec<u8>> {
    let room = header_room(stub).unwrap_or(0);
    if room < SEGMENT_CMD + SECTION_HDR {
        bail!(
            "this runtime has {room} bytes free after its load commands and a payload section needs {}: use a runtime built with `-Wl,-headerpad` (ampoule's own build.rs passes it)",
            SEGMENT_CMD + SECTION_HDR
        );
    }
    let mut out = Vec::with_capacity(stub.len() + data.len() + (1 << 20));
    libsui::Macho::from(stub.to_vec())?.write_section(name, data)?.build_and_sign(&mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_macho_bytes_have_no_sections() {
        assert_eq!(section(b"\x7fELF and the rest", SEGMENT, "__ampoule"), None);
        assert!(!is_arm64(b"\x7fELF and the rest"));
    }

    #[test]
    fn a_truncated_header_is_not_a_section() {
        let mut b = MH_MAGIC_64.to_le_bytes().to_vec();
        b.extend_from_slice(&CPU_TYPE_ARM64.to_le_bytes());
        b.extend_from_slice(&[0, 0, 0, 0, 2, 0, 0, 0, 9, 0, 0, 0]);
        assert!(is_arm64(&b));
        assert_eq!(section(Box::leak(b.into_boxed_slice()), SEGMENT, "__ampoule"), None);
    }

    /// The running test binary on an arm64 Mac: libsui writes a section into it, and it reads back
    /// from the file offsets alone.
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[test]
    fn a_section_written_into_this_binary_reads_back() {
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        if header_room(&me).unwrap_or(0) < SEGMENT_CMD + SECTION_HDR {
            // Test binaries are not linked with build.rs's header padding.
            assert!(with_section(&me, "__ampoule", b"x".to_vec()).is_err());
            return;
        }
        let data: Vec<u8> = (0..200_000u32).flat_map(|i| i.to_le_bytes()).collect();
        let out: &'static [u8] = Box::leak(with_section(&me, "__ampoule", data.clone()).unwrap().into_boxed_slice());
        assert_eq!(section(out, SEGMENT, "__ampoule"), Some(&data[..]));
    }
}
