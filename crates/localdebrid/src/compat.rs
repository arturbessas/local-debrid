//! Decides whether a release is likely to direct-play on the configured device.
//! Nothing is transcoded, so a release the device cannot decode is not worth offering.

use serde::Serialize;

use crate::config::DeviceProfile;
use crate::release::{Audio, Hdr, ReleaseInfo, VideoCodec};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Ok,
    Warn,
    Incompatible,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Verdict {
    pub level: Level,
    pub reasons: Vec<String>,
}

const UNSUPPORTED_CONTAINERS: &[&str] = &["avi", "wmv", "flv", "rmvb", "divx", "vob", "3gp", "ogv"];

pub fn assess(info: &ReleaseInfo, device: &DeviceProfile) -> Verdict {
    let mut incompatible = Vec::new();
    let mut warnings = Vec::new();

    if let Some(container) = &info.container
        && UNSUPPORTED_CONTAINERS.contains(&container.as_str())
    {
        incompatible.push(format!("{} container", container.to_uppercase()));
    }

    match info.video_codec {
        Some(VideoCodec::H264) if info.ten_bit => incompatible.push("10-bit H.264".into()),
        Some(VideoCodec::Hevc) if !device.hevc => incompatible.push("HEVC".into()),
        Some(VideoCodec::Av1) if !device.av1 => incompatible.push("AV1".into()),
        Some(VideoCodec::Vp9) if !device.vp9 => incompatible.push("VP9".into()),
        Some(VideoCodec::Mpeg4Part2) => incompatible.push("XviD/DivX".into()),
        Some(VideoCodec::Mpeg2) => warnings.push("MPEG-2 video".into()),
        _ => {}
    }

    if let Some(resolution) = info.resolution
        && resolution > device.max_resolution
    {
        incompatible.push(format!("{resolution}p above device limit"));
    }

    if info.three_d {
        incompatible.push("3D".into());
    }

    let has_dv = info.hdr.contains(&Hdr::DolbyVision);
    let has_hdr_fallback = info.hdr.iter().any(|h| matches!(h, Hdr::Hdr10 | Hdr::Hdr10Plus));
    if has_dv && !device.dolby_vision {
        if has_hdr_fallback && device.hdr10 {
            warnings.push("Dolby Vision (HDR10 fallback)".into());
        } else {
            incompatible.push("Dolby Vision only".into());
        }
    } else if has_hdr_fallback && !device.hdr10 {
        warnings.push("HDR on SDR display".into());
    }

    // Releases often carry several tracks; only flag when every listed track is a problem.
    let audio_ok = |a: &Audio| match a {
        Audio::Dts | Audio::DtsHd => device.dts,
        Audio::TrueHd => device.truehd,
        Audio::Atmos => true,
        _ => true,
    };
    let listed: Vec<&Audio> = info.audio.iter().filter(|a| **a != Audio::Atmos).collect();
    if !listed.is_empty() && !listed.iter().any(|a| audio_ok(a)) {
        let names: Vec<_> = listed.iter().map(|a| a.label()).collect();
        warnings.push(format!("{} audio may be silent", names.join("/")));
    }

    if !incompatible.is_empty() {
        incompatible.extend(warnings);
        Verdict {
            level: Level::Incompatible,
            reasons: incompatible,
        }
    } else if !warnings.is_empty() {
        Verdict {
            level: Level::Warn,
            reasons: warnings,
        }
    } else {
        Verdict {
            level: Level::Ok,
            reasons: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::release::parse;

    fn verdict(name: &str) -> Verdict {
        assess(&parse(name), &DeviceProfile::default())
    }

    #[test]
    fn plain_h264_aac_is_ok() {
        assert_eq!(verdict("Movie.2020.1080p.WEB-DL.AAC.x264.mp4").level, Level::Ok);
    }

    #[test]
    fn hi10p_is_incompatible() {
        assert_eq!(verdict("[G] Show - 01 [1080p Hi10P x264]").level, Level::Incompatible);
    }

    #[test]
    fn hevc_depends_on_profile() {
        let info = parse("Movie.2020.1080p.x265");
        assert_eq!(assess(&info, &DeviceProfile::default()).level, Level::Ok);
        let no_hevc = DeviceProfile {
            hevc: false,
            ..Default::default()
        };
        assert_eq!(assess(&info, &no_hevc).level, Level::Incompatible);
    }

    #[test]
    fn dolby_vision_only_is_incompatible_but_with_fallback_is_warning() {
        assert_eq!(verdict("Movie.2160p.WEB.DV.H265").level, Level::Incompatible);
        assert_eq!(verdict("Movie.2160p.WEB.DV.HDR.H265").level, Level::Warn);
    }

    #[test]
    fn dts_only_audio_is_warning_but_dts_plus_ac3_is_ok() {
        assert_eq!(verdict("Movie.1080p.BluRay.DTS.x264").level, Level::Warn);
        assert_eq!(verdict("Movie.1080p.BluRay.DTS.AC3.x264").level, Level::Ok);
    }

    #[test]
    fn avi_and_xvid_are_incompatible() {
        assert_eq!(verdict("Movie.DVDRip.XviD.avi").level, Level::Incompatible);
    }

    #[test]
    fn resolution_above_device_limit_is_incompatible() {
        let info = parse("Movie.2160p.WEB.x265");
        let hd = DeviceProfile {
            max_resolution: 1080,
            ..Default::default()
        };
        assert_eq!(assess(&info, &hd).level, Level::Incompatible);
    }

    #[test]
    fn unknown_details_are_not_penalised() {
        assert_eq!(verdict("Some Movie 2020").level, Level::Ok);
    }
}
