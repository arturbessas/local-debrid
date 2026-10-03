# HTTP API

Every route also works under `/<token>/…` when an access token is configured. The token
can instead be sent as `Authorization: Bearer <token>` or `?token=<token>`. Requests from
outside the local network get `403` while `lanOnly` is on.

## Stremio addon

| Method | Path | Notes |
|---|---|---|
| GET | `/manifest.json` | `resources: [stream]`, types `movie`, `series`, `anime`, id prefixes `tt`, `kitsu` |
| GET | `/stream/{type}/{id}.json` | `id` is `tt123`, `tt123:S:E` or `kitsu:N:E`. Answers `{"streams":[…]}` with `url` streams. |
| GET, HEAD | `/play/{infohash}/{selector}/{filename}` | Opens or reuses a session and streams the file. `selector`: file index, `s<S>e<E>` or `auto`. `filename` is cosmetic but its extension matters to players. |
| GET, HEAD | `/video/{sessionId}/{filename}` | Streams an existing session |

Stream object:

```json
{
  "name": "Local Debrid\n1080p",
  "title": "Movie.2020.1080p.WEB-DL.x264\n👤 154 💾 2.10 GB ⚙️ ThePirateBay\n🎞️ H.264 • 🔊 AAC • MP4",
  "description": "(same as title)",
  "url": "http://192.168.1.25:8765/play/<infohash>/1/Movie.2020.1080p.WEB-DL.x264.mp4",
  "behaviorHints": {
    "bingeGroup": "localdebrid|1080p|H.264",
    "filename": "Movie.2020.1080p.WEB-DL.x264.mp4",
    "videoSize": 2254857830,
    "videoHash": "8e245d9679d31e12",
    "notWebReady": false
  }
}
```

`videoSize` is exact once the torrent has been opened before, and approximate (from the
upstream title) otherwise. `videoHash` (OpenSubtitles hash) appears after the file has
been played once.

### Play responses

| Status | When |
|---|---|
| 200 / 206 | Full body / `Content-Range` partial body. `Accept-Ranges: bytes`, `Content-Type` from the extension. |
| 400 | Invalid info-hash or selector |
| 404 | File index or episode not in the torrent |
| 413 | File above `maxFileSizeGb` |
| 416 | Range starts beyond the end (`Content-Range: bytes */<len>`) |
| 422 | Torrent has no video file |
| 502 | Engine error |
| 503 | Too many concurrent streams |
| 504 | Metadata not obtained within `engine.metadataTimeoutSecs` (no reachable peers) |
| 507 | Not enough free storage for the file |

If peers vanish mid-stream, the body is aborted after `engine.readStallTimeoutSecs` with
no data, and the player retries.

## Admin

| Method | Path | Body / query | Result |
|---|---|---|---|
| GET | `/` | | Dashboard (HTML) |
| GET | `/api/status` | | Version, uptime, addon URLs per LAN IP, TLS state, engine totals (speed, peers), sessions, open streams, cache usage, free space, `restartPending` |
| GET | `/api/sessions` | | `{"sessions":[{id, infoHash, fileIdx, fileName, fileSize, title, progress, downloadedBytes, bytesServed, activeReaders, lastActivityMs, videoHash, torrent:{downloadBps, peersLive…}, url}]}` |
| POST | `/api/sessions` | `{"magnet": "…"}` or `{"infoHash": "…"}`, plus optional `fileIdx`, `season` + `episode`, `trackers` | `201` with the session and its `url` |
| GET | `/api/sessions/{id}` | | One session |
| DELETE | `/api/sessions/{id}` | `?deleteData=true\|false` (default: `cache.deleteOnSessionEnd`) | `204`; `404` if unknown |
| GET | `/api/cache` | | Directory, used bytes, limit, free space, entries (`infoHash`, `bytes`, `lastUsedMs`, `active`) |
| POST | `/api/cache/clear` | `?all=true` also ends every session first | `{closedSessions, deleted, freedBytes}` |
| GET | `/api/config` | | Full configuration |
| PUT | `/api/config` | Partial JSON, deep-merged | `{restartRequired, config}`; `400` on invalid values |
| POST | `/api/restart` | | `202`. Restarts the listener and engine with the saved configuration. |
| GET | `/api/logs` | `?lines=200` | Recent log lines |

## Configuration keys

```json
{
  "listenHost": "0.0.0.0", "port": 8765, "publicHost": "", "accessToken": "", "lanOnly": true,
  "addonName": "Local Debrid",
  "upstreams": [{"name": "Torrentio", "url": "https://torrentio.strem.fun/manifest.json", "enabled": true}],
  "maxStreams": 20, "resolutions": ["4k", "1440p", "1080p", "720p", "sd", "unknown"], "maxPerResolution": 0,
  "hideIncompatible": true, "hideLowQuality": true, "maxFileSizeGb": 0,
  "device": {"maxResolution": 2160, "hevc": true, "av1": false, "vp9": false, "hdr10": true,
             "dolbyVision": false, "dts": false, "truehd": false},
  "tls": {"mode": "localIp", "localIpDomain": "local-ip.sh",
          "localIpCertUrl": "https://local-ip.sh/server.pem", "localIpKeyUrl": "https://local-ip.sh/server.key",
          "customHostname": "", "customCertPath": "", "customKeyPath": ""},
  "streamUrlScheme": "http",
  "cache": {"idleSessionMinutes": 20, "deleteOnSessionEnd": false, "ttlHours": 12, "maxSizeGb": 10},
  "engine": {"btPort": 0, "enableDht": true, "enableUpload": true, "downloadLimitKbps": 0,
             "uploadLimitKbps": 0, "peerLimit": 100, "metadataTimeoutSecs": 60,
             "readStallTimeoutSecs": 120, "extraTrackers": ["udp://tracker.opentrackr.org:1337/announce", "…"]}
}
```

`streamUrlScheme: "sameAsRequest"` makes video URLs reuse the scheme and host of the
addon request (e.g. HTTPS through `local-ip.sh`) instead of `http://<lan-ip>`.
`publicHost` forces the host used in video URLs.
