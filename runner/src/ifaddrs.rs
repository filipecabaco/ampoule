//! The host's network interfaces, for the guest's `getifaddrs`, `if_nametoindex` and
//! `if_indextoname` (engine/ampoule-os/src/aos.c), so `:inet.getifaddrs/0` lists what the app's
//! sockets can actually bind: they are host sockets.
//!
//! `getifaddrs(buf, len)` writes one record per interface address, little-endian, and returns the
//! bytes it needed (more than `len`: nothing was written, call again with that much):
//!
//! ```text
//! u16 name_len · name · u32 flags (Linux IFF_*) · u8 family (0 none, 1 IPv4, 2 IPv6)
//!   IPv4: addr[4] · netmask[4] · u8 kind (0 none, 1 broadcast, 2 point-to-point) · other[4]
//!   IPv6: addr[16] · netmask[16] · u32 scope_id
//! ```
//!
//! An interface with no IPv4 or IPv6 address gets one record of family 0, so it is still listed.

use std::collections::HashSet;
use std::ffi::{CStr, CString};

use wasmtime::{Caller, Linker};

use crate::errno;
use crate::net::ret;
use crate::{k, Ctx};

// The guest's interface flags: Linux's numbering (compat/net/if.h).
const G_IFF_UP: u32 = 0x1;
const G_IFF_BROADCAST: u32 = 0x2;
const G_IFF_LOOPBACK: u32 = 0x8;
const G_IFF_POINTOPOINT: u32 = 0x10;
const G_IFF_RUNNING: u32 = 0x40;
const G_IFF_MULTICAST: u32 = 0x1000;

/// The host's flags in the guest's numbering (macOS numbers IFF_MULTICAST differently).
fn guest_flags(host: u32) -> u32 {
    [
        (libc::IFF_UP, G_IFF_UP),
        (libc::IFF_BROADCAST, G_IFF_BROADCAST),
        (libc::IFF_LOOPBACK, G_IFF_LOOPBACK),
        (libc::IFF_POINTOPOINT, G_IFF_POINTOPOINT),
        (libc::IFF_RUNNING, G_IFF_RUNNING),
        (libc::IFF_MULTICAST, G_IFF_MULTICAST),
    ]
    .iter()
    .filter(|(h, _)| host & (*h as u32) != 0)
    .fold(0, |acc, (_, g)| acc | g)
}

/// The records `getifaddrs` hands the guest (see the module's docs).
pub fn records() -> Vec<u8> {
    let mut out = Vec::new();
    let mut with_address = HashSet::new();
    let mut without = Vec::new();
    let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: getifaddrs fills `head` with a list freed below, and nothing else holds it.
    if unsafe { libc::getifaddrs(&mut head) } != 0 {
        return out;
    }
    let mut p = head;
    while !p.is_null() {
        // SAFETY: a node of the list getifaddrs returned, alive until freeifaddrs.
        let ifa = unsafe { &*p };
        p = ifa.ifa_next;
        // SAFETY: getifaddrs gives every node a NUL-terminated name.
        let name = unsafe { CStr::from_ptr(ifa.ifa_name) }.to_bytes().to_vec();
        let flags = guest_flags(ifa.ifa_flags as u32);
        // SAFETY: ifa_addr, when set, points at a sockaddr of the family it names.
        let family = if ifa.ifa_addr.is_null() { 0 } else { (unsafe { (*ifa.ifa_addr).sa_family }) as i32 };
        match family {
            libc::AF_INET => {
                // SAFETY: an AF_INET address, netmask and broadcast/destination are sockaddr_in.
                let v4 = |sa: *const libc::sockaddr| -> Option<[u8; 4]> {
                    (!sa.is_null()).then(|| unsafe { (*(sa as *const libc::sockaddr_in)).sin_addr.s_addr }.to_ne_bytes())
                };
                let Some(addr) = v4(ifa.ifa_addr) else { continue };
                let mask = v4(ifa.ifa_netmask).unwrap_or([0; 4]);
                let (kind, other) = match other_addr(ifa) {
                    Some(sa) if flags & G_IFF_BROADCAST != 0 => (1u8, v4(sa)),
                    Some(sa) if flags & G_IFF_POINTOPOINT != 0 => (2u8, v4(sa)),
                    _ => (0u8, None),
                };
                record(&mut out, &name, flags, 1);
                out.extend_from_slice(&addr);
                out.extend_from_slice(&mask);
                out.push(if other.is_some() { kind } else { 0 });
                out.extend_from_slice(&other.unwrap_or([0; 4]));
                with_address.insert(name);
            },
            libc::AF_INET6 => {
                // SAFETY: an AF_INET6 address and netmask are sockaddr_in6.
                let v6 = |sa: *const libc::sockaddr| -> Option<libc::sockaddr_in6> { (!sa.is_null()).then(|| unsafe { *(sa as *const libc::sockaddr_in6) }) };
                let Some(addr) = v6(ifa.ifa_addr) else { continue };
                let mask = v6(ifa.ifa_netmask).map(|m| m.sin6_addr.s6_addr).unwrap_or([0; 16]);
                record(&mut out, &name, flags, 2);
                out.extend_from_slice(&addr.sin6_addr.s6_addr);
                out.extend_from_slice(&mask);
                out.extend_from_slice(&addr.sin6_scope_id.to_le_bytes());
                with_address.insert(name);
            },
            _ => without.push((name, flags)),
        }
    }
    // SAFETY: the list getifaddrs returned, freed once.
    unsafe { libc::freeifaddrs(head) };
    let mut listed = HashSet::new();
    for (name, flags) in without {
        if !with_address.contains(&name) && listed.insert(name.clone()) {
            record(&mut out, &name, flags, 0);
        }
    }
    out
}

fn record(out: &mut Vec<u8>, name: &[u8], flags: u32, family: u8) {
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(name);
    out.extend_from_slice(&flags.to_le_bytes());
    out.push(family);
}

// The broadcast or point-to-point address: one field under two names, by platform.
#[cfg(target_os = "linux")]
fn other_addr(ifa: &libc::ifaddrs) -> Option<*const libc::sockaddr> {
    let sa = ifa.ifa_ifu;
    (!sa.is_null()).then_some(sa as *const libc::sockaddr)
}
#[cfg(not(target_os = "linux"))]
fn other_addr(ifa: &libc::ifaddrs) -> Option<*const libc::sockaddr> {
    let sa = ifa.ifa_dstaddr;
    (!sa.is_null()).then_some(sa as *const libc::sockaddr)
}

pub fn add(linker: &mut Linker<Ctx>) -> anyhow::Result<()> {
    let m = "ampoule_os";

    linker.func_wrap(m, "getifaddrs", |c: Caller<'_, Ctx>, buf: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let r = records();
            if r.len() <= len as usize {
                k.mem.write(buf as u32, &r)?;
            }
            Ok(r.len() as i32)
        })())
    })?;

    linker.func_wrap(m, "if_nametoindex", |c: Caller<'_, Ctx>, name: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let name = CString::new(k.mem.read(name as u32, len as u32)?).map_err(|_| errno::INVAL)?;
            // SAFETY: a NUL-terminated name.
            Ok(unsafe { libc::if_nametoindex(name.as_ptr()) } as i32)
        })())
    })?;

    linker.func_wrap(m, "if_indextoname", |c: Caller<'_, Ctx>, index: i32, buf: i32, len: i32| -> i32 {
        let k = k(&c);
        ret((|| {
            let mut name = [0 as libc::c_char; libc::IF_NAMESIZE];
            // SAFETY: a buffer of IF_NAMESIZE, as if_indextoname requires.
            let p = unsafe { libc::if_indextoname(index as u32, name.as_mut_ptr()) };
            if p.is_null() {
                return Err(errno::NXIO);
            }
            // SAFETY: if_indextoname wrote a NUL-terminated name into `name`.
            let name = unsafe { CStr::from_ptr(name.as_ptr()) }.to_bytes_with_nul();
            if name.len() > len as usize {
                return Err(errno::NAMETOOLONG);
            }
            k.mem.write(buf as u32, name)?;
            Ok(0)
        })())
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_are_in_linux_numbering() {
        let host = (libc::IFF_UP | libc::IFF_MULTICAST | libc::IFF_LOOPBACK) as u32;
        assert_eq!(guest_flags(host), G_IFF_UP | G_IFF_MULTICAST | G_IFF_LOOPBACK);
        assert_eq!(guest_flags(0), 0);
    }

    // Walks the records as the guest's getifaddrs does; the loopback interface is always there.
    #[test]
    fn records_list_the_loopback_interface_with_its_addresses() {
        let r = records();
        let mut at = 0;
        let mut loopback = false;
        while at < r.len() {
            let n = u16::from_le_bytes([r[at], r[at + 1]]) as usize;
            let flags = u32::from_le_bytes(r[at + 2 + n..at + 6 + n].try_into().unwrap());
            let family = r[at + 6 + n];
            at += 7 + n;
            at += match family {
                1 => 4 + 4 + 1 + 4,
                2 => 16 + 16 + 4,
                0 => 0,
                other => panic!("family {other}"),
            };
            loopback |= flags & G_IFF_LOOPBACK != 0 && family != 0;
        }
        assert_eq!(at, r.len());
        assert!(loopback);
    }
}
