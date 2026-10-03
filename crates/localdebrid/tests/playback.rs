mod common;

use std::time::Duration;

use common::{Harness, MOVIE_ID, eventually};
use localdebrid_testkit::{pattern_bytes, torrent_stream, write_pattern_file, write_season_pack};
use serde_json::json;

const MIB: u64 = 1024 * 1024;

async fn movie_harness(len: u64) -> (Harness, localdebrid_testkit::Seeder, String) {
    let h = Harness::new(|_| {}).await;
    let file = h.content_dir().join("Free.Movie.2020.1080p.WEB.x264.mkv");
    write_pattern_file(&file, len, 1).unwrap();
    let seeder = h.seed(&file).await;
    h.addon.set(
        "movie",
        MOVIE_ID,
        json!([torrent_stream(&seeder.info_hash, None, "Free.Movie.2020.1080p.WEB.x264", &h.tracker.announce_url)]),
    );
    let urls = h.stream_urls("movie", MOVIE_ID).await;
    assert_eq!(urls.len(), 1);
    (h, seeder, urls[0].clone())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn manifest_follows_the_stremio_protocol() {
    let h = Harness::new(|_| {}).await;
    let response = h.client.get(format!("{}/manifest.json", h.base)).send().await.unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.headers()["access-control-allow-origin"],
        "*",
        "Stremio requires CORS on every addon route"
    );
    let manifest: serde_json::Value = response.json().await.unwrap();
    for field in ["id", "version", "name", "description", "resources", "types", "catalogs"] {
        assert!(manifest.get(field).is_some(), "missing {field}");
    }
    assert_eq!(manifest["resources"][0]["name"], "stream");
    assert_eq!(manifest["catalogs"], json!([]));
    assert!(manifest["types"].as_array().unwrap().contains(&json!("series")));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stream_response_contains_direct_http_url_not_info_hash() {
    let (h, _seeder, url) = movie_harness(2 * MIB).await;
    let (_, body) = h.get_json(&format!("/stream/movie/{MOVIE_ID}.json")).await;
    let stream = &body["streams"][0];
    assert!(stream.get("infoHash").is_none());
    assert!(url.starts_with(&h.base), "{url}");
    assert!(url.ends_with(".mkv"), "player infers the container from the extension: {url}");
    assert_eq!(stream["behaviorHints"]["notWebReady"], true);
    assert!(stream["name"].as_str().unwrap().starts_with("Local Debrid"));
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn full_download_and_head_request() {
    let len = 3 * MIB + 12345;
    let (h, _seeder, url) = movie_harness(len).await;

    let head = h.client.head(&url).send().await.unwrap();
    assert_eq!(head.status(), 200);
    assert_eq!(head.headers()["content-length"], len.to_string().as_str());
    assert_eq!(head.headers()["accept-ranges"], "bytes");
    assert_eq!(head.headers()["content-type"], "video/x-matroska");

    let response = h.get_range(&url, None).await;
    assert_eq!(response.status(), 200);
    let body = response.bytes().await.unwrap();
    assert_eq!(body.len() as u64, len);
    assert!(body.as_ref() == pattern_bytes(1, 0, len as usize).as_slice());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn range_requests_and_seeking() {
    let len = 8 * MIB;
    let (h, _seeder, url) = movie_harness(len).await;
    let cases: &[(&str, u64, u64)] = &[
        ("bytes=0-0", 0, 0),
        ("bytes=0-1023", 0, 1023),
        ("bytes=5000000-5100000", 5_000_000, 5_100_000),
        ("bytes=8000000-", 8_000_000, len - 1),
        ("bytes=-65536", len - 65536, len - 1),
        ("bytes=100-99999999", 100, len - 1),
    ];
    for (range, start, end) in cases {
        let response = h.get_range(&url, Some(range)).await;
        assert_eq!(response.status(), 206, "{range}");
        assert_eq!(
            response.headers()["content-range"],
            format!("bytes {start}-{end}/{len}").as_str(),
            "{range}"
        );
        let body = response.bytes().await.unwrap();
        assert_eq!(body.len() as u64, end - start + 1, "{range}");
        assert!(
            body.as_ref() == pattern_bytes(1, *start, body.len()).as_slice(),
            "wrong bytes for {range}"
        );
    }

    let response = h.get_range(&url, Some(&format!("bytes={len}-"))).await;
    assert_eq!(response.status(), 416);
    assert_eq!(response.headers()["content-range"], format!("bytes */{len}").as_str());

    // Backwards seek after a forward one, as a player does when the user rewinds.
    for start in [6 * MIB, MIB, 7 * MIB + 3, 0] {
        let response = h.get_range(&url, Some(&format!("bytes={start}-{}", start + 4095))).await;
        let body = response.bytes().await.unwrap();
        assert!(body.as_ref() == pattern_bytes(1, start, 4096).as_slice());
    }
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_readers_share_one_session() {
    let len = 6 * MIB;
    let (h, _seeder, url) = movie_harness(len).await;
    let mut tasks = Vec::new();
    for i in 0..8u64 {
        let client = h.client.clone();
        let url = url.clone();
        tasks.push(tokio::spawn(async move {
            let start = (i * 700_001) % (len - 100_000);
            let body = client
                .get(&url)
                .header("Range", format!("bytes={start}-{}", start + 99_999))
                .send()
                .await
                .unwrap()
                .bytes()
                .await
                .unwrap();
            assert!(body.as_ref() == pattern_bytes(1, start, 100_000).as_slice());
        }));
    }
    for t in tasks {
        t.await.unwrap();
    }
    assert_eq!(h.state().sessions.list().len(), 1);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn client_disconnecting_mid_stream_releases_the_reader() {
    let len = 16 * MIB;
    let (h, _seeder, url) = movie_harness(len).await;
    let mut response = h.get_range(&url, None).await;
    let first = response.chunk().await.unwrap().unwrap();
    assert!(!first.is_empty());
    let session = h.state().sessions.list()[0].clone();
    assert_eq!(session.active_readers(), 1);
    drop(response);
    assert!(eventually(Duration::from_secs(5), || session.active_readers() == 0).await);

    let body = h.get_range(&url, Some("bytes=1000-1999")).await.bytes().await.unwrap();
    assert!(body.as_ref() == pattern_bytes(1, 1000, 1000).as_slice());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn season_pack_streams_the_requested_episode() {
    let h = Harness::new(|_| {}).await;
    let episode_len = 2 * MIB;
    let episodes = write_season_pack(&h.content_dir(), episode_len).unwrap();
    let pack = episodes[0].parent().unwrap();
    let seeder = h.seed(pack).await;
    h.addon.set(
        "series",
        "tt0000002:1:2",
        json!([torrent_stream(&seeder.info_hash, None, "Show.S01.1080p.WEB.x264", &h.tracker.announce_url)]),
    );
    let urls = h.stream_urls("series", "tt0000002:1:2").await;
    assert_eq!(urls.len(), 1);
    assert!(urls[0].contains("/s1e2/"), "{}", urls[0]);

    let response = h.get_range(&urls[0], None).await;
    assert_eq!(response.status(), 200);
    let body = response.bytes().await.unwrap();
    let expected_len = episode_len + 2000;
    assert_eq!(body.len() as u64, expected_len);
    assert!(body.as_ref() == pattern_bytes(102, 0, expected_len as usize).as_slice());
    let session = &h.state().sessions.list()[0];
    assert!(session.file_path.ends_with("Show.S01E02.1080p.WEB.x264.mkv"), "{}", session.file_path);

    let (_, listing) = h.get_json("/stream/series/tt0000002:1:2.json").await;
    let hints = &listing["streams"][0]["behaviorHints"];
    assert_eq!(hints["videoSize"], expected_len, "known file size is reported after first play");
    assert_eq!(hints["filename"], "Show.S01E02.1080p.WEB.x264.mkv");
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn video_hash_is_computed_and_published() {
    let len = 3 * MIB;
    let (h, _seeder, url) = movie_harness(len).await;
    assert_eq!(h.get_range(&url, Some("bytes=0-0")).await.status(), 206);
    let session = h.state().sessions.list()[0].clone();
    assert!(eventually(Duration::from_secs(20), || session.video_hash().is_some()).await);
    let data = pattern_bytes(1, 0, len as usize);
    let expected = localdebrid::sessions::oshash_from_parts(len, &data[..65536], &data[data.len() - 65536..]);
    assert_eq!(session.video_hash().unwrap(), expected);
    let (_, listing) = h.get_json(&format!("/stream/movie/{MOVIE_ID}.json")).await;
    assert_eq!(listing["streams"][0]["behaviorHints"]["videoHash"], expected);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sessions_api_lists_and_closes_and_deletes_data() {
    let (h, seeder, url) = movie_harness(2 * MIB).await;
    h.get_range(&url, None).await.bytes().await.unwrap();

    let (status, body) = h.get_json("/api/sessions").await;
    assert_eq!(status, 200);
    let sessions = body["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    let id = sessions[0]["id"].as_str().unwrap().to_string();
    assert_eq!(sessions[0]["infoHash"], seeder.info_hash);
    let session_url = sessions[0]["url"].as_str().unwrap().to_string();
    let body = h.get_range(&session_url, Some("bytes=10-19")).await.bytes().await.unwrap();
    assert!(body.as_ref() == pattern_bytes(1, 10, 10).as_slice());

    let torrent_dir = h.cache_dir().join(&seeder.info_hash);
    assert!(torrent_dir.exists());
    let response = h
        .client
        .delete(format!("{}/api/sessions/{id}?deleteData=true", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 204);
    assert!(h.state().sessions.list().is_empty());
    assert!(!torrent_dir.exists());
    assert!(h.state().engine.handle(&seeder.info_hash.parse().unwrap()).is_none());

    let response = h
        .client
        .delete(format!("{}/api/sessions/{id}", h.base))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 404);

    // Playing again after the session ended starts a new one.
    let body = h.get_range(&url, Some("bytes=0-99")).await.bytes().await.unwrap();
    assert!(body.as_ref() == pattern_bytes(1, 0, 100).as_slice());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn admin_can_create_a_session_from_a_magnet() {
    let h = Harness::new(|_| {}).await;
    let file = h.content_dir().join("Some.Video.mp4");
    write_pattern_file(&file, MIB, 9).unwrap();
    let seeder = h.seed(&file).await;
    let magnet = format!(
        "magnet:?xt=urn:btih:{}&tr={}",
        seeder.info_hash,
        percent_encoding::utf8_percent_encode(&h.tracker.announce_url, percent_encoding::NON_ALPHANUMERIC)
    );
    let response = h
        .client
        .post(format!("{}/api/sessions", h.base))
        .json(&json!({ "magnet": magnet }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    let created: serde_json::Value = response.json().await.unwrap();
    let url = created["url"].as_str().unwrap();
    assert!(url.ends_with("/Some.Video.mp4"), "{url}");
    let response = h.get_range(url, None).await;
    assert_eq!(response.headers()["content-type"], "video/mp4");
    let body = response.bytes().await.unwrap();
    assert!(body.as_ref() == pattern_bytes(9, 0, MIB as usize).as_slice());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn idle_sessions_are_reaped_and_cache_policy_applies() {
    let (h, seeder, url) = movie_harness(MIB).await;
    h.get_range(&url, None).await.bytes().await.unwrap();
    let state = h.state().clone();
    assert_eq!(state.sessions.close_idle(Duration::from_secs(3600), false).await.len(), 0);
    assert_eq!(state.sessions.close_idle(Duration::ZERO, false).await.len(), 1);
    let torrent_dir = h.cache_dir().join(&seeder.info_hash);
    assert!(torrent_dir.exists(), "kept for the TTL when deleteOnSessionEnd is off");

    let (status, cache) = h.get_json("/api/cache").await;
    assert_eq!(status, 200);
    assert_eq!(cache["entries"][0]["active"], false);
    assert!(cache["usedBytes"].as_u64().unwrap() >= MIB);

    let response = h.client.post(format!("{}/api/cache/clear", h.base)).send().await.unwrap();
    let report: serde_json::Value = response.json().await.unwrap();
    assert_eq!(report["deleted"], json!([seeder.info_hash]));
    assert!(!torrent_dir.exists());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn clear_cache_with_all_ends_active_sessions() {
    let (h, seeder, url) = movie_harness(MIB).await;
    h.get_range(&url, None).await.bytes().await.unwrap();
    let response = h.client.post(format!("{}/api/cache/clear?all=true", h.base)).send().await.unwrap();
    let report: serde_json::Value = response.json().await.unwrap();
    assert_eq!(report["closedSessions"], 1);
    assert!(!h.cache_dir().join(&seeder.info_hash).exists());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn invalid_requests_are_rejected_with_clear_statuses() {
    let (h, seeder, _url) = movie_harness(MIB).await;
    let get = |path: String| {
        let client = h.client.clone();
        let base = h.base.clone();
        async move { client.get(format!("{base}{path}")).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(get("/play/nothex/auto/x.mkv".into()).await, 400);
    assert_eq!(get(format!("/play/{}/bogus/x.mkv", seeder.info_hash)).await, 400);
    assert_eq!(get(format!("/play/{}/99/x.mkv", seeder.info_hash)).await, 404);
    assert_eq!(get(format!("/play/{}/s9e9/x.mkv", seeder.info_hash)).await, 404);
    assert_eq!(get("/video/doesnotexist/x.mkv".into()).await, 404);
    assert_eq!(get("/stream/movie/..%2F..%2Fetc.json".into()).await, 400);
    assert_eq!(get("/nothing/here".into()).await, 404);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torrent_without_peers_times_out_quickly() {
    let h = Harness::new(|c| c.engine.metadata_timeout_secs = 2).await;
    let started = std::time::Instant::now();
    let response = h
        .get_range(&format!("{}/play/{}/auto/x.mkv", h.base, "ab".repeat(20)), None)
        .await;
    assert_eq!(response.status(), 504);
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("metadata"));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(h.state().sessions.list().is_empty());
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn torrent_without_video_is_reported() {
    let h = Harness::new(|_| {}).await;
    let dir = h.content_dir().join("Album");
    std::fs::create_dir_all(&dir).unwrap();
    write_pattern_file(&dir.join("track01.flac"), MIB, 3).unwrap();
    std::fs::write(dir.join("cover.jpg"), b"jpg").unwrap();
    let seeder = h.seed(&dir).await;
    h.addon.set("movie", MOVIE_ID, json!([torrent_stream(&seeder.info_hash, None, "Album", &h.tracker.announce_url)]));
    let urls = h.stream_urls("movie", MOVIE_ID).await;
    let response = h.get_range(&urls[0], None).await;
    assert_eq!(response.status(), 422);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn peers_vanishing_mid_stream_aborts_the_response() {
    let h = Harness::new(|c| {
        c.engine.read_stall_timeout_secs = 2;
        c.engine.download_limit_kbps = 1024;
    })
    .await;
    let len = 24 * MIB;
    let file = h.content_dir().join("Slow.Movie.mkv");
    write_pattern_file(&file, len, 5).unwrap();
    let seeder = h.seed(&file).await;
    h.addon.set("movie", MOVIE_ID, json!([torrent_stream(&seeder.info_hash, None, "Slow.Movie", &h.tracker.announce_url)]));
    let url = &h.stream_urls("movie", MOVIE_ID).await[0];

    let mut response = h.get_range(url, None).await;
    assert_eq!(response.status(), 200);
    let mut received = response.chunk().await.unwrap().unwrap().len();
    seeder.stop().await;
    let outcome = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            match response.chunk().await {
                Ok(Some(c)) => received += c.len(),
                Ok(None) => return Ok(()),
                Err(e) => return Err(e),
            }
        }
    })
    .await
    .expect("response must not hang forever");
    assert!(outcome.is_err(), "body must be cut short, got {received} of {len} bytes");
    assert!((received as u64) < len);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failing_upstream_does_not_hide_the_others() {
    let h = Harness::new(|_| {}).await;
    let broken = localdebrid_testkit::FixtureAddon::start(std::net::Ipv4Addr::LOCALHOST).await.unwrap();
    let mut config = (*h.state().config()).clone();
    config.upstreams.push(localdebrid::config::UpstreamConfig {
        name: "down".into(),
        url: "http://127.0.0.1:1/manifest.json".into(),
        enabled: true,
    });
    config.upstreams.push(localdebrid::config::UpstreamConfig {
        name: "broken".into(),
        url: broken.manifest_url.clone(),
        enabled: true,
    });
    h.state().apply_config(config).unwrap();
    h.addon.set("movie", "tt9", json!([torrent_stream(&"cd".repeat(20), Some(0), "A.Movie.720p", "http://x/announce")]));
    broken.set("movie", "tt9", json!("not a list"));
    let urls = h.stream_urls("movie", "tt9").await;
    assert_eq!(urls.len(), 1);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn config_api_merges_validates_and_flags_restart() {
    let h = Harness::new(|_| {}).await;
    let response = h
        .client
        .put(format!("{}/api/config", h.base))
        .json(&json!({ "maxStreams": 5, "cache": { "ttlHours": 2 } }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["restartRequired"], false);
    assert_eq!(body["config"]["cache"]["ttlHours"], 2.0);
    assert_eq!(body["config"]["cache"]["maxSizeGb"], 10.0);
    assert_eq!(h.state().config().max_streams, 5);

    let response = h
        .client
        .put(format!("{}/api/config", h.base))
        .json(&json!({ "engine": { "peerLimit": 10 } }))
        .send()
        .await
        .unwrap();
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["restartRequired"], true);

    let response = h
        .client
        .put(format!("{}/api/config", h.base))
        .json(&json!({ "accessToken": "has/slash" }))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn status_reports_urls_and_engine_stats() {
    let (h, _seeder, url) = movie_harness(MIB).await;
    h.get_range(&url, None).await.bytes().await.unwrap();
    let (status, body) = h.get_json("/api/status").await;
    assert_eq!(status, 200);
    assert_eq!(body["sessions"], 1);
    assert!(body["port"].as_u64().unwrap() > 0);
    assert!(body["cacheUsedBytes"].as_u64().unwrap() >= MIB);
    assert!(body["engine"].get("downloadBps").is_some());
    let (status, logs) = h.get_json("/api/logs?lines=5").await;
    assert_eq!(status, 200);
    assert!(logs["lines"].is_array());
    let dashboard = h.client.get(format!("{}/", h.base)).send().await.unwrap();
    assert!(dashboard.headers()["content-type"].to_str().unwrap().starts_with("text/html"));
    h.shutdown().await;
}
