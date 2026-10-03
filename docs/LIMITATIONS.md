# Known limitations

- **No transcoding.** Files are served exactly as they are in the torrent. Releases the
  TV can't decode won't play; the compatibility filter hides or flags the likely
  offenders based on the release name, which can be wrong or incomplete.
- **HTTPS depends on `local-ip.sh`** (DNS and certificate) because Stroku only accepts
  HTTPS addons. Its certificate is a shared wildcard whose private key is public by
  design. That is fine for a LAN addon, but it gives no confidentiality against someone
  already on your Wi-Fi. If the service disappears, use a custom certificate
  (`tls.mode: custom`) for a domain you control. Routers with DNS-rebinding protection
  must allow `local-ip.sh`.
- **The addon URL contains the phone's IP.** Use a DHCP reservation so it doesn't change.
- **Torrent discovery comes from upstream addons.** Torrentio is the default; if it rate
  limits or changes, configure other sources. Upstream availability and accuracy are
  outside this project's control.
- **First play needs metadata from the swarm.** Expect a few seconds before playback
  starts (more for torrents with few peers). Metadata is cached, so replays start
  faster.
- **Whole-file download.** The selected file is downloaded completely in the
  background, and it needs that much free storage. The cache limit only evicts torrents
  that are not playing.
- **32-bit ARM devices are not supported.** librqbit 9.0.1 fails to write beyond 2 GiB on
  32-bit Android (fixed upstream after that release). The APK ships `arm64-v8a` and
  `x86_64`.
- **Wi-Fi power saving.** Android 14+ ignores the high-performance Wi-Fi lock. The
  server keeps working with the screen off (verified with forced Doze), but some vendor
  ROMs throttle background Wi-Fi more aggressively (see Troubleshooting).
- **Stroku subtitle matching** uses the IMDb id only: Stroku doesn't forward
  `videoHash` or `filename` to subtitle addons.
- **Single user.** No accounts. Anyone on the LAN can use the addon unless an access
  token is set.
- **Seeding while watching.** Upload is on by default (`engine.enableUpload`), as
  BitTorrent expects. Disable it if your network or jurisdiction requires.
