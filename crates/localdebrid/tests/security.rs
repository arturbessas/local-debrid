mod common;

use common::{Harness, MOVIE_ID};
use localdebrid::config::TlsMode;
use localdebrid::http::ConnInfo;
use localdebrid_testkit::{torrent_stream, write_pattern_file};
use serde_json::json;
use tower::{Layer, ServiceExt};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn access_token_protects_every_route_and_is_kept_in_urls() {
    let h = Harness::new(|c| c.access_token = "s3cret".into()).await;
    let file = h.content_dir().join("Token.Movie.mkv");
    write_pattern_file(&file, 512 * 1024, 2).unwrap();
    let seeder = h.seed(&file).await;
    h.addon.set("movie", MOVIE_ID, json!([torrent_stream(&seeder.info_hash, None, "Token.Movie", &h.tracker.announce_url)]));

    let status = |path: &str| {
        let client = h.client.clone();
        let url = format!("{}{path}", h.base);
        async move { client.get(url).send().await.unwrap().status().as_u16() }
    };
    assert_eq!(status("/manifest.json").await, 401);
    assert_eq!(status("/api/status").await, 401);
    assert_eq!(status("/wrong/manifest.json").await, 401);
    assert_eq!(status("/s3cret/manifest.json").await, 200);
    assert_eq!(status("/api/status?token=s3cret").await, 200);
    let bearer = h
        .client
        .get(format!("{}/api/status", h.base))
        .bearer_auth("s3cret")
        .send()
        .await
        .unwrap();
    assert_eq!(bearer.status(), 200);

    let (_, body) = h.get_json(&format!("/s3cret/stream/movie/{MOVIE_ID}.json")).await;
    let url = body["streams"][0]["url"].as_str().unwrap();
    assert!(url.contains("/s3cret/play/"), "{url}");
    let response = h.get_range(url, Some("bytes=0-9")).await;
    assert_eq!(response.status(), 206);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn requests_from_the_internet_are_refused() {
    let h = Harness::new(|_| {}).await;
    let state = h.state().clone();
    let service = axum::middleware::from_fn_with_state(state.clone(), localdebrid::http::gate)
        .layer(localdebrid::http::router(state));
    let request = |remote: &str| {
        let mut r = axum::http::Request::builder()
            .uri("/manifest.json")
            .body(axum::body::Body::empty())
            .unwrap();
        r.extensions_mut().insert(ConnInfo {
            remote: remote.parse().unwrap(),
            local: "192.168.1.5:8765".parse().unwrap(),
            tls: false,
        });
        r
    };
    let public = service.clone().oneshot(request("8.8.8.8:5000")).await.unwrap();
    assert_eq!(public.status(), 403);
    let lan = service.oneshot(request("192.168.1.40:5000")).await.unwrap();
    assert_eq!(lan.status(), 200);
    h.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn https_and_http_share_the_same_port() {
    let tmp = tempfile::tempdir().unwrap();
    let cert = rcgen::generate_simple_self_signed(vec!["localdebrid.test".to_string()]).unwrap();
    let cert_path = tmp.path().join("cert.pem");
    let key_path = tmp.path().join("key.pem");
    std::fs::write(&cert_path, cert.cert.pem()).unwrap();
    std::fs::write(&key_path, cert.signing_key.serialize_pem()).unwrap();

    let h = Harness::new(|c| {
        c.tls.mode = TlsMode::Custom;
        c.tls.custom_hostname = "localdebrid.test".into();
        c.tls.custom_cert_path = cert_path.display().to_string();
        c.tls.custom_key_path = key_path.display().to_string();
    })
    .await;
    assert!(h.state().tls.status().ready);
    let port = h.server.as_ref().unwrap().local_addr().port();

    let client = reqwest::Client::builder()
        .no_proxy()
        .tls_certs_only([reqwest::Certificate::from_pem(cert.cert.pem().as_bytes()).unwrap()])
        .resolve("localdebrid.test", ([127, 0, 0, 1], port).into())
        .build()
        .unwrap();
    let response = client
        .get(format!("https://localdebrid.test:{port}/manifest.json"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let manifest: serde_json::Value = response.json().await.unwrap();
    assert_eq!(manifest["id"], "com.localdebrid.addon");

    let plain = h.client.get(format!("{}/manifest.json", h.base)).send().await.unwrap();
    assert_eq!(plain.status(), 200);

    if let Some(urls) = h.state().status().urls.first() {
        assert_eq!(
            urls.https_manifest.as_deref(),
            Some(format!("https://localdebrid.test:{port}/manifest.json").as_str())
        );
    }
    h.shutdown().await;
}
