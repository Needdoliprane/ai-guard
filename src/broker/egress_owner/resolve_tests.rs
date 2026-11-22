use super::*;
use crate::egress::EgressTransport;
use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

/// Deterministic answer source standing in for a DNS server we own.
struct Answer {
    addresses: Vec<IpAddr>,
    calls: AtomicUsize,
    never: bool,
}
impl HostLookup for Answer {
    fn lookup(&self, _name: String, port: u16) -> LookupFuture {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let addresses = self
            .addresses
            .iter()
            .map(|ip| SocketAddr::new(*ip, port))
            .collect();
        let never = self.never;
        Box::pin(async move {
            if never {
                std::future::pending::<()>().await;
            }
            Ok(addresses)
        })
    }
}
fn answer(addresses: &[&str]) -> Arc<Answer> {
    Arc::new(Answer {
        addresses: addresses.iter().map(|raw| raw.parse().unwrap()).collect(),
        calls: AtomicUsize::new(0),
        never: false,
    })
}
fn authority(name: &str, port: u16) -> Authority {
    Authority::new(name, port, EgressTransport::ConnectTcpOpaque).unwrap()
}
fn soon() -> Instant {
    Instant::now() + Duration::from_secs(5)
}

#[tokio::test]
async fn egress_resolve_returns_validated_public_addresses_from_the_host_resolver() {
    let source = answer(&[
        "93.184.216.34",
        "2606:2800:220:1:248:1893:25c8:1946",
        "93.184.216.34",
    ]);
    let resolver = Resolver::new(source.clone(), vec![], vec![]).unwrap();
    let resolution = resolver
        .resolve(&authority("example.com", 443), soon())
        .await
        .unwrap();
    assert_eq!(resolution.source, ResolutionSource::HostResolver);
    assert_eq!(resolution.addresses.len(), 2, "duplicates are removed");
    assert_eq!(source.calls.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn egress_resolve_refuses_the_whole_answer_when_any_candidate_is_special() {
    for special in [
        "10.0.0.1",
        "127.0.0.1",
        "169.254.169.254",
        "100.64.0.1",
        "192.168.1.1",
        "::1",
        "::ffff:127.0.0.1",
        "fc00::1",
        "fe80::1",
        "0.0.0.0",
    ] {
        let resolver = Resolver::new(answer(&["93.184.216.34", special]), vec![], vec![]).unwrap();
        assert_eq!(
            resolver
                .resolve(&authority("example.com", 443), soon())
                .await,
            Err(ResolveError::RefusedAddress),
            "{special}"
        );
    }
}

#[tokio::test]
async fn egress_resolve_refuses_an_excluded_control_address_even_when_it_is_public() {
    let control: IpAddr = "203.0.114.10".parse().unwrap();
    let resolver = Resolver::new(answer(&["203.0.114.10"]), vec![], vec![control]).unwrap();
    assert_eq!(
        resolver
            .resolve(&authority("gateway.example.com", 443), soon())
            .await,
        Err(ResolveError::RefusedAddress)
    );
}

#[tokio::test]
async fn egress_resolve_uses_exact_internal_services_without_a_lookup() {
    let source = answer(&["93.184.216.34"]);
    let fixture = authority("fixture.egress.test", 8443);
    let loopback: IpAddr = "127.0.0.1".parse().unwrap();
    let resolver = Resolver::new(
        source.clone(),
        vec![InternalService {
            authority: fixture.clone(),
            addresses: vec![loopback],
        }],
        vec![],
    )
    .unwrap();
    let resolution = resolver.resolve(&fixture, soon()).await.unwrap();
    assert_eq!(
        resolution,
        Resolution {
            addresses: vec![loopback],
            source: ResolutionSource::InternalService
        }
    );
    // The same name on another port is not the declared service.
    assert_eq!(
        resolver
            .resolve(&authority("fixture.egress.test", 443), soon())
            .await
            .map(|r| r.source),
        Ok(ResolutionSource::HostResolver)
    );
    assert_eq!(
        source.calls.load(Ordering::SeqCst),
        1,
        "only the undeclared port was looked up"
    );
}

#[tokio::test]
async fn egress_resolve_bounds_time_count_and_empty_answers() {
    let never = Arc::new(Answer {
        addresses: vec![],
        calls: AtomicUsize::new(0),
        never: true,
    });
    let resolver = Resolver::new(never, vec![], vec![]).unwrap();
    assert_eq!(
        resolver
            .resolve(
                &authority("slow.example.com", 443),
                Instant::now() + Duration::from_millis(50)
            )
            .await,
        Err(ResolveError::Timeout)
    );
    let many: Vec<String> = (1..=17).map(|i| format!("93.184.216.{i}")).collect();
    let many: Vec<&str> = many.iter().map(String::as_str).collect();
    let resolver = Resolver::new(answer(&many), vec![], vec![]).unwrap();
    assert_eq!(
        resolver
            .resolve(&authority("wide.example.com", 443), soon())
            .await,
        Err(ResolveError::Capacity)
    );
    let resolver = Resolver::new(answer(&[]), vec![], vec![]).unwrap();
    assert_eq!(
        resolver
            .resolve(&authority("empty.example.com", 443), soon())
            .await,
        Err(ResolveError::Unavailable)
    );
}

#[test]
fn egress_resolve_refuses_internal_services_that_alias_control_addresses() {
    let control: IpAddr = "127.0.0.1".parse().unwrap();
    let service = InternalService {
        authority: authority("fixture.egress.test", 8443),
        addresses: vec![control],
    };
    assert!(Resolver::new(answer(&[]), vec![service.clone()], vec![control]).is_err());
    assert!(
        Resolver::new(answer(&[]), vec![service.clone(), service], vec![]).is_err(),
        "duplicates refused"
    );
}
