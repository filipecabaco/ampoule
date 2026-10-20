//! What the guest's sockets may do. Configured from the manifest's [network] table,
//! AMPOULE_* environment variables and flags; everything not restricted passes through.
//! Host processes are the exception: the guest runs only what `exec` allows (see `spawn`).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};

use crate::errno;

#[derive(Clone, Default)]
pub struct Policy {
    /// Ports the guest may bind (empty: any). Port 0 (an ephemeral port) is always allowed.
    pub listen: Vec<u16>,
    /// Guest port -> host port for binds.
    pub port_map: Vec<(u16, u16)>,
    /// Address wildcard binds land on.
    pub listen_host: Option<IpAddr>,
    pub deny_connect: Vec<SocketAddr>,
    pub rewrite_connect: Vec<(SocketAddr, SocketAddr)>,
    /// Host programs the guest may start (empty: none).
    pub exec: crate::spawn::Exec,
}

impl Policy {
    /// The guest binds `addr`: the host address to bind instead, or the errno refusing it.
    pub fn bind(&self, addr: SocketAddr) -> Result<SocketAddr, i32> {
        let port = addr.port();
        if port != 0 && !self.listen.is_empty() && !self.listen.contains(&port) {
            return Err(errno::ACCES);
        }
        let host_port = self.port_map.iter().find(|(g, _)| *g == port).map(|(_, h)| *h).unwrap_or(port);
        let ip = match (addr.ip(), self.listen_host) {
            // A wildcard bind lands on listen_host, in the socket's own address family.
            (ip, Some(host)) if ip.is_unspecified() => match (ip, host) {
                (IpAddr::V4(_), IpAddr::V4(h)) => IpAddr::V4(h),
                (IpAddr::V6(_), IpAddr::V6(h)) => IpAddr::V6(h),
                (IpAddr::V6(_), IpAddr::V4(h)) => IpAddr::V6(h.to_ipv6_mapped()),
                (IpAddr::V4(_), IpAddr::V6(h)) => match h.to_ipv4_mapped() {
                    Some(v4) => IpAddr::V4(v4),
                    None if h.is_loopback() => IpAddr::V4(Ipv4Addr::LOCALHOST),
                    None => ip,
                },
            },
            (ip, _) => ip,
        };
        Ok(SocketAddr::new(ip, host_port))
    }

    /// A bound host address as the guest should see it (the port it asked for).
    pub fn guest_local(&self, addr: SocketAddr) -> SocketAddr {
        match self.port_map.iter().find(|(_, h)| *h == addr.port()) {
            Some((g, _)) => SocketAddr::new(addr.ip(), *g),
            None => addr,
        }
    }

    /// The guest connects (or sends) to `addr`: the host address to use instead, or the errno
    /// refusing it. Rules match the address the connection would reach (see `reached`), and a
    /// rewritten destination is checked against the deny list too.
    pub fn connect(&self, addr: SocketAddr) -> Result<SocketAddr, i32> {
        let target = reached(addr);
        let dest = self.rewrite_connect.iter().find(|(from, _)| reached(*from) == target).map(|(_, to)| *to).unwrap_or(addr);
        if self.deny_connect.iter().any(|d| reached(*d) == target || reached(*d) == reached(dest)) {
            return Err(errno::CONNREFUSED);
        }
        // An IPv6 socket rewritten to an IPv4 destination reaches it as IPv4-mapped.
        Ok(match (addr, dest) {
            (SocketAddr::V6(_), SocketAddr::V4(d)) => SocketAddr::new(IpAddr::V6(d.ip().to_ipv6_mapped()), d.port()),
            _ => dest,
        })
    }
}

/// The address a connection to `addr` reaches: IPv4-mapped IPv6 as plain IPv4, without IPv6
/// flow label or scope, and an unspecified address as the loopback it connects to.
fn reached(addr: SocketAddr) -> SocketAddr {
    let ip = match addr.ip() {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(IpAddr::V6(v6), IpAddr::V4),
        ip => ip,
    };
    let ip = match ip {
        IpAddr::V4(v4) if v4.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST),
        IpAddr::V6(v6) if v6.is_unspecified() => IpAddr::V6(Ipv6Addr::LOCALHOST),
        ip => ip,
    };
    SocketAddr::new(ip, addr.port())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addr(s: &str) -> SocketAddr {
        s.parse().unwrap()
    }

    #[test]
    fn bind_refuses_ports_outside_listen() {
        let p = Policy { listen: vec![4000], ..Policy::default() };
        assert_eq!(p.bind(addr("0.0.0.0:5000")), Err(errno::ACCES));
    }

    #[test]
    fn bind_always_allows_an_ephemeral_port() {
        let p = Policy { listen: vec![4000], ..Policy::default() };
        assert_eq!(p.bind(addr("0.0.0.0:0")), Ok(addr("0.0.0.0:0")));
    }

    #[test]
    fn bind_maps_ports_and_wildcard_hosts() {
        let p = Policy { port_map: vec![(4000, 8080)], listen_host: Some("127.0.0.1".parse().unwrap()), ..Policy::default() };
        assert_eq!(p.bind(addr("0.0.0.0:4000")), Ok(addr("127.0.0.1:8080")));
    }

    #[test]
    fn guest_local_reports_the_port_the_guest_asked_for() {
        let p = Policy { port_map: vec![(4000, 8080)], ..Policy::default() };
        assert_eq!(p.guest_local(addr("127.0.0.1:8080")), addr("127.0.0.1:4000"));
    }

    #[test]
    fn deny_catches_ipv4_mapped_ipv6() {
        let p = Policy { deny_connect: vec![addr("10.0.0.2:22")], ..Policy::default() };
        assert_eq!(p.connect(addr("[::ffff:10.0.0.2]:22")), Err(errno::CONNREFUSED));
    }

    #[test]
    fn deny_ignores_ipv6_flow_label_and_scope() {
        let p = Policy { deny_connect: vec![addr("[::1]:22")], ..Policy::default() };
        let tricky = SocketAddr::V6(std::net::SocketAddrV6::new(Ipv6Addr::LOCALHOST, 22, 1, 3));
        assert_eq!(p.connect(tricky), Err(errno::CONNREFUSED));
    }

    #[test]
    fn deny_treats_unspecified_as_loopback() {
        let p = Policy { deny_connect: vec![addr("127.0.0.1:22")], ..Policy::default() };
        assert_eq!(p.connect(addr("0.0.0.0:22")), Err(errno::CONNREFUSED));
    }

    #[test]
    fn rewrite_to_a_denied_address_is_refused() {
        let p = Policy { rewrite_connect: vec![(addr("10.0.0.1:80"), addr("10.0.0.2:22"))], deny_connect: vec![addr("10.0.0.2:22")], ..Policy::default() };
        assert_eq!(p.connect(addr("10.0.0.1:80")), Err(errno::CONNREFUSED));
    }

    #[test]
    fn ipv6_wildcard_bind_with_ipv4_listen_host_stays_ipv6() {
        let p = Policy { listen_host: Some("127.0.0.1".parse().unwrap()), ..Policy::default() };
        assert_eq!(p.bind(addr("[::]:4000")), Ok(addr("[::ffff:127.0.0.1]:4000")));
    }

    #[test]
    fn connect_rewrites_then_denies() {
        let p = Policy { rewrite_connect: vec![(addr("10.0.0.1:5432"), addr("127.0.0.1:6543"))], deny_connect: vec![addr("10.0.0.2:22")], ..Policy::default() };
        assert_eq!((p.connect(addr("10.0.0.1:5432")), p.connect(addr("10.0.0.2:22"))), (Ok(addr("127.0.0.1:6543")), Err(errno::CONNREFUSED)));
    }
}
