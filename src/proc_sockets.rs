//! In-process TCP socket scan with libproc: what `lsof -nP -iTCP` reports, without spawning it.
//!
//! On Connect on Demand, WGBar looks for TCP connections to the tunnel's AllowedIPs once a second.
//! Spawning lsof for that costs a fork/exec, ~20 ms of wall time and a text parse per tick; this
//! makes the same libproc calls lsof makes (list pids, list each one's fds, read socket info) and
//! keeps only connecting/connected sockets whose remote address is inside the tunnel, so there
//! is nothing to format or parse.
//!
//! Like lsof run as the user, it sees the user's own processes only (`proc_pidinfo` fails with
//! EPERM for the others, which are skipped). Unlike netstat's sysctl, libproc keeps working
//! inside an ad-hoc signed app on macOS 27.
//!
//! The structs come from `<sys/proc_info.h>`, which the libc crate only partly mirrors. Rather
//! than re-declaring the 792-byte `socket_fdinfo` tree, the fields are read at their offsets
//! (stable kernel ABI since 10.5, identical on arm64 and x86_64).

use crate::on_demand::{Cidr, TcpSocket};

const PROC_ALL_PIDS: u32 = 1;
const PROC_PIDLISTFDS: libc::c_int = 1;
const PROC_PIDFDSOCKETINFO: libc::c_int = 3;
const PROX_FDTYPE_SOCKET: u32 = 2;
const SOCKINFO_TCP: i32 = 2;
const INI_IPV4: u8 = 0x1;
const INI_IPV6: u8 = 0x2;
const TCPS_SYN_SENT: i32 = 2;
const TCPS_ESTABLISHED: i32 = 4;

/// `sizeof(struct socket_fdinfo)`.
const SOCKET_FDINFO_SIZE: usize = 792;
// Offsets inside socket_fdinfo: `psi` (socket_info) starts at 24; in socket_info, `soi_kind` is
// at 232 and `soi_proto` (here a tcp_sockinfo) at 240; in tcp_sockinfo, `tcpsi_ini`
// (in_sockinfo) is at 0 and `tcpsi_state` at 80; in in_sockinfo, `insi_fport` is at 0,
// `insi_vflag` at 24 and `insi_faddr` at 32 (IPv4: the last 4 of its 16 bytes).
const OFF_KIND: usize = 24 + 232;
const OFF_TCP: usize = 24 + 240;
const OFF_FPORT: usize = OFF_TCP;
const OFF_VFLAG: usize = OFF_TCP + 24;
const OFF_FADDR: usize = OFF_TCP + 32;
const OFF_STATE: usize = OFF_TCP + 80;

extern "C" {
    fn proc_listpids(
        kind: u32,
        typeinfo: u32,
        buffer: *mut libc::c_void,
        size: libc::c_int,
    ) -> libc::c_int;
    fn proc_pidinfo(
        pid: libc::c_int,
        flavor: libc::c_int,
        arg: u64,
        buffer: *mut libc::c_void,
        size: libc::c_int,
    ) -> libc::c_int;
    fn proc_pidfdinfo(
        pid: libc::c_int,
        fd: libc::c_int,
        flavor: libc::c_int,
        buffer: *mut libc::c_void,
        size: libc::c_int,
    ) -> libc::c_int;
}

/// `struct proc_fdinfo`.
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FdInfo {
    fd: i32,
    fd_type: u32,
}

/// A `socket_fdinfo` buffer, aligned like the C struct.
#[repr(C, align(8))]
struct SocketFdInfo([u8; SOCKET_FDINFO_SIZE]);

impl SocketFdInfo {
    fn i32_at(&self, off: usize) -> i32 {
        i32::from_ne_bytes(self.0[off..off + 4].try_into().unwrap())
    }
}

/// Scratch buffers reused across scans (the scan runs once a second on one queue).
pub struct Scanner {
    pids: Vec<libc::pid_t>,
    fds: Vec<FdInfo>,
    info: Box<SocketFdInfo>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            pids: Vec::new(),
            fds: Vec::new(),
            info: Box::new(SocketFdInfo([0; SOCKET_FDINFO_SIZE])),
        }
    }

    /// The same set as `vpn_bound(&parse_lsof(lsof -nP -iTCP -F nT), allowed)`: SYN_SENT and
    /// ESTABLISHED sockets of any visible process whose remote address is inside `allowed`.
    /// `None` when the process list itself could not be read.
    pub fn vpn_bound(&mut self, allowed: &[Cidr]) -> Option<Vec<TcpSocket>> {
        if !self.list_pids() {
            return None;
        }
        let mut found = Vec::new();
        for i in 0..self.pids.len() {
            let pid = self.pids[i];
            if pid <= 0 || !self.list_fds(pid) {
                continue;
            }
            for j in 0..self.fds.len() {
                let fd = self.fds[j];
                if fd.fd_type != PROX_FDTYPE_SOCKET {
                    continue;
                }
                if let Some(socket) = self.tcp_socket(pid, fd.fd, allowed) {
                    found.push(socket);
                }
            }
        }
        Some(found)
    }

    fn list_pids(&mut self) -> bool {
        // SAFETY: a null buffer asks for the size needed.
        let needed = unsafe { proc_listpids(PROC_ALL_PIDS, 0, std::ptr::null_mut(), 0) };
        if needed <= 0 {
            return false;
        }
        // Headroom for processes started between the two calls.
        let cap = needed as usize / size_of::<libc::pid_t>() + 64;
        self.pids.clear();
        self.pids.resize(cap, 0);
        let bytes = (cap * size_of::<libc::pid_t>()) as libc::c_int;
        // SAFETY: `pids` holds `bytes` writable bytes.
        let got = unsafe { proc_listpids(PROC_ALL_PIDS, 0, self.pids.as_mut_ptr().cast(), bytes) };
        if got <= 0 {
            return false;
        }
        self.pids.truncate(got as usize / size_of::<libc::pid_t>());
        true
    }

    /// The fds of `pid` into `self.fds`; false when the process is gone or not ours.
    fn list_fds(&mut self, pid: libc::pid_t) -> bool {
        if self.fds.len() < 256 {
            self.fds.resize(256, FdInfo::default());
        }
        loop {
            let bytes = (self.fds.len() * size_of::<FdInfo>()) as libc::c_int;
            // SAFETY: `fds` holds `bytes` writable bytes of proc_fdinfo records.
            let got = unsafe {
                proc_pidinfo(pid, PROC_PIDLISTFDS, 0, self.fds.as_mut_ptr().cast(), bytes)
            };
            if got <= 0 {
                self.fds.clear();
                return false;
            }
            let n = got as usize / size_of::<FdInfo>();
            if n < self.fds.len() {
                self.fds.truncate(n);
                return true;
            }
            // Filled to the brim: there may be more.
            let grown = self.fds.len() * 2;
            self.fds.resize(grown, FdInfo::default());
        }
    }

    fn tcp_socket(&mut self, pid: libc::pid_t, fd: i32, allowed: &[Cidr]) -> Option<TcpSocket> {
        let info = &mut *self.info;
        // SAFETY: `info` is a writable buffer of exactly sizeof(socket_fdinfo) bytes.
        let got = unsafe {
            proc_pidfdinfo(
                pid,
                fd,
                PROC_PIDFDSOCKETINFO,
                info.0.as_mut_ptr().cast(),
                SOCKET_FDINFO_SIZE as libc::c_int,
            )
        };
        if got as usize != SOCKET_FDINFO_SIZE || info.i32_at(OFF_KIND) != SOCKINFO_TCP {
            return None;
        }
        let state = match info.i32_at(OFF_STATE) {
            TCPS_SYN_SENT => "SYN_SENT",
            TCPS_ESTABLISHED => "ESTABLISHED",
            _ => return None,
        };
        let vflag = info.0[OFF_VFLAG];
        // lsof's order: an IPv4 flag wins (v4-mapped addresses on IPv6 sockets set both).
        let addr: &[u8] = if vflag & INI_IPV4 != 0 {
            &info.0[OFF_FADDR + 12..OFF_FADDR + 16]
        } else if vflag & INI_IPV6 != 0 {
            &info.0[OFF_FADDR..OFF_FADDR + 16]
        } else {
            return None;
        };
        if !allowed.iter().any(|c| c.contains_bytes(addr)) {
            return None;
        }
        // `insi_fport` is an int holding the port in network byte order (lsof: ntohs).
        let port = u16::from_be(info.i32_at(OFF_FPORT) as u16);
        Some(TcpSocket {
            state: state.to_string(),
            remote_ip: ip_string(addr),
            remote_port: port,
        })
    }
}

fn ip_string(addr: &[u8]) -> String {
    match addr.len() {
        4 => std::net::Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3]).to_string(),
        _ => {
            let mut b = [0u8; 16];
            b.copy_from_slice(addr);
            std::net::Ipv6Addr::from(b).to_string()
        }
    }
}
