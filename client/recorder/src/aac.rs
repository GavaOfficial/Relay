pub const SAMPLES_PER_FRAME: u64 = 1024;

fn frequency_index(rate: u32) -> u8 {
    const RATES: [u32; 13] = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ];
    RATES.iter().position(|&r| r == rate).unwrap_or(3) as u8
}

pub fn adts_header(payload: usize, rate: u32, channels: u8) -> [u8; 7] {
    let len = payload + 7;
    let profile = 1u8;
    let freq = frequency_index(rate);
    [
        0xFF,
        0xF1,
        (profile << 6) | (freq << 2) | ((channels >> 2) & 1),
        ((channels & 3) << 6) | ((len >> 11) as u8 & 0x03),
        (len >> 3) as u8,
        (((len & 7) as u8) << 5) | 0x1F,
        0xFC,
    ]
}

pub fn is_adts(frame: &[u8]) -> bool {
    frame.len() >= 7 && frame[0] == 0xFF && frame[1] & 0xF6 == 0xF0
}

pub fn to_adts(frame: &[u8], rate: u32, channels: u8) -> Vec<u8> {
    if is_adts(frame) {
        return frame.to_vec();
    }
    let mut out = Vec::with_capacity(frame.len() + 7);
    out.extend_from_slice(&adts_header(frame.len(), rate, channels));
    out.extend_from_slice(frame);
    out
}

pub fn adts_frame_length(header: &[u8]) -> usize {
    (((header[3] & 0x03) as usize) << 11) | ((header[4] as usize) << 3) | (header[5] as usize >> 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_describes_lc_stereo_48k() {
        let h = adts_header(371, 48000, 2);
        assert_eq!(&h[..3], &[0xFF, 0xF1, 0x4C]);
        assert_eq!(h[3] >> 6, 2);
        assert_eq!(adts_frame_length(&h), 378);
        assert!(is_adts(&h));
    }

    #[test]
    fn raw_frames_get_a_header_once() {
        let raw = vec![0x21u8, 0x10, 0x05];
        let framed = to_adts(&raw, 48000, 2);
        assert_eq!(framed.len(), 10);
        assert_eq!(to_adts(&framed, 48000, 2), framed);
    }
}
