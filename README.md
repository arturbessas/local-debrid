# Local Debrid

Turn your Android phone into a personal "debrid" server. Local Debrid is a Stremio addon
that runs **on the phone**: it finds torrents through the addons you already use
(Torrentio by default), downloads the one you pick **on demand**, and serves the video to
your TV as a plain **HTTP stream with Range/seek support**. No AllDebrid, Real-Debrid,
TorBox, VPS, home server, public IP or domain.

```
Stroku / Stremio ──(addon protocol)──▶ Local Debrid on the phone ──▶ torrent swarm
       ▲                                       │
       └──── http://<phone-ip>:8765/play/…mkv ◀┘   (Range requests, no transcoding)
```

It was built for **Stroku Native on Roku**, and works with any Stremio client.

- Movies, series episodes (season packs included), anime (`kitsu:` ids)
- Picks the right file in multi-file torrents (skips samples, subtitles, NFOs, images)
- HTTP Range: seek, pause/resume, several concurrent connections
- Progressive download with piece prioritisation around the playback position
- Hides releases the TV probably can't play (10-bit H.264, AVI/XviD, Dolby Vision
  without HDR fallback…), with a configurable device profile
- Resolution filters and a per-resolution cap
- Runs with the screen locked (foreground service, wake lock, Wi-Fi lock)
- Cache with idle-session close, TTL and size limit
- Local dashboard and admin API, optional access token, LAN-only by default

## Quick start

1. **Install the APK** on the phone (`dist/LocalDebrid-1.0.0.apk`, see
   [Building](#building)). Allow installing from unknown sources when Android asks.
2. Open **Local Debrid**, tap **Iniciar servidor**, and allow notifications.
3. Tap **Permitir rodar com a tela bloqueada** and allow it. This exempts the app from
   battery optimisation, so Android does not throttle it while the screen is off.
4. The app shows two addresses:
   - `https://192-168-1-25.local-ip.sh:8765/manifest.json` for **Stroku**
   - `http://192.168.1.25:8765/manifest.json` for Stremio desktop/mobile and other clients
5. In **Stroku Native**, add the addon (Settings → Addons → add by URL, or from the Roku's
   setup page at `http://<roku-ip>:8324`) using the **HTTPS** address.
6. Optional: in Stroku/Stremio, disable or remove the Torrentio+AllDebrid addon, so the
   list only shows Local Debrid streams.
7. Open a movie → **Local Debrid** stream → play. The first seconds take a little longer
   while the phone finds peers.

Phone and TV must be on the **same Wi-Fi**. The phone needs internet (to reach the
torrent swarm), and so does the Roku (to resolve the `local-ip.sh` name).

### Why HTTPS, and what is `local-ip.sh`?

Stroku only accepts addon URLs that start with `https://`, and it validates the
certificate against Roku's public CA list, so a self-signed certificate cannot work.
`local-ip.sh` is a free public DNS service: `192-168-1-25.local-ip.sh` resolves to
`192.168.1.25`, and the service publishes a valid wildcard certificate meant for exactly
this LAN use. The app downloads and renews that certificate automatically. Only DNS and
the certificate come from the service. Traffic never leaves your network: the Roku
connects straight to the phone. Video URLs are plain `http://<phone-ip>`.
If you prefer your own domain and certificate, see [Configuration](#configuration).

## How it works

1. Stroku asks `GET /stream/movie/tt1234567.json`.
2. Local Debrid asks its *upstream* addons (Torrentio by default) for torrents. These are
   ordinary Stremio addons that answer with `infoHash` streams.
3. It filters and ranks them (resolution, codec compatibility, size), then answers with
   `url` streams such as
   `http://192.168.1.25:8765/play/<infohash>/<file-index>/<file-name>.mkv`.
   It sends no `infoHash`, so the Roku never has to speak BitTorrent.
4. When the TV opens that URL, the phone fetches the torrent metadata, picks the file and
   starts downloading only that file. It then streams bytes as they arrive, prioritising
   the pieces around the requested position.
5. Seeking is just a new `Range` request. Downloading continues in the background while
   you watch, and the session closes when the TV has been idle for a while.

More detail: [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Codec compatibility (no transcoding)

The video is sent as-is. Roku plays H.264 (8-bit) and HEVC (4K models) in MKV/MP4, with
AAC/AC3/EAC3 audio. Local Debrid parses release names and, by default:

| Release | Behaviour |
|---|---|
| H.264 8-bit, HEVC, AAC/AC3/EAC3 | listed |
| 10-bit H.264 (Hi10P), XviD/DivX, AVI/WMV/FLV, 3D | **hidden** (incompatible) |
| Dolby Vision without HDR10 fallback | **hidden** (wrong colours on non-DV TVs) |
| Dolby Vision + HDR10, DTS-only or TrueHD-only audio | listed with a ⚠️ warning, ranked lower |
| AV1, VP9 | hidden unless enabled in the device profile |
| CAM / TS / screener | hidden |

Tune this in the dashboard (**Configurações** → device capabilities) or turn off
"Esconder releases incompatíveis". If a stream plays without audio, it is almost always
DTS/TrueHD on a TV without passthrough. Pick another release.

## Dashboard and configuration

Tap **Abrir painel** in the app, or browse to `http://<phone-ip>:8765/` from any device on
the LAN. The dashboard shows status, the addon URLs, sessions (progress, peers, speed),
cache, logs, a form to open a magnet by hand, and all settings:

- **Filtros de resolução**: which resolutions are listed (4K, 1440p, 1080p, 720p, SD,
  unknown) and the maximum number of streams per resolution.
- **Fontes de torrents**: upstream addons, one per line as `Name | manifest URL`. Any
  addon that returns `infoHash` streams works, for example Torrentio with its own options
  (`https://torrentio.strem.fun/providers=yts,eztv,1337x|qualityfilter=cam,scr/manifest.json`),
  MediaFusion or Comet in P2P mode, or a self-hosted scraper.
- **Token de acesso**: when set, every URL must include it
  (`https://…:8765/<token>/manifest.json`). Re-install the addon after changing it.
- **Cache**: idle minutes before a session closes, delete-on-close, TTL, maximum size.
- **HTTPS**: `local-ip.sh` (default), custom certificate (`customHostname`,
  `customCertPath`, `customKeyPath` via the API) or off.
- **Engine**: DHT, upload, rate limits, peer limit.

Settings marked *(reinício)* need a server restart, from the dashboard button or by
stopping and starting in the app. Configuration lives in the app's private storage, in
`config.json`.

The admin API is documented in [docs/API.md](docs/API.md).

## Building

Requirements: Rust (stable, 1.88+), Android SDK with platform 36 and NDK 27, JDK 17,
Python 3 (tests only). The scripts install `cargo-ndk` and the Rust Android targets if
missing.

```bash
scripts/build-android.sh      # release APK → dist/LocalDebrid-<version>.apk
scripts/release.sh            # tests + APK signed with a dedicated key + Termux package + SHA256SUMS
scripts/dev.sh apk            # debug build installed on the connected device
```

`build-android.sh` signs with the debug key unless `android/keystore.properties`
exists. `release.sh` creates a release key in `android/keystore/` on first run. Keep it:
Android only installs updates signed with the same key.

ABIs: `arm64-v8a` (phones) and `x86_64` (emulators). Override with
`ABIS=arm64-v8a scripts/build-android.sh`. 32-bit ARM is not shipped (see
[limitations](docs/LIMITATIONS.md)).

### Termux (alternative to the APK)

`scripts/release.sh` also produces `dist/localdebrid-termux-arm64-<version>.tar.gz`:

```bash
tar xzf localdebrid-termux-arm64-*.tar.gz && cd localdebrid-termux-arm64-* && ./install.sh
localdebrid-start
```

The APK is the recommended option. It keeps the server alive with the screen off, while
Termux depends on `termux-wake-lock` and Termux's own battery exemption.

### Desktop / Linux

The same server runs on any machine (useful for development):

```bash
cargo run --release -p localdebrid -- --data-dir ./localdebrid-data
```

## Testing

```bash
scripts/dev.sh test           # unit + integration tests (real BitTorrent swarm on localhost)
scripts/dev.sh e2e            # end-to-end against a desktop server
scripts/dev.sh e2e-android    # end-to-end against the app on an emulator/phone via adb
```

The integration tests start a real tracker, a real librqbit seeder and a fixture Stremio
addon on localhost. They then check manifest, stream responses, file selection (season
packs with samples/subtitles), sessions, HTTP Range and seek, concurrent readers,
client disconnects, peers disappearing mid-stream, missing peers, invalid torrents and
files, cache policy, access token, LAN-only access and HTTPS.

`scripts/e2e.py` generates a synthetic test-pattern video with ffmpeg (no copyrighted
content), seeds it, and drives the whole flow as a player would: manifest, HTTPS
certificate, stream list, HEAD, Range, full download with hash check, ffmpeg decode with
seeks, pause/resume, disconnect, session end and cache clean-up. With
`--target android` it also locks the screen and forces Doze while streaming.

## Documentation

- [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md): components, diagram, decisions
- [docs/API.md](docs/API.md): addon and admin endpoints
- [docs/TROUBLESHOOTING.md](docs/TROUBLESHOOTING.md)
- [docs/LIMITATIONS.md](docs/LIMITATIONS.md)

## Legal

Local Debrid is a personal tool for content you have the right to access. It implements
no DRM or paywall circumvention. BitTorrent shares data with other peers while
downloading (upload can be disabled in the settings).

Third-party code: [librqbit](https://github.com/ikatson/rqbit) (Apache-2.0) is the
BitTorrent engine, and the Android component of
[rustls-platform-verifier](https://github.com/rustls/rustls-platform-verifier) (MIT) is
vendored in `android/third_party/`.
