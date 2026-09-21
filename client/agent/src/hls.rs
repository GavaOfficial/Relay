use std::collections::HashMap;

pub fn segment_index(name: &str) -> Option<u64> {
    name.strip_prefix("seg_")?.strip_suffix(".ts")?.parse().ok()
}

pub fn playlist_indices(playlist: &str) -> Vec<u64> {
    playlist
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| segment_file_index(l.trim()))
        .collect()
}

fn segment_file_index(name: &str) -> Option<u64> {
    let base = name.rsplit('/').next()?;
    let digits = base.strip_suffix(".ts")?;
    digits.strip_prefix("seg_").unwrap_or(digits).parse().ok()
}

pub fn missing_runs(known: &std::collections::BTreeSet<u64>, upto: u64) -> Vec<(u64, u64)> {
    let mut runs = Vec::new();
    let mut start: Option<u64> = None;
    for n in 0..upto {
        match (known.contains(&n), start) {
            (false, None) => start = Some(n),
            (true, Some(s)) => {
                runs.push((s, n));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(s) = start {
        runs.push((s, upto));
    }
    runs
}

pub const MIN_FULL_SEGMENT_MS: u32 = 3_900;

pub fn playlist_name(generation: u32) -> String {
    if generation == 0 {
        "out.m3u8".into()
    } else {
        format!("out_{generation}.m3u8")
    }
}

pub fn playlist_generation(name: &str) -> u32 {
    name.strip_prefix("out_")
        .and_then(|s| s.strip_suffix(".m3u8"))
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

pub fn is_playlist(name: &str) -> bool {
    name == "out.m3u8" || (name.starts_with("out_") && name.ends_with(".m3u8"))
}

pub fn parse_durations(playlist: &str) -> HashMap<u64, u32> {
    let mut out = HashMap::new();
    let mut pending: Option<u32> = None;
    for line in playlist.lines() {
        if let Some(rest) = line.strip_prefix("#EXTINF:") {
            let secs: f64 = rest
                .split(',')
                .next()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0.0);
            pending = Some((secs * 1000.0).round() as u32);
        } else if !line.starts_with('#') {
            if let (Some(ms), Some(n)) = (pending.take(), segment_file_index(line.trim())) {
                out.insert(n, ms);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_playlist() {
        let p = "#EXTM3U\n#EXTINF:4.000000,\nseg_00000000.ts\n#EXTINF:2.033000,\nseg_00000001.ts\n#EXT-X-ENDLIST\n";
        let d = parse_durations(p);
        assert_eq!(d[&0], 4000);
        assert_eq!(d[&1], 2033);
    }

    #[test]
    fn missing_runs_finds_holes() {
        let known: std::collections::BTreeSet<u64> = [0, 1, 4, 5].into_iter().collect();
        assert_eq!(missing_runs(&known, 8), vec![(2, 4), (6, 8)]);
        assert_eq!(missing_runs(&known, 2), vec![]);
        assert_eq!(missing_runs(&Default::default(), 3), vec![(0, 3)]);
    }

    #[test]
    fn server_playlist_indices() {
        let p = "#EXTM3U\n#EXTINF:4.000,\nsegments/00000000.ts\n#EXT-X-DISCONTINUITY\n#EXTINF:4.000,\nsegments/00000003.ts\n";
        assert_eq!(playlist_indices(p), vec![0, 3]);
    }

    #[test]
    fn index() {
        assert_eq!(segment_index("seg_00000007.ts"), Some(7));
        assert_eq!(segment_index("seg_00000007.ts.tmp"), None);
        assert_eq!(segment_index("out.m3u8"), None);
    }
}
