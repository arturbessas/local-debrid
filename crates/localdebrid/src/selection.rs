//! Picks the video file to play from a (possibly multi-file) torrent.

use std::sync::LazyLock;

use regex::Regex;

use crate::release::is_video_file;

#[derive(Clone, Debug, PartialEq)]
pub struct TorrentFile {
    pub index: usize,
    /// Path inside the torrent, `/`-separated.
    pub path: String,
    pub len: u64,
    pub padding: bool,
}

impl TorrentFile {
    pub fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selector {
    pub file_idx: Option<usize>,
    pub episode: Option<Episode>,
    pub filename_hint: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Episode {
    pub season: u32,
    pub episode: u32,
}

#[derive(Debug, thiserror::Error, PartialEq)]
pub enum SelectionError {
    #[error("the torrent has no video file")]
    NoVideo,
    #[error("file index {0} does not exist in the torrent")]
    NoSuchFile(usize),
    #[error("file index {0} is not a video file")]
    NotVideo(usize),
    #[error("no file for season {season} episode {episode}")]
    EpisodeNotFound { season: u32, episode: u32 },
}

static SAMPLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:sample|trailer|featurettes?|extras?|bonus|behind[ ._-]the[ ._-]scenes|deleted[ ._-]scenes|rarbg[ ._-]?com)(?:$|[^a-z0-9])").unwrap()
});
static SXXEYY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)s(\d{1,2})[ ._-]?e(\d{1,3})(?:[ ._-]?e?(\d{1,3}))?").unwrap());
static NXNN: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:^|[^a-z0-9])(\d{1,2})x(\d{2,3})(?:$|[^a-z0-9])").unwrap());
static SEASON_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)(?:season|temporada|s)[ ._-]?(\d{1,2})(?:$|[^a-z0-9])").unwrap());
static EPISODE_WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)(?:^|[^a-z0-9])(?:e|ep|episode|episodio|epis[oó]dio)[ ._-]?(\d{1,3})(?:$|[^a-z0-9])").unwrap()
});
static BARE_NUMBER: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?:^|[ ._\-\[])(\d{1,3})(?:v\d)?(?:$|[ ._\-\]])").unwrap());

/// Whether a file looks like a sample/extra rather than the feature.
pub fn is_extra(file: &TorrentFile) -> bool {
    SAMPLE.is_match(&file.path)
}

/// Season/episode numbers a file name refers to. The season can come from a parent folder.
pub fn parse_episode(path: &str) -> Option<(Option<u32>, Vec<u32>)> {
    let name = path.rsplit('/').next().unwrap_or(path);
    if let Some(c) = SXXEYY.captures(name) {
        let season = c[1].parse().ok();
        let first: u32 = c[2].parse().ok()?;
        let mut episodes = vec![first];
        if let Some(last) = c.get(3).and_then(|m| m.as_str().parse::<u32>().ok())
            && last > first
            && last - first < 5
        {
            episodes.extend(first + 1..=last);
        }
        return Some((season, episodes));
    }
    if let Some(c) = NXNN.captures(name) {
        return Some((c[1].parse().ok(), vec![c[2].parse().ok()?]));
    }
    let dir_season = path
        .rsplit('/')
        .skip(1)
        .find_map(|dir| SEASON_DIR.captures(dir).and_then(|c| c[1].parse().ok()));
    if let Some(c) = EPISODE_WORD.captures(name) {
        return Some((dir_season, vec![c[1].parse().ok()?]));
    }
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
    let numbers: Vec<u32> = BARE_NUMBER
        .captures_iter(stem)
        .filter_map(|c| c[1].parse().ok())
        .filter(|n| !(480..=2160).contains(n))
        .collect();
    if numbers.len() == 1 {
        return Some((dir_season, numbers));
    }
    None
}

pub fn select(files: &[TorrentFile], selector: &Selector) -> Result<usize, SelectionError> {
    let videos: Vec<&TorrentFile> = files
        .iter()
        .filter(|f| !f.padding && is_video_file(&f.path))
        .collect();
    if videos.is_empty() {
        return Err(SelectionError::NoVideo);
    }
    let largest = videos.iter().map(|f| f.len).max().unwrap_or(0);
    // A small file whose name says "sample" is never the feature; a huge one might be
    // (e.g. a movie literally called "Sample"), hence the size guard.
    let candidates: Vec<&TorrentFile> = videos
        .iter()
        .copied()
        .filter(|f| !(is_extra(f) && f.len < largest / 3))
        .collect();

    if let Some(idx) = selector.file_idx {
        let file = files
            .iter()
            .find(|f| f.index == idx)
            .ok_or(SelectionError::NoSuchFile(idx))?;
        if !is_video_file(&file.path) {
            return Err(SelectionError::NotVideo(idx));
        }
        return Ok(idx);
    }

    if let Some(hint) = selector.filename_hint.as_deref().filter(|h| !h.is_empty()) {
        let hint = hint.rsplit('/').next().unwrap_or(hint);
        if let Some(file) = candidates
            .iter()
            .find(|f| f.file_name().eq_ignore_ascii_case(hint))
        {
            return Ok(file.index);
        }
    }

    if let Some(ep) = selector.episode {
        let parsed: Vec<(&TorrentFile, Option<u32>, Vec<u32>)> = candidates
            .iter()
            .filter_map(|f| parse_episode(&f.path).map(|(s, e)| (*f, s, e)))
            .collect();
        let any_season_known = parsed.iter().any(|(_, s, _)| s.is_some());
        let mut matches: Vec<&TorrentFile> = parsed
            .iter()
            .filter(|(_, season, episodes)| {
                episodes.contains(&ep.episode)
                    && match season {
                        Some(s) => *s == ep.season,
                        None => !any_season_known,
                    }
            })
            .map(|(f, _, _)| *f)
            .collect();
        matches.sort_by_key(|f| std::cmp::Reverse(f.len));
        return matches
            .first()
            .map(|f| f.index)
            .ok_or(SelectionError::EpisodeNotFound {
                season: ep.season,
                episode: ep.episode,
            });
    }

    candidates
        .iter()
        .max_by_key(|f| f.len)
        .map(|f| f.index)
        .ok_or(SelectionError::NoVideo)
}

#[cfg(test)]
mod tests {
    use super::*;

    const MB: u64 = 1024 * 1024;

    fn files(list: &[(&str, u64)]) -> Vec<TorrentFile> {
        list.iter()
            .enumerate()
            .map(|(index, (path, len))| TorrentFile {
                index,
                path: path.to_string(),
                len: *len,
                padding: false,
            })
            .collect()
    }

    fn episode(season: u32, episode: u32) -> Selector {
        Selector {
            episode: Some(Episode { season, episode }),
            ..Default::default()
        }
    }

    #[test]
    fn movie_picks_main_file_and_skips_sample_subs_and_metadata() {
        let f = files(&[
            ("Movie/movie.srt", MB / 10),
            ("Movie/Sample/movie-sample.mkv", 40 * MB),
            ("Movie/movie.nfo", 1024),
            ("Movie/movie.mkv", 1800 * MB),
            ("Movie/cover.jpg", MB),
        ]);
        assert_eq!(select(&f, &Selector::default()), Ok(3));
    }

    #[test]
    fn single_file_torrent() {
        let f = files(&[("Movie.2020.1080p.mp4", 900 * MB)]);
        assert_eq!(select(&f, &Selector::default()), Ok(0));
    }

    #[test]
    fn torrent_without_video_is_an_error() {
        let f = files(&[("album/track.flac", 30 * MB), ("album/cover.jpg", MB)]);
        assert_eq!(select(&f, &Selector::default()), Err(SelectionError::NoVideo));
    }

    #[test]
    fn explicit_index_is_trusted_but_validated() {
        let f = files(&[("a.mkv", 100 * MB), ("b.mkv", 900 * MB), ("c.srt", MB)]);
        let sel = |i| Selector {
            file_idx: Some(i),
            ..Default::default()
        };
        assert_eq!(select(&f, &sel(0)), Ok(0));
        assert_eq!(select(&f, &sel(7)), Err(SelectionError::NoSuchFile(7)));
        assert_eq!(select(&f, &sel(2)), Err(SelectionError::NotVideo(2)));
    }

    #[test]
    fn season_pack_picks_requested_episode() {
        let f = files(&[
            ("Show.S01.1080p/Show.S01E01.1080p.mkv", 700 * MB),
            ("Show.S01.1080p/Show.S01E02.1080p.mkv", 650 * MB),
            ("Show.S01.1080p/Show.S01E03.1080p.mkv", 720 * MB),
            ("Show.S01.1080p/Show.S01E02.1080p.srt", MB),
        ]);
        assert_eq!(select(&f, &episode(1, 2)), Ok(1));
        assert_eq!(
            select(&f, &episode(1, 9)),
            Err(SelectionError::EpisodeNotFound { season: 1, episode: 9 })
        );
        assert_eq!(
            select(&f, &episode(2, 1)),
            Err(SelectionError::EpisodeNotFound { season: 2, episode: 1 })
        );
    }

    #[test]
    fn complete_series_with_season_folders() {
        let f = files(&[
            ("Show/Season 1/Episode 1.mkv", 500 * MB),
            ("Show/Season 1/Episode 2.mkv", 500 * MB),
            ("Show/Season 2/Episode 1.mkv", 500 * MB),
            ("Show/Season 2/Episode 2.mkv", 500 * MB),
        ]);
        assert_eq!(select(&f, &episode(2, 1)), Ok(2));
        assert_eq!(select(&f, &episode(1, 2)), Ok(1));
    }

    #[test]
    fn alternative_episode_notations() {
        let f = files(&[("show.1x05.hdtv.avi", 300 * MB), ("show.1x06.hdtv.avi", 300 * MB)]);
        assert_eq!(select(&f, &episode(1, 6)), Ok(1));
        let f = files(&[
            ("[Group] Anime - 01 [1080p].mkv", 300 * MB),
            ("[Group] Anime - 02 [1080p].mkv", 300 * MB),
        ]);
        assert_eq!(select(&f, &episode(1, 2)), Ok(1));
    }

    #[test]
    fn multi_episode_file_matches_each_episode() {
        let f = files(&[("Show.S02E01E02.mkv", 900 * MB), ("Show.S02E03.mkv", 400 * MB)]);
        assert_eq!(select(&f, &episode(2, 2)), Ok(0));
    }

    #[test]
    fn filename_hint_wins_over_size() {
        let f = files(&[("pack/Show.S01E01.mkv", 900 * MB), ("pack/Show.S01E02.mkv", 400 * MB)]);
        let sel = Selector {
            filename_hint: Some("Show.S01E02.mkv".into()),
            ..Default::default()
        };
        assert_eq!(select(&f, &sel), Ok(1));
    }

    #[test]
    fn padding_files_are_ignored() {
        let mut f = files(&[("movie.mkv", 900 * MB), ("_____padding_file_0.mkv", 999 * MB)]);
        f[1].padding = true;
        assert_eq!(select(&f, &Selector::default()), Ok(0));
    }

    #[test]
    fn large_file_named_sample_is_still_the_feature() {
        let f = files(&[("The.Sample.2020.mkv", 1500 * MB), ("extras/poster.mp4", 2 * MB)]);
        assert_eq!(select(&f, &Selector::default()), Ok(0));
    }
}
