#!/usr/bin/env python3
"""End-to-end test: torrent -> Local Debrid -> HTTP -> player.

Seeds a video from this machine (local tracker + fixture Stremio addon, see
crates/localdebrid-testkit), points a Local Debrid server at it and checks the whole
Stremio flow the way a player uses it: manifest, stream list, HEAD, Range/seek, full
download, decoding with ffmpeg (if available), pause, disconnect, cleanup.

Targets:
  --target local     builds and runs the desktop binary (default)
  --target android   installs the APK on the adb device/emulator and drives the app;
                     also locks the screen and forces Doze to check background playback

Only free content is used: a synthetic test pattern generated with ffmpeg, or a file
you pass with --video.
"""

import argparse
import hashlib
import json
import os
import shutil
import socket
import ssl
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
MOVIE_ID = "tt0000001"
RESULTS = []


def log(msg):
    print(f"[e2e] {msg}", flush=True)


def check(name, condition, detail=""):
    RESULTS.append((name, bool(condition), detail))
    log(("PASS " if condition else "FAIL ") + name + (f" — {detail}" if detail else ""))
    return condition


def http(method, url, headers=None, body=None, timeout=120, context=None):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method, headers=headers or {})
    if data is not None:
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, timeout=timeout, context=context) as r:
            return r.status, dict(r.headers), r.read()
    except urllib.error.HTTPError as e:
        return e.code, dict(e.headers), e.read()


def get_json(url, **kw):
    status, _, body = http("GET", url, **kw)
    return status, json.loads(body or b"null")


def find_ffmpeg():
    exe = shutil.which("ffmpeg")
    if exe:
        return exe
    try:
        import imageio_ffmpeg  # type: ignore

        return imageio_ffmpeg.get_ffmpeg_exe()
    except Exception:
        return None


def make_video(ffmpeg, out_dir, seconds):
    path = out_dir / "Test.Pattern.2026.720p.WEB.x264.AAC.mp4"
    log(f"generating {seconds}s synthetic H.264/AAC video")
    subprocess.run(
        [ffmpeg, "-hide_banner", "-loglevel", "error", "-y",
         "-f", "lavfi", "-i", "testsrc2=size=1280x720:rate=25",
         "-f", "lavfi", "-i", "sine=frequency=440:sample_rate=48000",
         "-t", str(seconds), "-c:v", "libx264", "-preset", "veryfast", "-pix_fmt", "yuv420p",
         "-profile:v", "high", "-level", "4.1", "-b:v", "3M", "-c:a", "aac", "-b:a", "128k",
         "-movflags", "+faststart", str(path)],
        check=True,
    )
    return path


def make_pattern_file(out_dir, size):
    path = out_dir / "Test.Pattern.2026.720p.WEB.x264.mkv"
    log("ffmpeg not found: using a non-video pattern file (byte checks only)")
    with open(path, "wb") as f:
        f.write(os.urandom(size))
    return path


def file_range(path, start, end):
    with open(path, "rb") as f:
        f.seek(start)
        return f.read(end - start + 1)


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def cargo_build():
    log("building release binaries")
    subprocess.run(["cargo", "build", "--release", "-p", "localdebrid", "-p", "localdebrid-testkit"],
                   cwd=ROOT, check=True)
    return ROOT / "target" / "release"


def start_swarm(bin_dir, content, advertise_host):
    proc = subprocess.Popen(
        [str(bin_dir / "testkit"), "--content", str(content), "--id", MOVIE_ID, "--advertise-host", advertise_host],
        stdout=subprocess.PIPE, text=True,
    )
    line = proc.stdout.readline()
    if not line:
        raise SystemExit("testkit did not start")
    info = json.loads(line)
    log(f"seeding {info['infoHash']} via {info['tracker']}")
    return proc, info


def wait_http(url, seconds):
    deadline = time.time() + seconds
    while time.time() < deadline:
        try:
            status, _, _ = http("GET", url, timeout=5)
            if status < 500:
                return True
        except Exception:
            pass
        time.sleep(1)
    return False


class Adb:
    def __init__(self, serial=None):
        self.base = ["adb"] + (["-s", serial] if serial else [])

    def __call__(self, *args, check=True, capture=True):
        r = subprocess.run(self.base + list(args), capture_output=capture, text=True)
        if check and r.returncode != 0:
            raise RuntimeError(f"adb {' '.join(args)} failed: {r.stderr}")
        return r.stdout

    def tap_text(self, text):
        import re
        import xml.etree.ElementTree as ET

        self("shell", "uiautomator", "dump", "/sdcard/ui.xml")
        xml = self("shell", "cat", "/sdcard/ui.xml")
        root = ET.fromstring(xml[xml.index("<"):])
        for node in root.iter("node"):
            if node.get("text", "").lower() == text.lower():
                x1, y1, x2, y2 = map(int, re.findall(r"\d+", node.get("bounds")))
                self("shell", "input", "tap", str((x1 + x2) // 2), str((y1 + y2) // 2))
                return True
        return False


def android_setup(args):
    adb = Adb(args.serial)
    apk = Path(args.apk) if args.apk else ROOT / "android/app/build/outputs/apk/release/app-release.apk"
    if not apk.exists():
        raise SystemExit(f"APK not found: {apk} (run scripts/build-android.sh)")
    log(f"installing {apk.name}")
    adb("install", "-r", "-g", str(apk))
    adb("shell", "dumpsys", "deviceidle", "whitelist", "+com.localdebrid.app", check=False)
    adb("shell", "input", "keyevent", "KEYCODE_WAKEUP")
    adb("shell", "wm", "dismiss-keyguard", check=False)
    adb("shell", "am", "start", "-n", "com.localdebrid.app/.MainActivity")
    time.sleep(3)
    if not adb.tap_text("Iniciar servidor"):
        log("start button not found (server may already be running)")
    adb("forward", f"tcp:{args.port}", "tcp:8765")
    return adb


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--target", choices=["local", "android"], default="local")
    parser.add_argument("--video", help="video file to seed instead of the generated test pattern")
    parser.add_argument("--seconds", type=int, default=150, help="length of the generated video")
    parser.add_argument("--port", type=int, default=8765)
    parser.add_argument("--apk")
    parser.add_argument("--serial", help="adb device serial")
    parser.add_argument("--advertise-host", help="address the server uses to reach this machine "
                        "(default: 127.0.0.1 local, 10.0.2.2 emulator)")
    args = parser.parse_args()

    work = Path(tempfile.mkdtemp(prefix="localdebrid-e2e-"))
    procs = []
    adb = None
    try:
        ffmpeg = find_ffmpeg()
        if args.video:
            content = Path(args.video).resolve()
        elif ffmpeg:
            content = make_video(ffmpeg, work, args.seconds)
        else:
            content = make_pattern_file(work, 48 << 20)
        size = content.stat().st_size

        bin_dir = cargo_build()
        if args.target == "android":
            advertise = args.advertise_host or "10.0.2.2"
        else:
            advertise = args.advertise_host or "127.0.0.1"
        swarm, info = start_swarm(bin_dir, content, advertise)
        procs.append(swarm)

        base = f"http://127.0.0.1:{args.port}"
        if args.target == "local":
            data = work / "server"
            server = subprocess.Popen([str(bin_dir / "localdebrid"), "--data-dir", str(data), "--port", str(args.port)])
            procs.append(server)
        else:
            adb = android_setup(args)
        if not check("server reachable", wait_http(f"{base}/manifest.json", 120)):
            return 1

        cfg = {"upstreams": [{"name": "e2e", "url": info["upstream"], "enabled": True}]}
        status, config = get_json(f"{base}/api/config")
        status, _, _ = http("PUT", f"{base}/api/config", body=cfg)
        check("configure upstream", status == 200)

        status, manifest = get_json(f"{base}/manifest.json")
        check("manifest.json", status == 200 and manifest["resources"][0]["name"] == "stream"
              and manifest["catalogs"] == [], json.dumps(manifest)[:120])

        status, st = get_json(f"{base}/api/status")
        if st.get("tls", {}).get("ready") and config["tls"]["mode"] == "localIp":
            # The wildcard name for 127.0.0.1 reaches the same port (directly or via adb forward).
            host = f"127-0-0-1.{config['tls']['localIpDomain']}"
            https_url = f"https://{host}:{args.port}/manifest.json"
            try:
                status, _, _ = http("GET", https_url, context=ssl.create_default_context(), timeout=30)
                check("HTTPS manifest with publicly trusted certificate", status == 200, https_url)
            except Exception as e:
                check("HTTPS manifest with publicly trusted certificate", False, f"{https_url}: {e}")
        else:
            log(f"HTTPS not ready ({st.get('tls')}); skipping HTTPS check")

        status, streams = get_json(f"{base}/stream/movie/{MOVIE_ID}.json")
        ok = status == 200 and len(streams.get("streams", [])) == 1
        check("stream list", ok, json.dumps(streams)[:200])
        if not ok:
            return 1
        stream = streams["streams"][0]
        url = stream["url"]
        check("stream has direct http url (no infoHash)", url.startswith("http") and "infoHash" not in stream, url)
        check("url keeps the container extension", url.lower().endswith(content.suffix.lower()), url)

        started = time.time()
        status, headers, _ = http("HEAD", url, timeout=180)
        headers = {k.lower(): v for k, v in headers.items()}
        check("HEAD", status == 200 and int(headers.get("content-length", -1)) == size
              and headers.get("accept-ranges") == "bytes", f"{status} {headers.get('content-length')} in {time.time() - started:.1f}s")

        for start, end in [(0, 1023), (size // 2, size // 2 + 262143), (size - 65536, size - 1)]:
            status, h, body = http("GET", url, headers={"Range": f"bytes={start}-{end}"})
            h = {k.lower(): v for k, v in h.items()}
            check(f"Range {start}-{end}", status == 206 and body == file_range(content, start, end)
                  and h.get("content-range") == f"bytes {start}-{end}/{size}")
        status, _, body = http("GET", url, headers={"Range": "bytes=-4096"})
        check("suffix range", status == 206 and body == file_range(content, size - 4096, size - 1))
        status, _, _ = http("GET", url, headers={"Range": f"bytes={size}-"})
        check("unsatisfiable range is 416", status == 416)

        started = time.time()
        status, _, body = http("GET", url, timeout=600)
        elapsed = time.time() - started
        check("full download matches the seeded file", status == 200 and hashlib.sha256(body).hexdigest() == sha256_file(content),
              f"{len(body) / elapsed / 1e6:.1f} MB/s")

        if ffmpeg and not args.video:
            for label, seek in [("decode from start", None), ("seek forward and decode", "100"), ("seek back and decode", "20")]:
                cmd = [ffmpeg, "-hide_banner", "-loglevel", "error"]
                if seek:
                    cmd += ["-ss", seek]
                cmd += ["-i", url, "-t", "3", "-f", "null", "-"]
                r = subprocess.run(cmd, capture_output=True, text=True, timeout=180)
                check(f"ffmpeg {label}", r.returncode == 0 and not r.stderr.strip(), r.stderr.strip()[:200])
        else:
            log("ffmpeg decode checks skipped")

        # Pause: a player stops reading but keeps the connection open, then resumes.
        try:
            req = urllib.request.Request(url, headers={"Range": "bytes=0-"})
            with urllib.request.urlopen(req, timeout=120) as r:
                first = r.read(1 << 20)
                time.sleep(20)
                second = r.read(1 << 20)
            check("pause and resume on the same connection", first + second == file_range(content, 0, (2 << 20) - 1))
        except Exception as e:
            check("pause and resume on the same connection", False, str(e))

        # Disconnect mid-stream.
        s = socket.create_connection(("127.0.0.1", args.port), timeout=30)
        path = url.split(f":{args.port}", 1)[1] if f":{args.port}" in url else url
        s.sendall(f"GET {path} HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n".encode())
        s.recv(65536)
        s.close()
        time.sleep(3)
        status, st = get_json(f"{base}/api/status")
        check("disconnect releases the stream", st.get("activeStreams") == 0, f"activeStreams={st.get('activeStreams')}")

        if adb:
            log("locking the screen and forcing Doze")
            adb("shell", "input", "keyevent", "KEYCODE_SLEEP")
            adb("shell", "dumpsys", "battery", "unplug")
            adb("shell", "dumpsys", "deviceidle", "force-idle", check=False)
            time.sleep(30)
            state = adb("shell", "dumpsys", "deviceidle", "get", "deep", check=False).strip()
            status, _, body = http("GET", url, headers={"Range": f"bytes={size // 3}-{size // 3 + 1048575}"}, timeout=120)
            check("serves with screen off and device idle", status == 206 and body == file_range(content, size // 3, size // 3 + 1048575),
                  f"deviceidle={state}")
            if ffmpeg and not args.video:
                r = subprocess.run([ffmpeg, "-hide_banner", "-loglevel", "error", "-ss", "60", "-i", url, "-t", "3", "-f", "null", "-"],
                                   capture_output=True, text=True, timeout=180)
                check("ffmpeg seek with screen off", r.returncode == 0, r.stderr.strip()[:200])
            adb("shell", "dumpsys", "deviceidle", "unforce", check=False)
            adb("shell", "dumpsys", "battery", "reset", check=False)
            adb("shell", "input", "keyevent", "KEYCODE_WAKEUP")

        status, sessions = get_json(f"{base}/api/sessions")
        ids = [x["id"] for x in sessions.get("sessions", [])]
        check("session listed", len(ids) == 1)
        for sid in ids:
            status, _, _ = http("DELETE", f"{base}/api/sessions/{sid}?deleteData=true")
            check("end session and delete data", status == 204)
        status, cache = get_json(f"{base}/api/cache")
        check("cache emptied", cache.get("entries") == [], json.dumps(cache.get("entries"))[:200])
        status, _, _ = http("POST", f"{base}/api/cache/clear")
        check("cache clear endpoint", status == 200)
    finally:
        if adb:
            adb("forward", "--remove", f"tcp:{args.port}", check=False)
        for p in procs:
            p.terminate()
            try:
                p.wait(timeout=20)
            except subprocess.TimeoutExpired:
                p.kill()
        shutil.rmtree(work, ignore_errors=True)

    failed = [r for r in RESULTS if not r[1]]
    print()
    print(f"{len(RESULTS) - len(failed)}/{len(RESULTS)} checks passed")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
