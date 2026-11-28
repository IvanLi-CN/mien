//! Network canary judgement.
//!
//! [`crate::fingerprint::LaunchPlan`] carries the network controls as values plus
//! an [`Enforcement`] level. This module turns live observations into evidence and
//! reconciles that evidence against the claim, so a control is only `Declared`
//! while the observed behaviour supports it. Anything short of positive evidence
//! degrades to `Unverified`; there is no path that upgrades a claim.
//!
//! The functions are pure: callers own the sockets, the browser and the parsing.
//! Three states per judgement keep "not observed" distinct from "observed absent",
//! which is what a leak detector must never conflate.

use crate::fingerprint::Enforcement;

/// Where a DNS query was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuerySource {
    /// Observed by the local canary resolver.
    Canary,
    /// Observed anywhere else; still counts as local resolution.
    External,
}

/// One observed DNS query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedQuery {
    pub name: String,
    pub source: QuerySource,
}

/// Outcome of comparing expected target names with observed queries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsVerdict {
    /// A target name was resolved locally.
    Leaked,
    /// Queries were observed and none named a target.
    NotLeaked,
    /// Nothing was observed; local resolution cannot be ruled in or out.
    Inconclusive,
}

/// Does the observation show a target name resolved locally?
///
/// `expected_names` are the names the browser was told to reach. `observed` are
/// the queries a canary resolver actually received (plus any out-of-band capture).
/// An empty observation set is `Inconclusive`, never `NotLeaked`: a resolver that
/// cannot see queries is not evidence that none happened.
pub fn classify_dns_observation(
    expected_names: &[String],
    observed: &[ClassifiedQuery],
) -> DnsVerdict {
    if observed.is_empty() {
        return DnsVerdict::Inconclusive;
    }
    let expected: Vec<String> = expected_names
        .iter()
        .map(|name| normalize_name(name))
        .collect();
    for query in observed {
        let name = normalize_name(&query.name);
        if !name.is_empty() && expected.contains(&name) {
            return DnsVerdict::Leaked;
        }
    }
    DnsVerdict::NotLeaked
}

fn normalize_name(name: &str) -> String {
    name.trim().trim_end_matches('.').to_ascii_lowercase()
}

/// Outcome of inspecting WebRTC candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebRtcVerdict {
    /// At least one host, server-reflexive or private-reflexive candidate.
    Direct,
    /// Candidates were reported, all of them relay.
    None,
    /// No candidate was reported at all; blocked UDP and an empty gatherer look identical.
    Inconclusive,
}

/// Is any non-proxy candidate present?
///
/// Accepts candidate strings (`candidate:...` or `a=candidate:...` lines) and
/// plain SDP. A `typ host` candidate exposes a local address, `typ srflx`/`prflx`
/// proves the browser reached a STUN server outside the proxy, and any UDP
/// candidate is non-proxied at transport level. `typ relay` is the only case that
/// is compatible with "no non-proxied UDP".
pub fn webrtc_candidates(sdp_like_entries: &[String]) -> WebRtcVerdict {
    let mut parsed = 0usize;
    let mut direct = false;
    for entry in sdp_like_entries {
        let Some(candidate) = candidate_field(entry) else {
            continue;
        };
        parsed += 1;
        let mut tokens = candidate.split_whitespace();
        let _foundation = tokens.next();
        let _component = tokens.next();
        let transport = tokens.next().unwrap_or_default().to_ascii_lowercase();
        let rest: Vec<String> = tokens.map(|token| token.to_ascii_lowercase()).collect();
        let kind = rest
            .iter()
            .position(|token| token == "typ")
            .and_then(|index| rest.get(index + 1))
            .cloned()
            .unwrap_or_default();
        if kind == "relay" {
            continue;
        }
        if matches!(kind.as_str(), "host" | "srflx" | "prflx") || transport == "udp" {
            direct = true;
        }
    }
    if parsed == 0 {
        return WebRtcVerdict::Inconclusive;
    }
    if direct {
        WebRtcVerdict::Direct
    } else {
        WebRtcVerdict::None
    }
}

/// Strip the SDP line prefix and return the candidate fields.
fn candidate_field(entry: &str) -> Option<&str> {
    let entry = entry.trim();
    let entry = entry.strip_prefix("a=").unwrap_or(entry);
    let candidate = entry.strip_prefix("candidate:")?;
    let candidate = candidate.trim();
    if candidate.is_empty() {
        None
    } else {
        Some(candidate)
    }
}

/// Outcome of inspecting the IPv6 egress actually used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ipv6Verdict {
    /// A non-loopback IPv6 address carried traffic.
    Reachable,
    /// The attempt to a non-loopback IPv6 target failed while the host has IPv6.
    Blocked,
    /// Loopback only, or no attempt: no statement about egress.
    Inconclusive,
}

/// Did IPv6 traffic leave the host?
///
/// `observed_route` is the address the peer saw, or the failure description when
/// the peer was never reached. Loopback (`::1`) is not egress and yields
/// `Inconclusive`; callers must pass `None` when the host has no non-loopback IPv6
/// at all instead of reporting a failure as `Blocked`.
pub fn ipv6_reachable(observed_route: Option<&str>) -> Ipv6Verdict {
    let Some(route) = observed_route
        .map(str::trim)
        .filter(|route| !route.is_empty())
    else {
        return Ipv6Verdict::Inconclusive;
    };
    for token in route.split(|ch: char| ch == '[' || ch == ']' || ch == ',' || ch.is_whitespace()) {
        let token = token.trim_matches(|ch: char| ch == '\'' || ch == '"');
        if token.is_empty() {
            continue;
        }
        if let Ok(address) = token.parse::<std::net::Ipv6Addr>() {
            return if address.is_loopback() || address.is_unspecified() {
                Ipv6Verdict::Inconclusive
            } else {
                Ipv6Verdict::Reachable
            };
        }
    }
    Ipv6Verdict::Blocked
}

/// Resolver configuration the browser reports for itself.
///
/// Read from the browser's own netlog (`hostResolverInfo.dns_config`), never from
/// the host resolver: the claim is about the browser process under test.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DohObservation {
    /// `secure_dns_mode`; 0 is off.
    pub secure_dns_mode: Option<u32>,
    /// Number of configured DoH servers.
    pub doh_servers: Option<usize>,
    /// Whether secure DNS transactions may be used at all.
    pub secure_dns_available: Option<bool>,
    /// Whether DNS-over-TLS/DDR is active.
    pub dot_active: Option<bool>,
    /// Secure-DNS policy the browser stamped on the lookups it actually made.
    pub request_policies: Vec<u32>,
}

/// Outcome of reading the resolver configuration in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DohVerdict {
    /// Secure DNS is off and no lookup ran under another policy.
    Disabled,
    /// Secure DNS is configured, available, or a lookup ran under another policy.
    Enabled,
    /// The configuration could not be read.
    Inconclusive,
}

/// Is DNS-over-HTTPS live in the running resolver?
///
/// A configured or available secure path is answered by the resolver
/// configuration. Whether it actually applied is answered by the per-request
/// `secure_dns_policy`: a lookup running under any non-zero policy used secure
/// DNS even if the configuration reads as off. Both inputs missing is
/// `Inconclusive`.
pub fn doh_state(observation: &DohObservation) -> DohVerdict {
    let configured = match (
        observation.secure_dns_mode,
        observation.doh_servers,
        observation.secure_dns_available,
        observation.dot_active,
    ) {
        (Some(mode), Some(servers), Some(secure), Some(dot)) => {
            Some(mode != 0 || servers > 0 || secure || dot)
        }
        _ => None,
    };
    let observed = if observation.request_policies.is_empty() {
        None
    } else {
        Some(
            observation
                .request_policies
                .iter()
                .any(|policy| *policy != 0),
        )
    };
    match (configured, observed) {
        (Some(true), _) | (_, Some(true)) => DohVerdict::Enabled,
        (Some(false), Some(false)) => DohVerdict::Disabled,
        _ => DohVerdict::Inconclusive,
    }
}

/// How the evidence relates to the claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Evidence {
    /// The observation supports the claimed value.
    Consistent,
    /// The observation contradicts the claimed value.
    Contradicted,
    /// The observation does not decide.
    Missing,
}

impl WebRtcVerdict {
    /// `disable_non_proxied_udp=true` is only supported when no direct candidate exists.
    ///
    /// An empty gatherer is `Missing`, not `Consistent`: a kernel that never
    /// produces UDP candidates hides a broken enforcement exactly as well as a
    /// working one, so the value stays unproven.
    pub fn evidence_for_disable_non_proxied_udp(self) -> Evidence {
        match self {
            Self::Direct => Evidence::Contradicted,
            Self::None | Self::Inconclusive => Evidence::Missing,
        }
    }
}

impl Ipv6Verdict {
    /// `ipv6_mode=block` is only supported when a non-loopback IPv6 attempt failed.
    pub fn evidence_for_block(self) -> Evidence {
        match self {
            Self::Reachable => Evidence::Contradicted,
            Self::Blocked => Evidence::Consistent,
            Self::Inconclusive => Evidence::Missing,
        }
    }
}

impl DohVerdict {
    /// `doh_mode=disabled` is supported when the resolver reports no DoH path.
    pub fn evidence_for_disabled(self) -> Evidence {
        match self {
            Self::Disabled => Evidence::Consistent,
            Self::Enabled => Evidence::Contradicted,
            Self::Inconclusive => Evidence::Missing,
        }
    }
}

/// Reconcile a claim with evidence. A claim may only stay `Declared` when the
/// observation supports it; there is deliberately no upgrade path.
pub fn reconcile(claim: Enforcement, evidence: Evidence) -> Enforcement {
    match (claim, evidence) {
        (Enforcement::Unsupported, _) => Enforcement::Unsupported,
        (Enforcement::Declared, Evidence::Consistent) => Enforcement::Declared,
        (Enforcement::Declared, _) | (Enforcement::Unverified, _) => Enforcement::Unverified,
    }
}

/// Resolver configuration as reported by the browser's own netlog.
///
/// `log-net-log` records the resolver state the browser actually built, which is
/// the only place a DoH switch can be observed without trusting argv. The parser
/// accepts the netlog document and returns `None` when the relevant section is
/// absent, so a missing log can never read as "DoH disabled".
pub fn doh_observation_from_netlog(netlog: &str) -> Option<DohObservation> {
    let document: serde_json::Value = serde_json::from_str(netlog).ok()?;
    let dns_config = document
        .get("polledData")?
        .as_array()?
        .iter()
        .find_map(|entry| entry.get("hostResolverInfo")?.get("dns_config"))?;
    Some(DohObservation {
        secure_dns_mode: dns_config
            .get("secure_dns_mode")
            .and_then(serde_json::Value::as_u64)
            .map(|value| value as u32),
        doh_servers: dns_config
            .get("doh_config")
            .and_then(|config| config.get("servers"))
            .and_then(serde_json::Value::as_array)
            .map(Vec::len),
        secure_dns_available: dns_config
            .get("can_use_secure_dns_transactions")
            .and_then(serde_json::Value::as_bool),
        dot_active: dns_config
            .get("dns_over_tls_active")
            .and_then(serde_json::Value::as_bool),
        request_policies: host_resolutions_from_netlog(netlog)
            .into_iter()
            .filter_map(|resolution| resolution.secure_dns_policy)
            .collect(),
    })
}

/// The browser's own view of a host lookup, as the netlog recorded it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostResolution {
    /// Host the browser asked its resolver for.
    pub host: String,
    /// `secure_dns_policy` the request ran under; 0 is off.
    pub secure_dns_policy: Option<u32>,
    /// `net_error` the resolver finished with, when it settled.
    pub net_error: Option<i32>,
}

/// Every host lookup the browser performed, in order.
///
/// The netlog records `HOST_RESOLVER_MANAGER_REQUEST` with the host and the
/// secure-DNS policy, which makes browser-side resolution observable even where
/// the OS resolves out of process. Event names are decoded through the netlog's
/// own `logEventTypes` table because the wire format carries bare numbers.
pub fn host_resolutions_from_netlog(netlog: &str) -> Vec<HostResolution> {
    let Ok(document) = serde_json::from_str::<serde_json::Value>(netlog) else {
        return Vec::new();
    };
    let Some(names) = document
        .get("constants")
        .and_then(|constants| constants.get("logEventTypes"))
        .and_then(|types| types.as_object())
    else {
        return Vec::new();
    };
    let id_of = |name: &str| names.get(name).and_then(serde_json::Value::as_u64);
    let Some(request) = id_of("HOST_RESOLVER_MANAGER_REQUEST") else {
        return Vec::new();
    };
    let Some(events) = document.get("events").and_then(|events| events.as_array()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut pending: Option<usize> = None;
    for event in events {
        if event.get("type").and_then(serde_json::Value::as_u64) != Some(request) {
            continue;
        }
        let Some(params) = event.get("params") else {
            continue;
        };
        if let Some(host) = params.get("host").and_then(serde_json::Value::as_str) {
            out.push(HostResolution {
                host: host.to_owned(),
                secure_dns_policy: params
                    .get("secure_dns_policy")
                    .and_then(serde_json::Value::as_u64)
                    .map(|value| value as u32),
                net_error: None,
            });
            pending = Some(out.len() - 1);
        } else if let (Some(index), Some(error)) = (
            pending,
            params.get("net_error").and_then(serde_json::Value::as_i64),
        ) {
            out[index].net_error = Some(error as i32);
            pending = None;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    fn observed(source: QuerySource, values: &[&str]) -> Vec<ClassifiedQuery> {
        values
            .iter()
            .map(|name| ClassifiedQuery {
                name: (*name).to_owned(),
                source,
            })
            .collect()
    }

    #[test]
    fn network_classify_dns_reports_a_local_target_lookup() {
        let targets = names(&["login.example.test"]);
        let seen = observed(QuerySource::Canary, &["Login.Example.Test."]);
        assert_eq!(
            classify_dns_observation(&targets, &seen),
            DnsVerdict::Leaked
        );
    }

    #[test]
    fn network_classify_dns_accepts_unrelated_queries_as_not_leaked() {
        let targets = names(&["login.example.test"]);
        let seen = observed(QuerySource::External, &["proxy.node.test"]);
        assert_eq!(
            classify_dns_observation(&targets, &seen),
            DnsVerdict::NotLeaked
        );
    }

    #[test]
    fn network_classify_dns_is_inconclusive_without_observations() {
        let targets = names(&["login.example.test"]);
        assert_eq!(
            classify_dns_observation(&targets, &[]),
            DnsVerdict::Inconclusive
        );
    }

    #[test]
    fn network_webrtc_detects_host_and_reflexive_candidates() {
        let host =
            vec!["candidate:1 1 udp 2122260223 192.0.2.10 54321 typ host generation 0".to_owned()];
        assert_eq!(webrtc_candidates(&host), WebRtcVerdict::Direct);
        let reflexive = vec![
            "a=candidate:2 1 udp 1686052607 198.51.100.7 61234 typ srflx raddr 192.0.2.10 rport 54321".to_owned(),
        ];
        assert_eq!(webrtc_candidates(&reflexive), WebRtcVerdict::Direct);
        let mdns = vec![
            "candidate:3 1 udp 2113937151 5d4e1f0b-1111-2222-3333-444455556666.local 50123 typ host".to_owned(),
        ];
        assert_eq!(webrtc_candidates(&mdns), WebRtcVerdict::Direct);
    }

    #[test]
    fn network_webrtc_treats_relay_only_as_no_direct_candidate() {
        let relay = vec![
            "candidate:4 1 udp 41885439 203.0.113.9 443 typ relay raddr 0.0.0.0 rport 0".to_owned(),
        ];
        assert_eq!(webrtc_candidates(&relay), WebRtcVerdict::None);
    }

    #[test]
    fn network_webrtc_is_inconclusive_for_an_empty_gather() {
        assert_eq!(webrtc_candidates(&[]), WebRtcVerdict::Inconclusive);
        assert_eq!(
            webrtc_candidates(&["v=0".to_owned(), "m=application 9 UDP/DTLS/SCTP".to_owned()]),
            WebRtcVerdict::Inconclusive
        );
    }

    #[test]
    fn network_ipv6_reports_global_egress() {
        assert_eq!(
            ipv6_reachable(Some("2001:db8:1::20")),
            Ipv6Verdict::Reachable
        );
        assert_eq!(
            ipv6_reachable(Some("peer=[2001:db8:1::20]")),
            Ipv6Verdict::Reachable
        );
    }

    #[test]
    fn network_ipv6_reports_a_blocked_attempt() {
        assert_eq!(
            ipv6_reachable(Some("connect error: EHOSTUNREACH")),
            Ipv6Verdict::Blocked
        );
    }

    #[test]
    fn network_ipv6_is_inconclusive_for_loopback_or_no_attempt() {
        assert_eq!(ipv6_reachable(Some("::1")), Ipv6Verdict::Inconclusive);
        assert_eq!(ipv6_reachable(Some("   ")), Ipv6Verdict::Inconclusive);
        assert_eq!(ipv6_reachable(None), Ipv6Verdict::Inconclusive);
    }

    fn observation(
        mode: Option<u32>,
        servers: Option<usize>,
        secure: Option<bool>,
        dot: Option<bool>,
        policies: &[u32],
    ) -> DohObservation {
        DohObservation {
            secure_dns_mode: mode,
            doh_servers: servers,
            secure_dns_available: secure,
            dot_active: dot,
            request_policies: policies.to_vec(),
        }
    }

    #[test]
    fn network_doh_states_cover_all_three_outcomes() {
        assert_eq!(
            doh_state(&observation(
                Some(0),
                Some(0),
                Some(false),
                Some(false),
                &[0, 0]
            )),
            DohVerdict::Disabled
        );
        assert_eq!(
            doh_state(&observation(
                Some(2),
                Some(1),
                Some(true),
                Some(false),
                &[2]
            )),
            DohVerdict::Enabled
        );
        assert_eq!(
            doh_state(&observation(Some(0), Some(0), None, Some(false), &[0])),
            DohVerdict::Inconclusive
        );
    }

    #[test]
    fn network_doh_reads_each_secure_signal() {
        for ob in [
            observation(Some(1), Some(0), Some(false), Some(false), &[0]),
            observation(Some(0), Some(1), Some(false), Some(false), &[0]),
            observation(Some(0), Some(0), Some(true), Some(false), &[0]),
            observation(Some(0), Some(0), Some(false), Some(true), &[0]),
            observation(Some(0), Some(0), Some(false), Some(false), &[0, 2]),
        ] {
            assert_eq!(doh_state(&ob), DohVerdict::Enabled, "{ob:?}");
        }
    }

    #[test]
    fn network_doh_needs_the_lookups_too_when_the_config_reads_off() {
        // 配置说 off，但一次查询就落在 secure 策略上：以实际查询为准。
        assert_eq!(
            doh_state(&observation(
                Some(0),
                Some(0),
                Some(false),
                Some(false),
                &[2]
            )),
            DohVerdict::Enabled
        );
        // 配置和查询都读不到。
        assert_eq!(
            doh_state(&observation(None, None, None, None, &[])),
            DohVerdict::Inconclusive
        );
        // 配置可用但还没查询：仅凭配置即 Enabled，不是 Inconclusive。
        assert_eq!(
            doh_state(&observation(Some(2), Some(1), Some(true), Some(false), &[])),
            DohVerdict::Enabled
        );
    }

    #[test]
    fn network_host_resolutions_decode_names_and_settlements() {
        let netlog = r#"{
          "constants": {"logEventTypes": {"HOST_RESOLVER_MANAGER_REQUEST": 7, "OTHER": 9}},
          "events": [
            {"type": 7, "params": {"host": "a.canary.test", "secure_dns_policy": 0}},
            {"type": 7, "params": {"net_error": -105}},
            {"type": 7, "params": {"host": "b.canary.test", "secure_dns_policy": 2}},
            {"type": 9, "params": {"host": "ignored.test"}},
            {"type": 7, "params": {"net_error": 0}}
          ]
        }"#;
        let resolutions = host_resolutions_from_netlog(netlog);
        assert_eq!(resolutions.len(), 2);
        assert_eq!(resolutions[0].host, "a.canary.test");
        assert_eq!(resolutions[0].net_error, Some(-105));
        assert_eq!(resolutions[1].secure_dns_policy, Some(2));
        assert!(host_resolutions_from_netlog("not json").is_empty());
    }

    #[test]
    fn network_reconcile_refuses_to_upgrade_a_claim() {
        assert_eq!(
            reconcile(Enforcement::Declared, Evidence::Consistent),
            Enforcement::Declared
        );
        assert_eq!(
            reconcile(Enforcement::Declared, Evidence::Contradicted),
            Enforcement::Unverified
        );
        assert_eq!(
            reconcile(Enforcement::Declared, Evidence::Missing),
            Enforcement::Unverified
        );
        assert_eq!(
            reconcile(Enforcement::Unverified, Evidence::Consistent),
            Enforcement::Unverified
        );
        assert_eq!(
            reconcile(Enforcement::Unsupported, Evidence::Consistent),
            Enforcement::Unsupported
        );
    }

    #[test]
    fn network_verdict_evidence_never_claims_consistency_from_silence() {
        assert_eq!(
            WebRtcVerdict::Inconclusive.evidence_for_disable_non_proxied_udp(),
            Evidence::Missing
        );
        assert_eq!(
            WebRtcVerdict::None.evidence_for_disable_non_proxied_udp(),
            Evidence::Missing
        );
        assert_eq!(
            WebRtcVerdict::Direct.evidence_for_disable_non_proxied_udp(),
            Evidence::Contradicted
        );
        assert_eq!(
            Ipv6Verdict::Reachable.evidence_for_block(),
            Evidence::Contradicted
        );
        assert_eq!(
            DohVerdict::Disabled.evidence_for_disabled(),
            Evidence::Consistent
        );
    }
}
