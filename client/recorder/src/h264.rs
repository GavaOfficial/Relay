pub const NAL_IDR: u8 = 5;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;

const AUD: [u8; 6] = [0, 0, 0, 1, NAL_AUD, 0xF0];

pub fn nal_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, i + 3));
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (n, &(_, begin)) in starts.iter().enumerate() {
        let mut end = starts.get(n + 1).map_or(data.len(), |s| s.0);
        while end > begin && data[end - 1] == 0 {
            end -= 1;
        }
        if end > begin {
            out.push(&data[begin..end]);
        }
    }
    out
}

pub fn nal_type(nal: &[u8]) -> u8 {
    nal.first().map_or(0, |b| b & 0x1F)
}

pub fn is_keyframe(access_unit: &[u8]) -> bool {
    nal_units(access_unit)
        .iter()
        .any(|n| nal_type(n) == NAL_IDR)
}

#[derive(Default)]
pub struct AccessUnits {
    sps: Option<Vec<u8>>,
    pps: Option<Vec<u8>>,
}

impl AccessUnits {
    pub fn remember(&mut self, sequence_header: &[u8]) {
        for nal in nal_units(sequence_header) {
            match nal_type(nal) {
                NAL_SPS => self.sps = Some(nal.to_vec()),
                NAL_PPS => self.pps = Some(nal.to_vec()),
                _ => {}
            }
        }
    }

    pub fn prepare(&mut self, access_unit: &[u8]) -> (Vec<u8>, bool) {
        let nals = nal_units(access_unit);
        let keyframe = nals.iter().any(|n| nal_type(n) == NAL_IDR);
        let has_headers = nals.iter().any(|n| nal_type(n) == NAL_SPS)
            && nals.iter().any(|n| nal_type(n) == NAL_PPS);
        let mut out = Vec::with_capacity(access_unit.len() + 64);
        out.extend_from_slice(&AUD);
        if keyframe && !has_headers {
            if let (Some(sps), Some(pps)) = (&self.sps, &self.pps) {
                for nal in [sps, pps] {
                    out.extend_from_slice(&[0, 0, 0, 1]);
                    out.extend_from_slice(nal);
                }
            }
        }
        for nal in nals {
            match nal_type(nal) {
                NAL_AUD => continue,
                NAL_SPS => self.sps = Some(nal.to_vec()),
                NAL_PPS => self.pps = Some(nal.to_vec()),
                _ => {}
            }
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(nal);
        }
        (out, keyframe)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPS: &[u8] = &[0x67, 0x64, 0x00, 0x28, 0xAC];
    const PPS: &[u8] = &[0x68, 0xEE, 0x3C, 0x80];
    const IDR: &[u8] = &[0x65, 0x88, 0x84, 0x00, 0x21];
    const SLICE: &[u8] = &[0x41, 0x9A, 0x00, 0x10];

    fn annexb(nals: &[&[u8]], long: bool) -> Vec<u8> {
        let mut v = Vec::new();
        for n in nals {
            if long {
                v.push(0);
            }
            v.extend_from_slice(&[0, 0, 1]);
            v.extend_from_slice(n);
        }
        v
    }

    #[test]
    fn splits_on_short_and_long_start_codes() {
        let mut data = annexb(&[SPS, PPS], true);
        data.extend(annexb(&[IDR], false));
        let nals = nal_units(&data);
        assert_eq!(nals, vec![SPS, PPS, IDR]);
        assert!(is_keyframe(&data));
        assert!(!is_keyframe(&annexb(&[SLICE], true)));
    }

    #[test]
    fn every_access_unit_starts_with_a_delimiter() {
        let mut au = AccessUnits::default();
        let (out, key) = au.prepare(&annexb(&[SLICE], true));
        assert!(!key);
        assert_eq!(&out[..6], &AUD);
        assert_eq!(nal_units(&out).len(), 2);

        let (out, _) = au.prepare(&annexb(&[&[NAL_AUD, 0xF0], SLICE], true));
        let types: Vec<u8> = nal_units(&out).iter().map(|n| nal_type(n)).collect();
        assert_eq!(types, vec![NAL_AUD, 1]);
    }

    #[test]
    fn keyframes_get_the_parameter_sets_they_miss() {
        let mut au = AccessUnits::default();
        au.remember(&annexb(&[SPS, PPS], true));
        let (out, key) = au.prepare(&annexb(&[IDR], true));
        assert!(key);
        let types: Vec<u8> = nal_units(&out).iter().map(|n| nal_type(n)).collect();
        assert_eq!(types, vec![NAL_AUD, NAL_SPS, NAL_PPS, NAL_IDR]);

        let (out, _) = au.prepare(&annexb(&[SPS, PPS, IDR], true));
        assert_eq!(nal_units(&out).len(), 4);
    }

    #[test]
    fn parameter_sets_seen_in_the_stream_are_reused() {
        let mut au = AccessUnits::default();
        au.prepare(&annexb(&[SPS, PPS, IDR], true));
        let (out, _) = au.prepare(&annexb(&[IDR], true));
        let types: Vec<u8> = nal_units(&out).iter().map(|n| nal_type(n)).collect();
        assert_eq!(types, vec![NAL_AUD, NAL_SPS, NAL_PPS, NAL_IDR]);
    }
}
