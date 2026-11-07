//! Packet counters of the tunnel's network interface (macOS only).
//!
//! An armed tunnel routes its AllowedIPs into its utun interface, so a new connection into the
//! tunnel must send a packet (its SYN) out of that interface. The interface's output counter is
//! one `getifaddrs` call away (microseconds), while a full socket scan costs ~0.6 ms of a
//! performance core; while armed, the On-Demand poll scans only when the counter moved.
//!
//! The interface is found by the config's `Address =` (wg-quick assigns it to the utun), as the
//! run folder's `<tunnel>.name` file that names it is readable by root only.

use crate::on_demand::ip_bytes;

/// Offset of `ifi_opackets` (u32) in `struct if_data` (<net/if_var.h>), which `getifaddrs`
/// gives for each AF_LINK entry.
const IF_DATA_OPACKETS: usize = 28;

/// The addresses of an `Address = 10.43.66.2/32, fd00::2/128` value, as raw bytes.
pub fn tunnel_addresses(address_value: &str) -> Vec<Vec<u8>> {
    address_value
        .split(',')
        .filter_map(|a| ip_bytes(a.trim().split('/').next().unwrap_or("")))
        .collect()
}

/// Packets sent so far out of the interface holding one of `addrs` (a wrapping 32-bit count:
/// compare for equality only). `None` when no interface has them (tunnel down) or the list
/// cannot be read.
pub fn out_packets(addrs: &[Vec<u8>]) -> Option<u32> {
    if addrs.is_empty() {
        return None;
    }
    let mut list: *mut libc::ifaddrs = std::ptr::null_mut();
    // SAFETY: `list` is a valid out-parameter; freed below.
    if unsafe { libc::getifaddrs(&mut list) } != 0 {
        return None;
    }
    let entries = || {
        let mut p = list;
        std::iter::from_fn(move || {
            // SAFETY: `p` walks the list getifaddrs returned, which stays valid until freed.
            let entry = unsafe { p.as_ref()? };
            p = entry.ifa_next;
            Some(entry)
        })
    };
    let name = entries()
        .find(|e| addr_bytes(e.ifa_addr).is_some_and(|b| addrs.contains(&b)))
        .map(|e| e.ifa_name);
    let count = name.and_then(|name| {
        entries()
            .filter(|e| {
                // SAFETY: both names are NUL-terminated strings owned by the list.
                unsafe { libc::strcmp(e.ifa_name, name) == 0 }
            })
            .find(|e| family(e.ifa_addr) == Some(libc::AF_LINK) && !e.ifa_data.is_null())
            .map(|e| {
                // SAFETY: for AF_LINK entries `ifa_data` points to a `struct if_data` (96 bytes).
                unsafe {
                    e.ifa_data
                        .cast::<u8>()
                        .add(IF_DATA_OPACKETS)
                        .cast::<u32>()
                        .read_unaligned()
                }
            })
    });
    // SAFETY: `list` came from getifaddrs and nothing borrowed from it outlives this call.
    unsafe { libc::freeifaddrs(list) };
    count
}

fn family(sa: *const libc::sockaddr) -> Option<i32> {
    // SAFETY: getifaddrs entries hold either null or a valid sockaddr.
    unsafe { sa.as_ref() }.map(|s| i32::from(s.sa_family))
}

/// The address of an AF_INET / AF_INET6 sockaddr as 4 / 16 raw bytes.
fn addr_bytes(sa: *const libc::sockaddr) -> Option<Vec<u8>> {
    match family(sa)? {
        libc::AF_INET => {
            // SAFETY: the family says this is a sockaddr_in.
            let sin = unsafe { &*sa.cast::<libc::sockaddr_in>() };
            Some(sin.sin_addr.s_addr.to_ne_bytes().to_vec())
        }
        libc::AF_INET6 => {
            // SAFETY: the family says this is a sockaddr_in6.
            let sin6 = unsafe { &*sa.cast::<libc::sockaddr_in6>() };
            Some(sin6.sin6_addr.s6_addr.to_vec())
        }
        _ => None,
    }
}
