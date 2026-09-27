use std::{
    collections::VecDeque,
    fs::File,
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    time::Duration,
};

use crate::ts::{TsWriter, CLOCK};

const MAX_WAIT: u64 = CLOCK;
const MAX_AUDIO_WITHOUT_VIDEO: usize = 400;

pub fn playlist_name(generation: u32) -> String {
    if generation == 0 {
        "out.m3u8".into()
    } else {
        format!("out_{generation}.m3u8")
    }
}

pub fn segment_name(index: u64) -> String {
    format!("seg_{index:08}.ts")
}

pub fn replace(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= 20 => return Err(e),
            Err(_) => {
                attempt += 1;
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }
}

pub fn next_segment(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix("seg_")?
                .strip_suffix(".ts")?
                .parse::<u64>()
                .ok()
        })
        .max()
        .map_or(0, |m| m + 1)
}

pub struct VideoPacket {
    pub frame: u64,
    pub pts: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
}

pub struct AudioPacket {
    pub pts: u64,
    pub data: Vec<u8>,
}

struct Open {
    index: u64,
    slot: u64,
    start_pts: u64,
    last_pts: u64,
    tmp: PathBuf,
    file: BufWriter<File>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Summary {
    pub segments: Vec<(u64, f64)>,
    pub next_index: u64,
    pub late_keyframes: u64,
}

pub struct SegmentWriter {
    dir: PathBuf,
    playlist: String,
    frames_per_segment: u64,
    segment_secs: u32,
    frame_ticks: u64,
    next_index: u64,
    first_frame: Option<u64>,
    open: Option<Open>,
    closed: Vec<(u64, f64)>,
    ts: TsWriter,
    buf: Vec<u8>,
    audio: bool,
    video_queue: VecDeque<VideoPacket>,
    audio_queue: VecDeque<AudioPacket>,
    newest_video: Option<u64>,
    newest_audio: Option<u64>,
    late_keyframes: u64,
}

impl SegmentWriter {
    pub fn new(
        dir: &Path,
        playlist: String,
        first_index: u64,
        fps: u32,
        segment_secs: u32,
        audio: bool,
    ) -> Self {
        Self {
            dir: dir.to_path_buf(),
            playlist,
            frames_per_segment: fps as u64 * segment_secs as u64,
            segment_secs,
            frame_ticks: CLOCK / fps as u64,
            next_index: first_index,
            first_frame: None,
            open: None,
            closed: Vec::new(),
            ts: TsWriter::new(audio),
            buf: Vec::with_capacity(512 * 1024),
            audio,
            video_queue: VecDeque::new(),
            audio_queue: VecDeque::new(),
            newest_video: None,
            newest_audio: None,
            late_keyframes: 0,
        }
    }

    pub fn push_video(&mut self, p: VideoPacket) -> io::Result<()> {
        self.newest_video = Some(self.newest_video.map_or(p.pts, |n| n.max(p.pts)));
        self.video_queue.push_back(p);
        self.drain(false)
    }

    pub fn push_audio(&mut self, p: AudioPacket) -> io::Result<()> {
        if !self.audio {
            return Ok(());
        }
        self.newest_audio = Some(self.newest_audio.map_or(p.pts, |n| n.max(p.pts)));
        self.audio_queue.push_back(p);
        self.drain(false)
    }

    pub fn next_index(&self) -> u64 {
        self.open.as_ref().map_or(self.next_index, |o| o.index + 1)
    }

    fn drain(&mut self, force: bool) -> io::Result<()> {
        let newest = self.newest_video.max(self.newest_audio).unwrap_or(0);
        loop {
            let v = self.video_queue.front().map(|p| p.pts);
            let a = self.audio_queue.front().map(|p| p.pts);
            let take_video = match (v, a) {
                (None, None) => return Ok(()),
                (Some(_), None) => true,
                (None, Some(_)) => false,
                (Some(v), Some(a)) => v <= a,
            };
            if take_video {
                let pts = v.unwrap();
                let ready = force
                    || !self.audio
                    || self.newest_audio.is_some_and(|n| n >= pts)
                    || newest.saturating_sub(pts) > MAX_WAIT;
                if !ready {
                    return Ok(());
                }
                let p = self.video_queue.pop_front().unwrap();
                self.write_video(p)?;
            } else {
                let pts = a.unwrap();
                if self.open.is_none() {
                    if force || self.audio_queue.len() > MAX_AUDIO_WITHOUT_VIDEO {
                        self.audio_queue.pop_front();
                        continue;
                    }
                    return Ok(());
                }
                let ready = force
                    || self.newest_video.is_some_and(|n| n >= pts)
                    || newest.saturating_sub(pts) > MAX_WAIT;
                if !ready {
                    return Ok(());
                }
                let p = self.audio_queue.pop_front().unwrap();
                self.write_audio(p)?;
            }
        }
    }

    fn write_video(&mut self, p: VideoPacket) -> io::Result<()> {
        let first = match self.first_frame {
            Some(f) => f,
            None => {
                if !p.keyframe {
                    return Ok(());
                }
                self.first_frame = Some(p.frame);
                p.frame
            }
        };
        let slot = p.frame.saturating_sub(first) / self.frames_per_segment;
        match self.open.as_ref().map(|o| o.slot) {
            None => self.open_segment(slot, p.pts)?,
            Some(current) if slot > current => {
                if p.keyframe {
                    self.close_segment(p.pts)?;
                    self.open_segment(slot, p.pts)?;
                } else if (p.frame - first).is_multiple_of(self.frames_per_segment) {
                    self.late_keyframes += 1;
                }
            }
            Some(_) => {}
        }
        self.ts.video(&mut self.buf, p.pts, p.keyframe, &p.data);
        let open = self.open.as_mut().unwrap();
        open.last_pts = p.pts;
        open.file.write_all(&self.buf)?;
        self.buf.clear();
        Ok(())
    }

    fn write_audio(&mut self, p: AudioPacket) -> io::Result<()> {
        let Some(open) = self.open.as_mut() else {
            return Ok(());
        };
        if p.pts < open.start_pts && self.closed.is_empty() {
            return Ok(());
        }
        self.ts.audio(&mut self.buf, p.pts, &p.data);
        let open = self.open.as_mut().unwrap();
        open.file.write_all(&self.buf)?;
        self.buf.clear();
        Ok(())
    }

    fn open_segment(&mut self, slot: u64, pts: u64) -> io::Result<()> {
        let index = self.next_index();
        std::fs::create_dir_all(&self.dir)?;
        let tmp = self.dir.join(format!("{}.tmp", segment_name(index)));
        let mut file = BufWriter::with_capacity(256 * 1024, File::create(&tmp)?);
        self.ts.tables(&mut self.buf);
        file.write_all(&self.buf)?;
        self.buf.clear();
        self.open = Some(Open {
            index,
            slot,
            start_pts: pts,
            last_pts: pts,
            tmp,
            file,
        });
        Ok(())
    }

    fn close_segment(&mut self, end_pts: u64) -> io::Result<()> {
        let Some(open) = self.open.take() else {
            return Ok(());
        };
        let Open {
            index,
            start_pts,
            tmp,
            mut file,
            ..
        } = open;
        file.flush()?;
        drop(file);
        replace(&tmp, &self.dir.join(segment_name(index)))?;
        let secs = end_pts.saturating_sub(start_pts) as f64 / CLOCK as f64;
        self.closed.push((index, secs));
        self.next_index = index + 1;
        self.write_playlist(false)
    }

    fn write_playlist(&self, end: bool) -> io::Result<()> {
        let longest = self.closed.iter().map(|c| c.1).fold(0.0, f64::max);
        let target = (longest.ceil() as u32).max(self.segment_secs);
        let first = self.closed.first().map_or(self.next_index, |c| c.0);
        let mut text = format!(
            "#EXTM3U\n#EXT-X-VERSION:3\n#EXT-X-TARGETDURATION:{target}\n#EXT-X-MEDIA-SEQUENCE:{first}\n#EXT-X-INDEPENDENT-SEGMENTS\n"
        );
        for (index, secs) in &self.closed {
            text.push_str(&format!("#EXTINF:{secs:.6},\n{}\n", segment_name(*index)));
        }
        if end {
            text.push_str("#EXT-X-ENDLIST\n");
        }
        let path = self.dir.join(&self.playlist);
        let tmp = self.dir.join(format!("{}.tmp", self.playlist));
        std::fs::write(&tmp, text)?;
        replace(&tmp, &path)
    }

    pub fn finish(mut self, cutoff: Option<u64>) -> io::Result<(Summary, Vec<AudioPacket>)> {
        let mut carry = Vec::new();
        if let Some(c) = cutoff {
            while self.audio_queue.back().is_some_and(|p| p.pts >= c) {
                carry.push(self.audio_queue.pop_back().unwrap());
            }
            carry.reverse();
        }
        self.drain(true)?;
        if let Some(open) = &self.open {
            let end = open.last_pts + self.frame_ticks;
            self.close_segment(end)?;
        }
        if !self.closed.is_empty() {
            self.write_playlist(true)?;
        }
        Ok((
            Summary {
                segments: self.closed.clone(),
                next_index: self.next_index,
                late_keyframes: self.late_keyframes,
            },
            carry,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts::demux::demux;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("relay-recorder-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn frame(k: u64, fps: u64, key_every: u64) -> VideoPacket {
        VideoPacket {
            frame: k,
            pts: 126_000 + k * CLOCK / fps,
            keyframe: k.is_multiple_of(key_every),
            data: vec![0, 0, 0, 1, 0x09, 0xF0, 0, 0, 0, 1, 0x41, k as u8],
        }
    }

    fn audio(i: u64) -> AudioPacket {
        AudioPacket {
            pts: 126_000 + i * 1920,
            data: vec![0xFF, 0xF1, 0x4C, 0x80, 0x01, 0x3F, 0xFC, i as u8],
        }
    }

    fn feed(w: &mut SegmentWriter, fps: u64, frames: u64, key_every: u64) {
        let mut next_audio = 0;
        for k in 0..frames {
            w.push_video(frame(k, fps, key_every)).unwrap();
            let t = (k + 1) * CLOCK / fps;
            while next_audio * 1920 < t {
                w.push_audio(audio(next_audio)).unwrap();
                next_audio += 1;
            }
        }
    }

    fn read_playlist(dir: &Path, name: &str) -> String {
        std::fs::read_to_string(dir.join(name)).unwrap()
    }

    #[test]
    fn segments_have_the_exact_length_of_the_grid() {
        let dir = temp_dir("grid");
        let mut w = SegmentWriter::new(&dir, playlist_name(0), 0, 30, 4, true);
        feed(&mut w, 30, 30 * 10, 120);
        let (summary, carry) = w.finish(None).unwrap();
        assert!(carry.is_empty());
        assert_eq!(summary.segments, vec![(0, 4.0), (1, 4.0), (2, 2.0)]);
        assert_eq!(summary.next_index, 3);

        let p = read_playlist(&dir, "out.m3u8");
        assert!(p.starts_with("#EXTM3U\n"));
        assert!(
            p.contains("#EXTINF:4.000000,\nseg_00000000.ts\n#EXTINF:4.000000,\nseg_00000001.ts\n")
        );
        assert!(p.ends_with("#EXTINF:2.000000,\nseg_00000002.ts\n#EXT-X-ENDLIST\n"));
        assert!(!dir.join("out.m3u8.tmp").exists());

        for (i, n) in [(0u64, 120usize), (1, 120), (2, 60)] {
            let data = std::fs::read(dir.join(segment_name(i))).unwrap();
            let d = demux(&data);
            assert_eq!(d.pat, 1);
            assert_eq!(d.pmt_streams.len(), 2);
            assert_eq!(d.video.pes.len(), n);
            assert!(
                d.video.pes[0].1,
                "il pezzo {i} non inizia con un fotogramma chiave"
            );
            let first = d.video.pes[0].0;
            assert_eq!(first, 126_000 + i * 4 * CLOCK);
            assert!(d
                .audio
                .pes
                .iter()
                .all(|a| a.0 >= first && a.0 < first + 4 * CLOCK));
            assert!(!d.audio.pes.is_empty());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn playlist_is_readable_while_recording() {
        let dir = temp_dir("live");
        let mut w = SegmentWriter::new(&dir, playlist_name(3), 7, 10, 1, false);
        feed(&mut w, 10, 25, 10);
        let p = read_playlist(&dir, "out_3.m3u8");
        assert!(p.contains("#EXT-X-MEDIA-SEQUENCE:7\n"));
        assert!(p.contains("seg_00000007.ts") && p.contains("seg_00000008.ts"));
        assert!(!p.contains("#EXT-X-ENDLIST"));
        assert!(dir.join("seg_00000009.ts.tmp").exists());
        assert!(!dir.join("seg_00000009.ts").exists());
        let (s, _) = w.finish(None).unwrap();
        assert_eq!(s.segments.last(), Some(&(9, 0.5)));
        assert!(dir.join("seg_00000009.ts").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_keyframe_delays_the_cut_instead_of_breaking_the_segment() {
        let dir = temp_dir("late");
        let mut w = SegmentWriter::new(&dir, playlist_name(0), 0, 10, 1, false);
        for k in 0..30u64 {
            let mut f = frame(k, 10, 1000);
            f.keyframe = k == 0 || k == 13 || k == 20;
            w.push_video(f).unwrap();
        }
        let (s, _) = w.finish(None).unwrap();
        assert_eq!(s.segments, vec![(0, 1.3), (1, 0.7), (2, 1.0)]);
        assert_eq!(s.late_keyframes, 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn video_is_never_blocked_forever_by_missing_audio() {
        let dir = temp_dir("mute");
        let mut w = SegmentWriter::new(&dir, playlist_name(0), 0, 10, 1, true);
        for k in 0..25 {
            w.push_video(frame(k, 10, 10)).unwrap();
        }
        assert!(dir.join(segment_name(0)).exists());
        let (s, _) = w.finish(None).unwrap();
        assert_eq!(s.segments.len(), 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn audio_after_the_cutoff_moves_to_the_next_generation() {
        let dir = temp_dir("carry");
        let mut w = SegmentWriter::new(&dir, playlist_name(0), 0, 10, 1, true);
        for k in 0..10 {
            w.push_video(frame(k, 10, 10)).unwrap();
        }
        for i in 0..60 {
            w.push_audio(audio(i)).unwrap();
        }
        let cutoff = 126_000 + 10 * CLOCK / 10;
        let (s, carry) = w.finish(Some(cutoff)).unwrap();
        assert_eq!(s.segments, vec![(0, 1.0)]);
        assert!(!carry.is_empty());
        assert!(carry.iter().all(|a| a.pts >= cutoff));
        let d = demux(&std::fs::read(dir.join(segment_name(0))).unwrap());
        assert!(d.audio.pes.iter().all(|a| a.0 < cutoff));
        assert_eq!(d.audio.pes.len() + carry.len(), 60);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_segment_continues_after_the_highest_file() {
        let dir = temp_dir("next");
        assert_eq!(next_segment(&dir), 0);
        std::fs::write(dir.join("seg_00000004.ts"), b"").unwrap();
        std::fs::write(dir.join("seg_00000009.ts.tmp"), b"").unwrap();
        assert_eq!(next_segment(&dir), 5);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
