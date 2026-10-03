use std::net::{IpAddr, Ipv4Addr, UdpSocket};

/// Addresses that can only originate inside the local network (or the device itself).
pub fn is_local_network(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_v4(v4),
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_private_v4(v4);
            }
            let first = v6.segments()[0];
            v6.is_loopback() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80
        }
    }
}

fn is_private_v4(ip: Ipv4Addr) -> bool {
    ip.is_private() || ip.is_loopback() || ip.is_link_local()
        // 100.64.0.0/10 (CGNAT) is used by some hotspots and by Tailscale.
        || (ip.octets()[0] == 100 && (ip.octets()[1] & 0xc0) == 64)
}

/// IPv4 addresses of this device on private networks, most likely Wi-Fi first.
pub fn lan_ipv4s() -> Vec<Ipv4Addr> {
    let mut ips: Vec<(String, Ipv4Addr)> = if_addrs::get_if_addrs()
        .unwrap_or_default()
        .into_iter()
        .filter_map(|iface| match iface.ip() {
            IpAddr::V4(v4) if !v4.is_loopback() && is_private_v4(v4) => Some((iface.name, v4)),
            _ => None,
        })
        .collect();
    ips.sort_by_key(|(name, _)| {
        let wifi = name.starts_with("wlan") || name.starts_with("en") || name.starts_with("wl");
        let mobile = name.starts_with("rmnet") || name.starts_with("ccmni") || name.starts_with("tun");
        (!wifi, mobile)
    });
    let mut out: Vec<Ipv4Addr> = Vec::new();
    for (_, ip) in ips {
        if !out.contains(&ip) {
            out.push(ip);
        }
    }
    if out.is_empty()
        && let Some(ip) = default_route_ipv4()
    {
        out.push(ip);
    }
    out
}

/// Source address the OS would use to reach the internet. No packet is sent.
fn default_route_ipv4() -> Option<Ipv4Addr> {
    let socket = UdpSocket::bind((Ipv4Addr::UNSPECIFIED, 0)).ok()?;
    socket.connect((Ipv4Addr::new(192, 0, 2, 1), 9)).ok()?;
    match socket.local_addr().ok()?.ip() {
        IpAddr::V4(v4) if !v4.is_unspecified() => Some(v4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_ranges() {
        for ip in ["192.168.1.2", "10.0.0.1", "172.16.5.4", "127.0.0.1", "169.254.1.1", "100.100.1.1", "::1", "fe80::1", "fd00::1", "::ffff:192.168.0.3"] {
            assert!(is_local_network(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["8.8.8.8", "1.1.1.1", "2001:4860::8888", "::ffff:8.8.8.8", "172.32.0.1"] {
            assert!(!is_local_network(ip.parse().unwrap()), "{ip}");
        }
    }
}
