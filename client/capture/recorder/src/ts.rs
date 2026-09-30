pub const PACKET: usize = 188;
pub const CLOCK: u64 = 90_000;
pub const PAT_PID: u16 = 0x0000;
pub const PMT_PID: u16 = 0x1000;
pub const VIDEO_PID: u16 = 0x0100;
pub const AUDIO_PID: u16 = 0x0101;

const STREAM_H264: u8 = 0x1B;
const STREAM_AAC_ADTS: u8 = 0x0F;
const PCR_DELAY: u64 = CLOCK / 10;
const PTS_MASK: u64 = (1 << 33) - 1;

pub fn crc32_mpeg(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 {
                (crc << 1) ^ 0x04C1_1DB7
            } else {
                crc << 1
            };
        }
    }
    crc
}

fn timestamp(prefix: u8, ts: u64) -> [u8; 5] {
    let ts = ts & PTS_MASK;
    [
        (prefix << 4) | (((ts >> 30) as u8 & 0x07) << 1) | 1,
        (ts >> 22) as u8,
        (((ts >> 15) as u8) << 1) | 1,
        (ts >> 7) as u8,
        ((ts as u8) << 1) | 1,
    ]
}

pub fn read_timestamp(b: &[u8]) -> u64 {
    (((b[0] as u64 >> 1) & 0x07) << 30)
        | ((b[1] as u64) << 22)
        | ((b[2] as u64 >> 1) << 15)
        | ((b[3] as u64) << 7)
        | (b[4] as u64 >> 1)
}

fn pcr_bytes(base: u64) -> [u8; 6] {
    let base = base & PTS_MASK;
    [
        (base >> 25) as u8,
        (base >> 17) as u8,
        (base >> 9) as u8,
        (base >> 1) as u8,
        (((base & 1) as u8) << 7) | 0x7E,
        0,
    ]
}

pub struct TsWriter {
    audio: bool,
    cc: [u8; 4],
}

impl TsWriter {
    pub fn new(audio: bool) -> Self {
        Self { audio, cc: [0; 4] }
    }

    fn next_cc(&mut self, pid: u16) -> u8 {
        let slot = match pid {
            PAT_PID => 0,
            PMT_PID => 1,
            VIDEO_PID => 2,
            _ => 3,
        };
        let cc = self.cc[slot];
        self.cc[slot] = (cc + 1) & 0x0F;
        cc
    }

    fn section(&mut self, out: &mut Vec<u8>, pid: u16, section: &[u8]) {
        let start = out.len();
        let cc = self.next_cc(pid);
        out.extend_from_slice(&[0x47, 0x40 | (pid >> 8) as u8, pid as u8, 0x10 | cc, 0x00]);
        out.extend_from_slice(section);
        out.extend_from_slice(&crc32_mpeg(section).to_be_bytes());
        out.resize(start + PACKET, 0xFF);
    }

    pub fn tables(&mut self, out: &mut Vec<u8>) {
        let pat = [
            0x00,
            0xB0,
            13,
            0x00,
            0x01,
            0xC1,
            0x00,
            0x00,
            0x00,
            0x01,
            0xE0 | (PMT_PID >> 8) as u8,
            PMT_PID as u8,
        ];
        self.section(out, PAT_PID, &pat);

        let mut streams: Vec<(u8, u16)> = vec![(STREAM_H264, VIDEO_PID)];
        if self.audio {
            streams.push((STREAM_AAC_ADTS, AUDIO_PID));
        }
        let length = 9 + 5 * streams.len() + 4;
        let mut pmt = vec![
            0x02,
            0xB0 | (length >> 8) as u8,
            length as u8,
            0x00,
            0x01,
            0xC1,
            0x00,
            0x00,
            0xE0 | (VIDEO_PID >> 8) as u8,
            VIDEO_PID as u8,
            0xF0,
            0x00,
        ];
        for (kind, pid) in streams {
            pmt.extend_from_slice(&[kind, 0xE0 | (pid >> 8) as u8, pid as u8, 0xF0, 0x00]);
        }
        self.section(out, PMT_PID, &pmt);
    }

    pub fn video(&mut self, out: &mut Vec<u8>, pts: u64, keyframe: bool, access_unit: &[u8]) {
        let mut pes = Vec::with_capacity(access_unit.len() + 14);
        pes.extend_from_slice(&[0x00, 0x00, 0x01, 0xE0, 0x00, 0x00, 0x84, 0x80, 0x05]);
        pes.extend_from_slice(&timestamp(0x2, pts));
        pes.extend_from_slice(access_unit);
        let pcr = pts.wrapping_sub(PCR_DELAY) & PTS_MASK;
        self.packetize(out, VIDEO_PID, &pes, Some(pcr), keyframe);
    }

    pub fn audio(&mut self, out: &mut Vec<u8>, pts: u64, adts_frames: &[u8]) {
        let mut pes = Vec::with_capacity(adts_frames.len() + 14);
        let length = adts_frames.len() + 8;
        let length = if length > 0xFFFF { 0 } else { length };
        pes.extend_from_slice(&[
            0x00,
            0x00,
            0x01,
            0xC0,
            (length >> 8) as u8,
            length as u8,
            0x80,
            0x80,
            0x05,
        ]);
        pes.extend_from_slice(&timestamp(0x2, pts));
        pes.extend_from_slice(adts_frames);
        self.packetize(out, AUDIO_PID, &pes, None, false);
    }

    fn packetize(
        &mut self,
        out: &mut Vec<u8>,
        pid: u16,
        pes: &[u8],
        pcr: Option<u64>,
        random_access: bool,
    ) {
        let mut pos = 0;
        let mut first = true;
        while pos < pes.len() {
            let start = out.len();
            let remaining = pes.len() - pos;
            let mut flags = 0u8;
            let mut fields: Vec<u8> = Vec::new();
            if first {
                if random_access {
                    flags |= 0x40;
                }
                if let Some(p) = pcr {
                    flags |= 0x10;
                    fields.extend_from_slice(&pcr_bytes(p));
                }
            }
            let wants_field = flags != 0;
            let field_len = if wants_field { 2 + fields.len() } else { 0 };
            let room = 184 - field_len;
            let (field_len, take) = if remaining >= room {
                (field_len, room)
            } else {
                (184 - remaining, remaining)
            };
            let cc = self.next_cc(pid);
            let control = if field_len > 0 { 0x30 } else { 0x10 };
            out.extend_from_slice(&[
                0x47,
                (if first { 0x40 } else { 0x00 }) | (pid >> 8) as u8,
                pid as u8,
                control | cc,
            ]);
            if field_len > 0 {
                out.push((field_len - 1) as u8);
                if field_len > 1 {
                    out.push(flags);
                    out.extend_from_slice(&fields);
                    out.resize(start + 4 + field_len, 0xFF);
                }
            }
            out.extend_from_slice(&pes[pos..pos + take]);
            debug_assert_eq!(out.len() - start, PACKET);
            pos += take;
            first = false;
        }
    }
}

#[cfg(test)]
pub(crate) mod demux {
    use super::*;

    #[derive(Debug, Default)]
    pub struct Stream {
        pub pes: Vec<(u64, bool, Vec<u8>)>,
        pub pcr: Vec<u64>,
    }

    #[derive(Debug, Default)]
    pub struct Demuxed {
        pub pat: usize,
        pub pmt_streams: Vec<(u8, u16)>,
        pub video: Stream,
        pub audio: Stream,
    }

    pub fn demux(data: &[u8]) -> Demuxed {
        assert_eq!(data.len() % PACKET, 0, "lunghezza non multipla di 188");
        let mut d = Demuxed::default();
        let mut cc: std::collections::HashMap<u16, u8> = Default::default();
        let mut current: std::collections::HashMap<u16, (bool, Vec<u8>)> = Default::default();
        for p in data.chunks(PACKET) {
            assert_eq!(p[0], 0x47);
            let pusi = p[1] & 0x40 != 0;
            let pid = (((p[1] & 0x1F) as u16) << 8) | p[2] as u16;
            let control = p[3] >> 4;
            let counter = p[3] & 0x0F;
            if let Some(prev) = cc.insert(pid, counter) {
                assert_eq!(
                    counter,
                    (prev + 1) & 0x0F,
                    "contatore saltato sul pid {pid:#x}"
                );
            }
            let mut i = 4;
            let mut random_access = false;
            if control & 0x2 != 0 {
                let len = p[4] as usize;
                if len > 0 {
                    let flags = p[5];
                    random_access = flags & 0x40 != 0;
                    if flags & 0x10 != 0 {
                        let b = &p[6..12];
                        let base = ((b[0] as u64) << 25)
                            | ((b[1] as u64) << 17)
                            | ((b[2] as u64) << 9)
                            | ((b[3] as u64) << 1)
                            | (b[4] as u64 >> 7);
                        if pid == VIDEO_PID {
                            d.video.pcr.push(base);
                        }
                    }
                }
                i = 5 + len;
            }
            let payload = &p[i..];
            match pid {
                PAT_PID => {
                    let s = &payload[1..];
                    let len = (((s[1] & 0x0F) as usize) << 8) | s[2] as usize;
                    assert_eq!(crc32_mpeg(&s[..3 + len]), 0, "CRC del PAT");
                    d.pat += 1;
                }
                PMT_PID => {
                    let s = &payload[1..];
                    let len = (((s[1] & 0x0F) as usize) << 8) | s[2] as usize;
                    assert_eq!(crc32_mpeg(&s[..3 + len]), 0, "CRC del PMT");
                    let mut j = 12;
                    d.pmt_streams.clear();
                    while j + 5 <= 3 + len - 4 {
                        let pid = (((s[j + 1] & 0x1F) as u16) << 8) | s[j + 2] as u16;
                        d.pmt_streams.push((s[j], pid));
                        j += 5;
                    }
                }
                VIDEO_PID | AUDIO_PID => {
                    if pusi {
                        if let Some((ra, buf)) = current.remove(&pid) {
                            push_pes(&mut d, pid, ra, &buf);
                        }
                        current.insert(pid, (random_access, payload.to_vec()));
                    } else {
                        current
                            .get_mut(&pid)
                            .expect("dati senza inizio PES")
                            .1
                            .extend_from_slice(payload);
                    }
                }
                other => panic!("pid inatteso {other:#x}"),
            }
        }
        for (pid, (ra, buf)) in current {
            push_pes(&mut d, pid, ra, &buf);
        }
        d
    }

    fn push_pes(d: &mut Demuxed, pid: u16, random_access: bool, buf: &[u8]) {
        assert_eq!(&buf[..3], &[0, 0, 1]);
        let declared = u16::from_be_bytes([buf[4], buf[5]]) as usize;
        let header = 9 + buf[8] as usize;
        let pts = read_timestamp(&buf[9..14]);
        let body = buf[header..].to_vec();
        if declared != 0 {
            assert_eq!(declared, buf.len() - 6, "lunghezza PES");
        }
        let s = if pid == VIDEO_PID {
            &mut d.video
        } else {
            &mut d.audio
        };
        s.pes.push((pts, random_access, body));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_mpeg_table() {
        let pat = [
            0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00, 0x01, 0xF0, 0x00,
        ];
        assert_eq!(crc32_mpeg(&pat), 0x2AB1_04B2);
    }

    #[test]
    fn tables_match_what_ffmpeg_writes() {
        let mut w = TsWriter::new(true);
        let mut out = Vec::new();
        w.tables(&mut out);
        assert_eq!(out.len(), 2 * PACKET);
        assert_eq!(
            &out[..21],
            &[
                0x47, 0x40, 0x00, 0x10, 0x00, 0x00, 0xB0, 0x0D, 0x00, 0x01, 0xC1, 0x00, 0x00, 0x00,
                0x01, 0xF0, 0x00, 0x2A, 0xB1, 0x04, 0xB2
            ]
        );
        let d = demux::demux(&out);
        assert_eq!(d.pat, 1);
        assert_eq!(
            d.pmt_streams,
            vec![(STREAM_H264, VIDEO_PID), (STREAM_AAC_ADTS, AUDIO_PID)]
        );
    }

    #[test]
    fn timestamps_survive_encoding() {
        for ts in [0u64, 1, 126_000, 90_000 * 3600 * 20, PTS_MASK] {
            assert_eq!(read_timestamp(&timestamp(0x2, ts)), ts);
        }
    }

    #[test]
    fn frames_of_any_size_come_back_intact() {
        let mut w = TsWriter::new(true);
        let mut out = Vec::new();
        w.tables(&mut out);
        let sizes = [
            1usize, 150, 170, 175, 176, 177, 183, 184, 185, 400, 5000, 70_000,
        ];
        for (i, &n) in sizes.iter().enumerate() {
            let data: Vec<u8> = (0..n).map(|b| (b * 7 + i) as u8).collect();
            w.video(&mut out, 126_000 + i as u64 * 1500, i % 3 == 0, &data);
            w.audio(&mut out, 126_000 + i as u64 * 1920, &data[..n.min(2000)]);
        }
        let d = demux::demux(&out);
        assert_eq!(d.video.pes.len(), sizes.len());
        assert_eq!(d.audio.pes.len(), sizes.len());
        for (i, &n) in sizes.iter().enumerate() {
            let data: Vec<u8> = (0..n).map(|b| (b * 7 + i) as u8).collect();
            let (pts, ra, body) = &d.video.pes[i];
            assert_eq!(*pts, 126_000 + i as u64 * 1500);
            assert_eq!(*ra, i % 3 == 0);
            assert_eq!(body, &data);
            let (apts, _, abody) = &d.audio.pes[i];
            assert_eq!(*apts, 126_000 + i as u64 * 1920);
            assert_eq!(abody.as_slice(), &data[..n.min(2000)]);
        }
        assert_eq!(d.video.pcr.len(), sizes.len());
        assert!(d
            .video
            .pcr
            .iter()
            .zip(&d.video.pes)
            .all(|(pcr, (pts, _, _))| pcr + PCR_DELAY == *pts));
    }
}
