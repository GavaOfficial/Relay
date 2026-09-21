use std::collections::BTreeMap;

use relay_common::SEGMENT_SECONDS;

pub fn segment_file(n: u64) -> String {
    format!("{n:08}.ts")
}

pub fn parse_segment(s: &str) -> Option<u64> {
    s.strip_suffix(".ts").unwrap_or(s).parse().ok()
}

pub fn build(segments: &BTreeMap<u64, u32>, ended: bool) -> String {
    let max_ms = segments
        .values()
        .copied()
        .max()
        .unwrap_or(0)
        .max(SEGMENT_SECONDS * 1000);
    let mut out = String::from("#EXTM3U\n#EXT-X-VERSION:3\n");
    out.push_str(&format!(
        "#EXT-X-TARGETDURATION:{}\n",
        max_ms.div_ceil(1000)
    ));
    out.push_str("#EXT-X-MEDIA-SEQUENCE:0\n");
    out.push_str(if ended {
        "#EXT-X-PLAYLIST-TYPE:VOD\n"
    } else {
        "#EXT-X-PLAYLIST-TYPE:EVENT\n"
    });

    let mut push = |n: u64, ms: u32, discontinuity: bool| {
        if discontinuity {
            out.push_str("#EXT-X-DISCONTINUITY\n");
        }
        out.push_str(&format!(
            "#EXTINF:{}.{:03},\nsegments/{}\n",
            ms / 1000,
            ms % 1000,
            segment_file(n)
        ));
    };

    if ended {
        let mut prev: Option<u64> = None;
        for (&n, &ms) in segments {
            push(n, ms, matches!(prev, Some(p) if n != p + 1));
            prev = Some(n);
        }
        out.push_str("#EXT-X-ENDLIST\n");
    } else {
        let mut n = 0;
        while let Some(&ms) = segments.get(&n) {
            push(n, ms, false);
            n += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(v: &[u64]) -> BTreeMap<u64, u32> {
        v.iter().map(|&n| (n, 4000)).collect()
    }

    #[test]
    fn live_stops_at_first_gap() {
        let p = build(&set(&[0, 1, 3]), false);
        assert!(p.contains("00000001.ts"));
        assert!(!p.contains("00000003.ts"));
        assert!(!p.contains("ENDLIST"));
    }

    #[test]
    fn ended_includes_all_with_discontinuity() {
        let p = build(&set(&[0, 1, 3]), true);
        assert!(p.contains("00000003.ts"));
        assert_eq!(p.matches("DISCONTINUITY").count(), 1);
        assert!(p.ends_with("#EXT-X-ENDLIST\n"));
    }

    #[test]
    fn uses_real_duration() {
        let mut s = set(&[0, 1]);
        s.insert(2, 1500);
        let p = build(&s, true);
        assert!(p.contains("#EXTINF:1.500,"));
        assert!(p.contains("#EXT-X-TARGETDURATION:4"));
    }

    #[test]
    fn parse() {
        assert_eq!(parse_segment("00000012.ts"), Some(12));
        assert_eq!(parse_segment("x.ts"), None);
    }
}
