use super::*;
use std::sync::Arc;

fn key(host: &str, addresses: &[&str], allow_private: bool) -> ClientKey {
    ClientKey::new(
        Profile::Stream,
        host,
        &addresses
            .iter()
            .map(|address| address.parse().unwrap())
            .collect::<Vec<_>>(),
        allow_private,
    )
}

#[test]
fn repeated_validated_destinations_share_one_client_build() {
    let mut cache = ClientCache::default();
    let now = Instant::now();
    let mut builds = 0;
    for _ in 0..3 {
        let value = cache
            .get_or_insert_with(key("api.example.com", &["1.1.1.1"], false), now, || {
                builds += 1;
                Ok(7)
            })
            .unwrap();
        assert_eq!(value, 7);
    }
    assert_eq!(builds, 1);
    assert_eq!(cache.entries.len(), 1);
}

#[test]
fn address_order_duplicates_and_host_case_do_not_split_a_pool() {
    assert_eq!(
        key("API.Example.com", &["1.1.1.1", "8.8.8.8"], false),
        key("api.example.com", &["8.8.8.8", "1.1.1.1", "1.1.1.1"], false)
    );
}

#[test]
fn changed_pins_permissions_hosts_and_profiles_never_reuse_a_client() {
    let mut cache = ClientCache::default();
    let first = key("api.example.com", &["1.1.1.1"], false);
    let mut ping = first.clone();
    ping.profile = Profile::Ping;
    let keys = [
        first,
        key("api.example.com", &["8.8.8.8"], false),
        key("api.example.com", &["1.1.1.1"], true),
        key("other.example.com", &["1.1.1.1"], false),
        ping,
    ];
    for (index, key) in keys.into_iter().enumerate() {
        assert_eq!(
            cache
                .get_or_insert_with(key, Instant::now(), || Ok(index))
                .unwrap(),
            index
        );
    }
    assert_eq!(cache.entries.len(), 5);
}

#[test]
fn idle_ttl_is_refreshed_on_use_and_expires_at_the_boundary() {
    let mut cache = ClientCache::default();
    let k = key("api.example.com", &["1.1.1.1"], false);
    let now = Instant::now();
    cache.get_or_insert_with(k.clone(), now, || Ok(1)).unwrap();
    let used = now + IDLE_TTL - Duration::from_secs(1);
    assert_eq!(
        cache.get_or_insert_with(k.clone(), used, || Ok(2)).unwrap(),
        1
    );
    assert_eq!(
        cache
            .get_or_insert_with(k, used + IDLE_TTL, || Ok(3))
            .unwrap(),
        3
    );
}

#[test]
fn lru_evicts_the_least_recently_used_not_the_oldest_built() {
    let mut cache = ClientCache::default();
    let now = Instant::now();
    for index in 0..MAX_CLIENTS {
        cache
            .get_or_insert_with(
                key(&format!("h{index}.example.com"), &["1.1.1.1"], false),
                now + Duration::from_secs(index as u64),
                || Ok(index),
            )
            .unwrap();
    }
    let oldest = key("h0.example.com", &["1.1.1.1"], false);
    cache
        .get_or_insert_with(oldest.clone(), now + Duration::from_secs(10), || Ok(99))
        .unwrap();
    cache
        .get_or_insert_with(
            key("extra.example.com", &["1.1.1.1"], false),
            now + Duration::from_secs(11),
            || Ok(100),
        )
        .unwrap();
    assert_eq!(cache.entries.len(), MAX_CLIENTS);
    assert!(cache.entries.iter().any(|entry| entry.key == oldest));
    assert!(cache
        .entries
        .iter()
        .all(|entry| entry.key.host != "h1.example.com"));
}

#[test]
fn failed_builds_are_not_cached_or_allowed_to_evict_live_clients() {
    let mut cache = ClientCache::default();
    let now = Instant::now();
    for index in 0..MAX_CLIENTS {
        cache
            .get_or_insert_with(
                key(&format!("h{index}.example.com"), &["1.1.1.1"], false),
                now,
                || Ok(index),
            )
            .unwrap();
    }
    let k = key("failed.example.com", &["1.1.1.1"], false);
    assert!(cache
        .get_or_insert_with(k.clone(), now, || Err("build failed".into()))
        .is_err());
    assert_eq!(cache.entries.len(), MAX_CLIENTS);
    assert!(cache.entries.iter().all(|entry| entry.key != k));
    assert_eq!(cache.get_or_insert_with(k, now, || Ok(42)).unwrap(), 42);
}

#[test]
fn eviction_only_drops_the_cache_reference_not_in_flight_clients() {
    let mut cache = ClientCache::default();
    let now = Instant::now();
    let original = Arc::new(17);
    let held = cache
        .get_or_insert_with(
            key("original.example.com", &["1.1.1.1"], false),
            now,
            || Ok(original.clone()),
        )
        .unwrap();
    for index in 0..MAX_CLIENTS {
        cache
            .get_or_insert_with(
                key(&format!("h{index}.example.com"), &["1.1.1.1"], false),
                now + Duration::from_secs(index as u64 + 1),
                || Ok(Arc::new(index)),
            )
            .unwrap();
    }
    assert_eq!(*held, 17);
    assert_eq!(Arc::strong_count(&original), 2);
}

#[test]
fn empty_pins_are_rejected_before_client_creation() {
    assert!(client_for(Profile::Stream, "api.example.com", &[], false).is_err());
}

#[test]
fn stream_and_ping_clients_build_with_their_pinned_profiles() {
    build_client(&key(
        "api.example.com",
        &["1.1.1.1", "2606:4700::1111"],
        false,
    ))
    .unwrap();
    let mut ping = key("localhost", &["127.0.0.1"], true);
    ping.profile = Profile::Ping;
    build_client(&ping).unwrap();
}
