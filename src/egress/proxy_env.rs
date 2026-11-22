//! Fixed proxy environment of a broker-egress worker.
//!
//! These values only help clients find the loopback helper; removing them
//! never yields a route, because the worker stays on network `none`.

use std::time::{Duration, Instant};

/// Marker read by the provider bootstrap; its value is the proxy URL.
pub const EGRESS_PROXY_MARKER: &str = "AI_GUARD_EGRESS_PROXY";
const NO_PROXY: &str = "localhost,127.0.0.1,::1";

pub fn proxy_url(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// Every name/value pair set in the worker for a helper on `port`.
pub fn proxy_environment(port: u16) -> Vec<(&'static str, String)> {
    let proxy = proxy_url(port);
    let mut environment: Vec<(&'static str, String)> =
        ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"]
            .into_iter()
            .map(|name| (name, proxy.clone()))
            .collect();
    environment.extend(
        ["NO_PROXY", "no_proxy"]
            .into_iter()
            .map(|name| (name, NO_PROXY.to_owned())),
    );
    environment.push((EGRESS_PROXY_MARKER, proxy));
    environment
}

/// Parse the marker value: exactly `http://127.0.0.1:<port>`, port >= 1024.
pub fn parse_marker(value: &str) -> Option<u16> {
    let port = value.strip_prefix("http://127.0.0.1:")?;
    if port.is_empty()
        || port.len() > 5
        || port.starts_with('0')
        || !port.bytes().all(|b| b.is_ascii_digit())
    {
        return None;
    }
    port.parse::<u16>().ok().filter(|port| *port >= 1024)
}

/// Wait, bounded, until the loopback helper accepts connections. Liveness
/// only: an untrusted helper answering proves nothing about the route.
pub fn wait_for_helper(port: u16, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    let address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return false;
        }
        if std::net::TcpStream::connect_timeout(&address, left.min(Duration::from_millis(500)))
            .is_ok()
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100).min(left));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn egress_proxy_environment_is_fixed_and_the_marker_parser_is_closed() {
        let environment = proxy_environment(3128);
        assert_eq!(environment.len(), 7);
        assert!(environment.contains(&("HTTPS_PROXY", "http://127.0.0.1:3128".into())));
        assert!(environment.contains(&("no_proxy", "localhost,127.0.0.1,::1".into())));
        assert_eq!(parse_marker("http://127.0.0.1:3128"), Some(3128));
        for bad in [
            "",
            "http://127.0.0.1:80",
            "http://127.0.0.1:03128",
            "http://localhost:3128",
            "https://127.0.0.1:3128",
            "http://127.0.0.1:3128/",
            "http://127.0.0.1:65536",
            "http://127.0.0.1:+3128",
        ] {
            assert_eq!(parse_marker(bad), None, "{bad}");
        }
    }

    #[test]
    fn egress_proxy_wait_reports_a_listening_helper_and_bounds_a_missing_one() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(wait_for_helper(port, Duration::from_secs(5)));
        drop(listener);
        let started = Instant::now();
        assert!(!wait_for_helper(port, Duration::from_millis(300)));
        assert!(started.elapsed() < Duration::from_secs(3));
    }
}
