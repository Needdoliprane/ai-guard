//! Test-first intent for the local policy core. No socket, resolver or runtime.

use super::*;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

fn authority(name: &str, port: u16) -> Authority {
    Authority::new(name, port, EgressTransport::ConnectTcpOpaque).expect("fixture authority")
}

fn policy(revision: u64, names: &[(&str, u16)]) -> EgressPolicyV1 {
    let destinations: Vec<_> = names
        .iter()
        .map(|(name, port)| authority(name, *port))
        .collect();
    EgressPolicyV1::new(revision, 100, 1_000, &destinations, PolicyLimits::default())
        .expect("fixture policy")
}

fn allowed(decision: PolicyDecision) -> PolicyAllowed {
    match decision {
        PolicyDecision::PolicyAllowed(value) => value,
        other => panic!("expected descriptive policy allowance, got {other:?}"),
    }
}

#[test]
fn connect_authority_has_one_exact_ascii_name_and_explicit_port() {
    let parsed = Authority::parse_connect("API.Example.COM:443").expect("valid authority");
    assert_eq!(parsed.name(), "api.example.com");
    assert_eq!(parsed.port(), 443);
    assert_eq!(parsed.transport(), EgressTransport::ConnectTcpOpaque);
    assert_eq!(parsed, authority("api.example.com", 443));
    assert_eq!(
        Authority::parse_connect("api.example.com:65535")
            .expect("max port")
            .port(),
        65535
    );
}

#[test]
fn authority_rejects_urls_credentials_ip_spellings_and_dns_alias_heuristics() {
    // Removing any corresponding parsing guard must admit at least one row.
    for input in [
        "",
        "example.com",
        "example.com:",
        "example.com:0",
        "example.com:65536",
        "example.com:0443",
        "example.com:+443",
        "example.com: 443",
        "example.com:443 ",
        "https://example.com:443",
        "user:password@example.com:443",
        "example.com:443/path",
        "example.com:443?token=synthetic",
        "example.com:443#fragment",
        "example.com.:443",
        "*.example.com:443",
        ".example.com:443",
        "example..com:443",
        "localhost:443",
        "-bad.example:443",
        "bad-.example:443",
        "bad_name.example:443",
        "éxample.com:443",
        "example.com\r\n:443",
        "example.com\0:443",
        "127.0.0.1:443",
        "127.1:443",
        "0177.0.0.1:443",
        "2130706433:443",
        "0x7f000001:443",
        "[::1]:443",
        "[::ffff:127.0.0.1]:443",
        "::1:443",
        "example.123:443",
        "api%2eexample.com:443",
    ] {
        assert!(
            Authority::parse_connect(input).is_err(),
            "accepted a rejected fixture row"
        );
    }
    assert!(Authority::new("example.com", 0, EgressTransport::ConnectTcpOpaque).is_err());
}

#[test]
fn authority_bounds_are_checked_without_truncation() {
    let label = "a".repeat(63);
    let max_name = format!("{label}.{label}.{label}.{}", "b".repeat(61));
    assert_eq!(max_name.len(), 253);
    assert!(Authority::new(&max_name, 443, EgressTransport::ConnectTcpOpaque).is_ok());
    let too_long = format!("{max_name}b");
    assert!(Authority::new(&too_long, 443, EgressTransport::ConnectTcpOpaque).is_err());
    assert!(Authority::parse_connect(&format!("{}.example:443", "a".repeat(64))).is_err());
    assert!(Authority::parse_connect(&"a".repeat(513)).is_err());
}

#[test]
fn invalid_authority_diagnostics_never_echo_untrusted_input() {
    let secret_fixture = "synthetic-private-value@example.com:443";
    let error = Authority::parse_connect(secret_fixture).expect_err("userinfo must fail");
    assert!(!format!("{error:?} {error}").contains("synthetic-private-value"));
    let valid_but_sensitive = authority("synthetic-private-value.example", 443);
    assert!(!format!("{valid_but_sensitive:?}").contains("synthetic-private-value"));
}

#[test]
fn empty_policies_deny_and_both_policies_must_allow_the_exact_destination() {
    let empty = policy(1, &[]);
    let tenant = policy(2, &[("api.example.com", 443), ("tenant-only.example", 443)]);
    let session = policy(
        3,
        &[("api.example.com", 443), ("session-only.example", 443)],
    );
    let target = authority("api.example.com", 443);
    assert!(matches!(
        empty.evaluate(&session, &target, 100),
        PolicyDecision::Denied(_)
    ));
    assert!(matches!(
        tenant.evaluate(&empty, &target, 100),
        PolicyDecision::Denied(_)
    ));
    let result = allowed(tenant.evaluate(&session, &target, 100));
    assert_eq!(result.authority(), &target);
    assert_eq!(result.tenant_revision(), 2);
    assert_eq!(result.session_revision(), 3);
    assert_eq!(result.tenant_digest(), tenant.digest());
    assert_eq!(result.session_digest(), session.digest());
    assert!(matches!(
        tenant.evaluate(&session, &authority("tenant-only.example", 443), 100),
        PolicyDecision::Denied(DenyReason::DestinationNotAllowedBySession)
    ));
    assert!(matches!(
        tenant.evaluate(&session, &authority("session-only.example", 443), 100),
        PolicyDecision::Denied(DenyReason::DestinationNotAllowedByTenant)
    ));
    for destination in [
        authority("sub.api.example.com", 443),
        authority("api.example.com", 8443),
    ] {
        assert!(matches!(
            tenant.evaluate(&session, &destination, 100),
            PolicyDecision::Denied(_)
        ));
    }
}

#[test]
fn policy_validity_is_half_open_and_does_not_refresh_from_evaluation_time() {
    let target = authority("api.example.com", 443);
    let tenant = EgressPolicyV1::new(
        1,
        100,
        1_000,
        std::slice::from_ref(&target),
        PolicyLimits::default(),
    )
    .expect("tenant");
    let session = EgressPolicyV1::new(
        2,
        200,
        500,
        std::slice::from_ref(&target),
        PolicyLimits::default(),
    )
    .expect("session");
    assert!(matches!(
        tenant.evaluate(&session, &target, 99),
        PolicyDecision::Denied(DenyReason::TenantInactive)
    ));
    assert!(matches!(
        tenant.evaluate(&session, &target, 199),
        PolicyDecision::Denied(DenyReason::SessionInactive)
    ));
    assert_eq!(
        allowed(tenant.evaluate(&session, &target, 200)).valid_until_ms(),
        500
    );
    assert_eq!(
        allowed(tenant.evaluate(&session, &target, 499)).valid_until_ms(),
        500
    );
    assert!(matches!(
        tenant.evaluate(&session, &target, 500),
        PolicyDecision::Denied(DenyReason::SessionInactive)
    ));
    assert!(matches!(
        tenant.evaluate(&session, &target, 1_000),
        PolicyDecision::Denied(DenyReason::TenantInactive)
    ));
}

#[test]
fn policy_construction_rejects_ambiguous_revisions_intervals_duplicates_and_oversize() {
    let target = authority("api.example.com", 443);
    assert!(EgressPolicyV1::new(0, 100, 200, &[], PolicyLimits::default()).is_err());
    for (from, until) in [(100, 100), (200, 100)] {
        assert!(EgressPolicyV1::new(1, from, until, &[], PolicyLimits::default()).is_err());
    }
    assert!(
        EgressPolicyV1::new(
            1,
            100,
            200,
            &[target.clone(), target],
            PolicyLimits::default()
        )
        .is_err()
    );
    let rows: Vec<_> = (0..129)
        .map(|n| authority(&format!("h{n}.example"), 443))
        .collect();
    assert!(EgressPolicyV1::new(1, 100, 200, &rows[..128], PolicyLimits::default()).is_ok());
    assert!(EgressPolicyV1::new(1, 100, 200, &rows, PolicyLimits::default()).is_err());
    let restrictive = PolicyLimits::new(1, 1).expect("small limits");
    assert!(EgressPolicyV1::new(1, 100, 200, &rows[..2], restrictive).is_err());
}

#[test]
fn limits_cannot_exceed_the_envelope_and_session_cannot_widen_tenant_limits() {
    for (destinations, addresses) in [(0, 1), (1, 0), (129, 1), (1, 17), (usize::MAX, usize::MAX)] {
        assert!(PolicyLimits::new(destinations, addresses).is_err());
    }
    let target = authority("api.example.com", 443);
    let tenant = EgressPolicyV1::new(
        1,
        100,
        200,
        std::slice::from_ref(&target),
        PolicyLimits::new(2, 3).expect("tenant limits"),
    )
    .expect("tenant");
    let session = EgressPolicyV1::new(
        2,
        100,
        200,
        std::slice::from_ref(&target),
        PolicyLimits::new(1, 8).expect("session limits"),
    )
    .expect("session");
    let effective = allowed(tenant.evaluate(&session, &target, 100)).limits();
    assert_eq!(effective.max_destinations(), 1);
    assert_eq!(effective.max_resolution_ips(), 3);
}

#[test]
fn policy_digest_is_canonical_and_binds_revision_validity_destinations_and_limits() {
    let a = authority("a.example", 443);
    let b = authority("b.example", 8443);
    let base = EgressPolicyV1::new(
        1,
        100,
        200,
        &[a.clone(), b.clone()],
        PolicyLimits::default(),
    )
    .expect("base");
    let reordered = EgressPolicyV1::new(
        1,
        100,
        200,
        &[b.clone(), a.clone()],
        PolicyLimits::default(),
    )
    .expect("reordered");
    assert_eq!(base.digest(), reordered.digest());
    for changed in [
        EgressPolicyV1::new(
            2,
            100,
            200,
            &[a.clone(), b.clone()],
            PolicyLimits::default(),
        ),
        EgressPolicyV1::new(1, 99, 200, &[a.clone(), b.clone()], PolicyLimits::default()),
        EgressPolicyV1::new(
            1,
            100,
            201,
            &[a.clone(), b.clone()],
            PolicyLimits::default(),
        ),
        EgressPolicyV1::new(1, 100, 200, &[a.clone()], PolicyLimits::default()),
        EgressPolicyV1::new(
            1,
            100,
            200,
            &[a, b],
            PolicyLimits::new(128, 15).expect("limits"),
        ),
    ] {
        assert_ne!(base.digest(), changed.expect("changed").digest());
    }
}

fn ip(raw: &str) -> IpAddr {
    raw.parse().expect("fixture IP")
}

#[test]
fn resolution_accepts_only_a_bounded_set_of_public_candidates_without_performing_dns() {
    let target = authority("api.example.com", 443);
    let resolution = ValidatedResolution::new(
        &target,
        &[ip("8.8.8.8"), ip("2606:4700:4700::1111"), ip("8.8.8.8")],
        PolicyLimits::default(),
    )
    .expect("public fixtures");
    assert_eq!(resolution.authority(), &target);
    assert_eq!(
        resolution.addresses(),
        &[ip("8.8.8.8"), ip("2606:4700:4700::1111")]
    );
    assert_eq!(
        ValidatedResolution::new(&target, &[], PolicyLimits::default()).expect_err("empty"),
        ResolutionError::Empty
    );
    assert_eq!(
        ValidatedResolution::new(&target, &[ip("8.8.8.8"); 17], PolicyLimits::default())
            .expect_err("bound before dedup"),
        ResolutionError::TooManyAddresses
    );
    assert!(
        ValidatedResolution::new(
            &target,
            &[ip("8.8.8.8"), ip("1.1.1.1")],
            PolicyLimits::new(1, 1).expect("limit")
        )
        .is_err()
    );
}

#[test]
fn one_special_or_local_candidate_rejects_the_complete_resolution() {
    let target = authority("api.example.com", 443);
    for raw in [
        "0.0.0.0",
        "0.1.2.3",
        "10.0.0.1",
        "100.64.0.1",
        "100.127.255.255",
        "127.0.0.1",
        "169.254.169.254",
        "169.254.170.2",
        "172.16.0.1",
        "172.31.255.255",
        "192.0.0.1",
        "192.0.0.9",
        "192.0.2.1",
        "192.31.196.1",
        "192.52.193.1",
        "192.88.99.1",
        "192.168.0.1",
        "192.175.48.1",
        "198.18.0.1",
        "198.19.255.255",
        "198.51.100.1",
        "203.0.113.1",
        "224.0.0.1",
        "239.255.255.255",
        "240.0.0.1",
        "255.255.255.255",
        "::",
        "::1",
        "::ffff:8.8.8.8",
        "::ffff:127.0.0.1",
        "64:ff9b::808:808",
        "64:ff9b:1::808:808",
        "100::1",
        "2001::1",
        "2001:2::1",
        "2001:10::1",
        "2001:20::1",
        "2001:db8::1",
        "2002:808:808::1",
        "2620:4f:8000::1",
        "3fff::1",
        "fc00::1",
        "fe80::1",
        "ff02::1",
    ] {
        let forbidden = ip(raw);
        assert_eq!(
            ValidatedResolution::new(
                &target,
                &[ip("8.8.8.8"), forbidden],
                PolicyLimits::default()
            )
            .expect_err("mixed set must fail"),
            ResolutionError::NonPublicAddress,
            "fixture {raw}"
        );
    }
}

#[test]
fn ipv4_and_ipv6_prefix_boundaries_do_not_expand_local_ranges_or_drop_public_neighbors() {
    let target = authority("api.example.com", 443);
    for addr in [
        Ipv4Addr::new(100, 63, 255, 255).into(),
        Ipv4Addr::new(100, 128, 0, 0).into(),
        Ipv4Addr::new(172, 15, 255, 255).into(),
        Ipv4Addr::new(172, 32, 0, 0).into(),
        Ipv4Addr::new(198, 17, 255, 255).into(),
        Ipv4Addr::new(198, 20, 0, 0).into(),
        Ipv6Addr::new(0x2000, 0xffff, 0, 0, 0, 0, 0, 1).into(),
        Ipv6Addr::new(0x2001, 0x200, 0, 0, 0, 0, 0, 1).into(),
        Ipv6Addr::new(0x3fff, 0x1000, 0, 0, 0, 0, 0, 1).into(),
    ] {
        assert!(
            ValidatedResolution::new(&target, &[addr], PolicyLimits::default()).is_ok(),
            "public neighbor fixture {addr}"
        );
    }
}

// ADDENDUM after the first implementation, authorized by Root. The original
// twelve intent tests remain retained byte-for-byte in the TEST_ONLY snapshot.
#[test]
fn authority_rejects_legacy_mixed_radix_ipv4_without_rejecting_numeric_dns_labels() {
    for raw in [
        "0x7f.0x1:443",
        "127.0x1:443",
        "0177.0x0.0.0x1:443",
        "0X7f.0X1:443",
        "127.0x0000001:443",
    ] {
        assert!(Authority::parse_connect(raw).is_err());
    }
    for raw in [
        "0x7f.example:443",
        "v1.api.example:443",
        "127.api.example:443",
    ] {
        assert!(Authority::parse_connect(raw).is_ok());
    }
}

#[test]
fn ipv6_as112_documentation_and_ietf_prefix_edges_reject_the_entire_special_range() {
    let target = authority("api.example.com", 443);
    for (raw, accepted) in [
        ("1fff:ffff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("2001:1ff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("2001:200::1", true),
        ("2620:4f:7fff:ffff:ffff:ffff:ffff:ffff", true),
        ("2620:4f:8000::", false),
        ("2620:4f:8000:ffff:ffff:ffff:ffff:ffff", false),
        ("2620:4f:8001::", true),
        ("3ffe:ffff:ffff:ffff:ffff:ffff:ffff:ffff", true),
        ("3fff::", false),
        ("3fff:fff:ffff:ffff:ffff:ffff:ffff:ffff", false),
        ("3fff:1000::", true),
        ("4000::", false),
    ] {
        assert_eq!(
            ValidatedResolution::new(&target, &[ip(raw)], PolicyLimits::default()).is_ok(),
            accepted,
            "prefix-edge fixture {raw}",
        );
    }
}
