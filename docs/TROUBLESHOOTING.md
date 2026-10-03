# Troubleshooting

Logs: dashboard → **Logs**, `GET /api/logs`, or `scripts/dev.sh logcat` with the phone
connected over adb.

## Installing the addon

**Stroku says the URL is invalid.** Use the `https://…local-ip.sh:8765/manifest.json`
address. Stroku rejects `http://` URLs and URLs that don't end in `/manifest.json`.

**Stroku can't reach the addon / request timed out.**
- Phone and TV on the same Wi-Fi? Guest networks and "AP/client isolation" block
  device-to-device traffic. Disable isolation or use the main network.
- Open the `http://` address in the phone's browser and then on another device. If
  another device can't open it, the router is blocking LAN traffic.
- **DNS rebinding protection**: some routers, Pi-hole/AdGuard and some ISPs refuse DNS
  answers that point to private IPs, so `192-168-1-25.local-ip.sh` doesn't resolve.
  Check from a computer on the same network with `nslookup 192-168-1-25.local-ip.sh`.
  Fix: allow `local-ip.sh` in the router or DNS filter (dnsmasq:
  `rebind-domain-ok=/local-ip.sh/`), or set the Roku/router to a public DNS such as
  1.1.1.1.
- The app shows "HTTPS indisponível": the phone couldn't download the certificate (no
  internet when the server started). It retries every minute. Restart the server once
  the phone is online.

**The phone's IP changed.** The addon URL contains the IP. Reserve a fixed IP for the
phone in the router (DHCP reservation), or re-install the addon with the new address.
Using the same token keeps URLs short.

## Playback

**The stream list is empty.**
- Upstream addon down or rate-limited: check the logs for `upstream request failed`.
  Add a second source in **Fontes de torrents**.
- Filters too strict: check **Filtros de resolução** and the device profile, or turn
  off "Esconder releases incompatíveis" to see everything.

**Playback doesn't start / the TV shows an error after ~30 s.**
- Few or no seeders. Pick a release with more 👤 seeders.
- Logs say `timed out … waiting for torrent metadata`: no peer could be reached. Mobile
  hotspots and some ISPs block BitTorrent. Try another network.
- Logs say `not enough free space`: clear the cache or lower the cache size.

**Video plays, no audio.** The release has only DTS or TrueHD audio and the TV can't
decode or pass it through. Choose an AAC/AC3/EAC3 release. Releases like this show a ⚠️
warning.

**Wrong colours (purple/green).** Dolby Vision profile 5 on a TV without Dolby Vision.
These are hidden by default; if you enabled `device.dolbyVision`, disable it.

**Buffering.** The download speed (dashboard) must stay above the video bitrate:
roughly 3–8 Mbit/s for 1080p, 15–40 Mbit/s for 4K remuxes. Pick smaller releases or
ones with more seeders. Phone Wi-Fi on 5 GHz helps.

**Stops when the screen turns off.**
- Allow **Permitir rodar com a tela bloqueada** (battery optimisation exemption).
- Some vendors (Xiaomi/MIUI, Huawei, Samsung "sleeping apps", OnePlus) add their own
  killers. Set the app to "No restrictions" / "Autostart" and remove it from sleeping
  apps. See https://dontkillmyapp.com.
- The notification "Local Debrid · <ip>:8765" must be visible while the server runs.

**Subtitles (OpenSubtitles addon).** Stroku searches subtitles by IMDb id. Local
Debrid also provides `filename`, `videoSize` and, after the first play, `videoHash` for
clients that use them (Stremio).

## Development

**`cargo ndk` not found / missing target.** `scripts/build-android.sh` installs both.
Otherwise run `cargo install cargo-ndk` and
`rustup target add aarch64-linux-android x86_64-linux-android`.

**Gradle can't find the SDK.** Set `ANDROID_HOME` or create `android/local.properties`
with `sdk.dir=…`.

**Port already in use.** Change `port` in the dashboard or with `--port` on desktop.
