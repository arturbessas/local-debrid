//! Best-effort parsing of scene/P2P release names ("Movie.2020.1080p.WEB-DL.x265.10bit.DDP5.1").

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    H264,
    Hevc,
    Av1,
    Vp9,
    Mpeg4Part2,
    Mpeg2,
}

impl VideoCodec {
    pub fn label(self) -> &'static str {
        match self {
            VideoCodec::H264 => "H.264",
            VideoCodec::Hevc => "HEVC",
            VideoCodec::Av1 => "AV1",
            VideoCodec::Vp9 => "VP9",
            VideoCodec::Mpeg4Part2 => "XviD",
            VideoCodec::Mpeg2 => "MPEG-2",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Audio {
    Aac,
    Ac3,
    Eac3,
    Dts,
    DtsHd,
    TrueHd,
    Atmos,
    Flac,
    Opus,
    Mp3,
    Pcm,
}

impl Audio {
    pub fn label(self) -> &'static str {
        match self {
            Audio::Aac => "AAC",
            Audio::Ac3 => "AC3",
            Audio::Eac3 => "EAC3",
            Audio::Dts => "DTS",
            Audio::DtsHd => "DTS-HD",
            Audio::TrueHd => "TrueHD",
            Audio::Atmos => "Atmos",
            Audio::Flac => "FLAC",
            Audio::Opus => "Opus",
            Audio::Mp3 => "MP3",
            Audio::Pcm => "PCM",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Hdr {
    DolbyVision,
    Hdr10Plus,
    Hdr10,
    Hlg,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Cam,
    Telesync,
    Screener,
    Hdtv,
    WebRip,
    WebDl,
    BluRay,
    Remux,
    Dvd,
}

impl Source {
    pub fn is_low_quality(self) -> bool {
        matches!(self, Source::Cam | Source::Telesync | Source::Screener)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseInfo {
    pub resolution: Option<u32>,
    pub video_codec: Option<VideoCodec>,
    pub ten_bit: bool,
    pub hdr: Vec<Hdr>,
    pub audio: Vec<Audio>,
    pub source: Option<Source>,
    pub container: Option<String>,
    pub three_d: bool,
}

macro_rules! re {
    ($name:ident, $pattern:expr) => {
        static $name: LazyLock<Regex> = LazyLock::new(|| Regex::new($pattern).unwrap());
    };
}

// Tokens are delimited by anything that is not a letter or digit, so "x265" matches
// in "Movie.x265-GRP" but not inside "abx2650".
const L: &str = r"(?:^|[^a-z0-9])";
const R: &str = r"(?:$|[^a-z0-9])";

re!(RES_2160, &format!(r"{L}(?:2160p|4k|uhd){R}"));
re!(RES_1440, &format!(r"{L}1440p{R}"));
re!(RES_1080, &format!(r"{L}1080[pi]{R}"));
re!(RES_720, &format!(r"{L}720p{R}"));
re!(RES_576, &format!(r"{L}576p{R}"));
re!(RES_480, &format!(r"{L}(?:480p|sd){R}"));
re!(RES_360, &format!(r"{L}360p{R}"));

re!(CODEC_HEVC, &format!(r"{L}(?:x\.?265|h\.?265|hevc){R}"));
re!(CODEC_H264, &format!(r"{L}(?:x\.?264|h\.?264|avc){R}"));
re!(CODEC_AV1, &format!(r"{L}av1{R}"));
re!(CODEC_VP9, &format!(r"{L}vp9{R}"));
re!(CODEC_XVID, &format!(r"{L}(?:xvid|divx){R}"));
re!(CODEC_MPEG2, &format!(r"{L}mpeg-?2{R}"));
re!(TEN_BIT, &format!(r"{L}(?:10-?bits?|hi10p?|10\s?bit){R}"));

re!(HDR_DV, &format!(r"{L}(?:dv|dovi|dolby[ .]?vision){R}"));
re!(HDR_10P, &format!(r"{L}hdr10(?:\+|plus)"));
re!(HDR_10, &format!(r"{L}hdr(?:10)?{R}"));
re!(HDR_HLG, &format!(r"{L}hlg{R}"));

re!(AUDIO_TRUEHD, &format!(r"{L}true-?hd"));
re!(AUDIO_ATMOS, &format!(r"{L}atmos{R}"));
re!(AUDIO_DTSHD, &format!(r"{L}dts-?(?:hd|x|ma){R}|{L}dts-?hd[ .-]?ma"));
re!(AUDIO_DTS, &format!(r"{L}dts{R}|{L}dts[0-9]"));
re!(AUDIO_EAC3, &format!(r"{L}(?:e-?ac-?3|ddp|dd\+|dolby digital plus){R}|{L}ddp?[0-9]"));
re!(AUDIO_AC3, &format!(r"{L}(?:ac-?3|dd|dolby digital){R}|{L}dd[0-9]"));
re!(AUDIO_AAC, &format!(r"{L}aac"));
re!(AUDIO_FLAC, &format!(r"{L}flac"));
re!(AUDIO_OPUS, &format!(r"{L}opus{R}"));
re!(AUDIO_MP3, &format!(r"{L}mp3{R}"));
re!(AUDIO_PCM, &format!(r"{L}l?pcm{R}"));

re!(SRC_CAM, &format!(r"{L}(?:cam|camrip|hdcam|cam-?rip){R}"));
re!(SRC_TS, &format!(r"{L}(?:ts|telesync|hdts|hd-?ts|tc|telecine|pdvd){R}"));
re!(SRC_SCR, &format!(r"{L}(?:scr|screener|dvdscr|bdscr){R}"));
re!(SRC_REMUX, &format!(r"{L}(?:remux|bdremux){R}"));
re!(SRC_BLURAY, &format!(r"{L}(?:blu-?ray|bdrip|brrip|bd|bd25|bd50){R}"));
re!(SRC_WEBRIP, &format!(r"{L}web-?rip{R}"));
re!(SRC_WEBDL, &format!(r"{L}(?:web-?dl|web){R}"));
re!(SRC_HDTV, &format!(r"{L}(?:hdtv|pdtv|dsr){R}"));
re!(SRC_DVD, &format!(r"{L}(?:dvd-?rip|dvd){R}"));
re!(THREE_D, &format!(r"{L}(?:3d|h-?sbs|h-?ou|half-?sbs|half-?ou){R}"));

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "mov", "webm", "ts", "m2ts", "mts", "avi", "wmv", "mpg", "mpeg",
    "flv", "ogv", "3gp", "vob", "divx", "rmvb",
];

pub fn extension(name: &str) -> Option<String> {
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let (_, ext) = base.rsplit_once('.')?;
    let ext = ext.to_ascii_lowercase();
    (!ext.is_empty() && ext.len() <= 5 && ext.chars().all(|c| c.is_ascii_alphanumeric()))
        .then_some(ext)
}

pub fn is_video_file(name: &str) -> bool {
    extension(name).is_some_and(|ext| VIDEO_EXTENSIONS.contains(&ext.as_str()))
}

pub fn parse(text: &str) -> ReleaseInfo {
    let t = text.to_lowercase();
    let mut info = ReleaseInfo {
        resolution: [
            (&*RES_2160, 2160),
            (&*RES_1440, 1440),
            (&*RES_1080, 1080),
            (&*RES_720, 720),
            (&*RES_576, 576),
            (&*RES_480, 480),
            (&*RES_360, 360),
        ]
        .into_iter()
        .find(|(re, _)| re.is_match(&t))
        .map(|(_, r)| r),
        ten_bit: TEN_BIT.is_match(&t),
        three_d: THREE_D.is_match(&t),
        ..Default::default()
    };

    info.video_codec = [
        (&*CODEC_HEVC, VideoCodec::Hevc),
        (&*CODEC_AV1, VideoCodec::Av1),
        (&*CODEC_VP9, VideoCodec::Vp9),
        (&*CODEC_H264, VideoCodec::H264),
        (&*CODEC_XVID, VideoCodec::Mpeg4Part2),
        (&*CODEC_MPEG2, VideoCodec::Mpeg2),
    ]
    .into_iter()
    .find(|(re, _)| re.is_match(&t))
    .map(|(_, c)| c);

    if HDR_DV.is_match(&t) {
        info.hdr.push(Hdr::DolbyVision);
    }
    if HDR_10P.is_match(&t) {
        info.hdr.push(Hdr::Hdr10Plus);
    } else if HDR_10.is_match(&t) {
        info.hdr.push(Hdr::Hdr10);
    }
    if HDR_HLG.is_match(&t) {
        info.hdr.push(Hdr::Hlg);
    }

    let mut push_audio = |re: &Regex, audio: Audio| {
        if re.is_match(&t) && !info.audio.contains(&audio) {
            info.audio.push(audio);
        }
    };
    push_audio(&AUDIO_TRUEHD, Audio::TrueHd);
    push_audio(&AUDIO_ATMOS, Audio::Atmos);
    if AUDIO_DTSHD.is_match(&t) {
        push_audio(&AUDIO_DTSHD, Audio::DtsHd);
    } else {
        push_audio(&AUDIO_DTS, Audio::Dts);
    }
    if AUDIO_EAC3.is_match(&t) {
        push_audio(&AUDIO_EAC3, Audio::Eac3);
    } else {
        push_audio(&AUDIO_AC3, Audio::Ac3);
    }
    push_audio(&AUDIO_AAC, Audio::Aac);
    push_audio(&AUDIO_FLAC, Audio::Flac);
    push_audio(&AUDIO_OPUS, Audio::Opus);
    push_audio(&AUDIO_MP3, Audio::Mp3);
    push_audio(&AUDIO_PCM, Audio::Pcm);

    info.source = [
        (&*SRC_CAM, Source::Cam),
        (&*SRC_SCR, Source::Screener),
        (&*SRC_REMUX, Source::Remux),
        (&*SRC_BLURAY, Source::BluRay),
        (&*SRC_WEBRIP, Source::WebRip),
        (&*SRC_WEBDL, Source::WebDl),
        (&*SRC_HDTV, Source::Hdtv),
        (&*SRC_DVD, Source::Dvd),
        (&*SRC_TS, Source::Telesync),
    ]
    .into_iter()
    .find(|(re, _)| re.is_match(&t))
    .map(|(_, s)| s);

    info.container = text
        .lines()
        .filter_map(extension)
        .find(|ext| VIDEO_EXTENSIONS.contains(&ext.as_str()));
    info
}

impl ReleaseInfo {
    /// Fills fields this one is missing from `other` (e.g. torrent name + file name).
    pub fn merge(mut self, other: ReleaseInfo) -> ReleaseInfo {
        self.resolution = self.resolution.or(other.resolution);
        self.video_codec = self.video_codec.or(other.video_codec);
        self.ten_bit |= other.ten_bit;
        self.three_d |= other.three_d;
        for hdr in other.hdr {
            if !self.hdr.contains(&hdr) {
                self.hdr.push(hdr);
            }
        }
        for audio in other.audio {
            if !self.audio.contains(&audio) {
                self.audio.push(audio);
            }
        }
        self.source = self.source.or(other.source);
        self.container = other.container.or(self.container);
        self
    }

    /// Group used by the resolution filter (see `config::RESOLUTION_GROUPS`).
    pub fn resolution_group(&self) -> &'static str {
        match self.resolution {
            Some(r) if r >= 2160 => "4k",
            Some(r) if r >= 1440 => "1440p",
            Some(r) if r >= 1080 => "1080p",
            Some(r) if r >= 720 => "720p",
            Some(_) => "sd",
            None => "unknown",
        }
    }

    pub fn resolution_label(&self) -> String {
        match self.resolution {
            Some(2160) => "4K".into(),
            Some(r) => format!("{r}p"),
            None => "SD?".into(),
        }
    }

    pub fn video_label(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let Some(codec) = self.video_codec {
            parts.push(codec.label().to_string());
        }
        if self.ten_bit {
            parts.push("10bit".into());
        }
        for hdr in &self.hdr {
            parts.push(
                match hdr {
                    Hdr::DolbyVision => "DV",
                    Hdr::Hdr10Plus => "HDR10+",
                    Hdr::Hdr10 => "HDR",
                    Hdr::Hlg => "HLG",
                }
                .into(),
            );
        }
        (!parts.is_empty()).then(|| parts.join(" "))
    }

    pub fn audio_label(&self) -> Option<String> {
        (!self.audio.is_empty()).then(|| {
            self.audio
                .iter()
                .map(|a| a.label())
                .collect::<Vec<_>>()
                .join("/")
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_typical_web_release() {
        let info = parse("Movie.Name.2021.1080p.WEB-DL.DDP5.1.H.264-GRP");
        assert_eq!(info.resolution, Some(1080));
        assert_eq!(info.video_codec, Some(VideoCodec::H264));
        assert_eq!(info.audio, vec![Audio::Eac3]);
        assert_eq!(info.source, Some(Source::WebDl));
        assert!(!info.ten_bit);
    }

    #[test]
    fn parses_uhd_remux() {
        let info = parse("Movie 2019 2160p UHD BluRay REMUX HDR10 HEVC TrueHD 7.1 Atmos-FGT");
        assert_eq!(info.resolution, Some(2160));
        assert_eq!(info.video_codec, Some(VideoCodec::Hevc));
        assert_eq!(info.hdr, vec![Hdr::Hdr10]);
        assert!(info.audio.contains(&Audio::TrueHd));
        assert!(info.audio.contains(&Audio::Atmos));
        assert_eq!(info.source, Some(Source::Remux));
    }

    #[test]
    fn parses_dolby_vision_without_hdr_fallback() {
        let info = parse("Show.S01E01.2160p.WEB.H265.DV.DDP5.1");
        assert_eq!(info.hdr, vec![Hdr::DolbyVision]);
        let info = parse("Show.S01E01.2160p.WEB.H265.DV.HDR.DDP5.1");
        assert_eq!(info.hdr, vec![Hdr::DolbyVision, Hdr::Hdr10]);
    }

    #[test]
    fn detects_hi10p_anime() {
        let info = parse("[Group] Anime - 01 [1080p Hi10P x264 FLAC].mkv");
        assert!(info.ten_bit);
        assert_eq!(info.video_codec, Some(VideoCodec::H264));
        assert_eq!(info.container.as_deref(), Some("mkv"));
        assert_eq!(info.audio, vec![Audio::Flac]);
    }

    #[test]
    fn detects_low_quality_sources() {
        assert_eq!(parse("Movie.2024.HDCAM.x264").source, Some(Source::Cam));
        assert_eq!(parse("Movie 2024 HD-TS x264").source, Some(Source::Telesync));
        assert_eq!(parse("Movie.2024.DVDScr.XviD").source, Some(Source::Screener));
    }

    #[test]
    fn does_not_match_tokens_inside_words() {
        let info = parse("Hdtvland.Tsunamis.Camera.Ordered");
        assert_eq!(info.source, None);
        assert!(parse("Movie.Avcxyz").video_codec.is_none());
    }

    #[test]
    fn container_comes_from_any_line() {
        let info = parse("Movie 2020 720p\nMovie.2020.720p.mp4\n👤 10 💾 1.2 GB");
        assert_eq!(info.container.as_deref(), Some("mp4"));
        assert_eq!(info.resolution, Some(720));
    }

    #[test]
    fn video_file_detection() {
        assert!(is_video_file("dir/Movie.MKV"));
        assert!(is_video_file("a.mp4"));
        assert!(!is_video_file("Movie.srt"));
        assert!(!is_video_file("Movie.nfo"));
        assert!(!is_video_file("noextension"));
    }

    #[test]
    fn merge_prefers_file_container() {
        let a = parse("Movie.2020.1080p.BluRay");
        let b = parse("movie.mp4");
        let merged = a.merge(b);
        assert_eq!(merged.resolution, Some(1080));
        assert_eq!(merged.container.as_deref(), Some("mp4"));
    }
}
