//! Annex-B start codes → raw NAL payloads.

/// Length of start code at `i` (`00 00 01` or `00 00 00 01`), or None.
pub fn start_code_len(buf: &[u8], i: usize) -> Option<usize> {
    if i + 4 <= buf.len() && buf[i..i + 4] == [0, 0, 0, 1] {
        return Some(4);
    }
    if i + 3 <= buf.len() && buf[i..i + 3] == [0, 0, 1] {
        return Some(3);
    }
    None
}

fn next_start_code(buf: &[u8], search_from: usize) -> Option<usize> {
    let mut i = search_from;
    while i + 2 < buf.len() {
        if start_code_len(buf, i).is_some() {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Split Annex-B into individual NAL RBSP blobs (no start codes).
pub fn nal_units(buf: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < buf.len() {
        let Some(sc) = start_code_len(buf, i) else {
            i += 1;
            continue;
        };
        let nal_from = i + sc;
        let nal_to = next_start_code(buf, nal_from).unwrap_or(buf.len());
        if nal_from < nal_to {
            out.push(buf[nal_from..nal_to].to_vec());
        }
        i = nal_to;
    }
    out
}

pub fn nal_type(nal: &[u8]) -> Option<u8> {
    nal.first().map(|b| b & 0x1f)
}

/// AVCC sample: each NAL prefixed with 4-byte big-endian length (no start codes).
pub fn nal_units_to_avcc_sample(nals: &[Vec<u8>]) -> Vec<u8> {
    let mut v = Vec::new();
    for nal in nals {
        let len = nal.len() as u32;
        v.extend_from_slice(&len.to_be_bytes());
        v.extend_from_slice(nal);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::{nal_type, nal_units, nal_units_to_avcc_sample};

    #[test]
    fn parses_mixed_start_codes() {
        let annexb = [
            0, 0, 0, 1, 0x67, 0x11, 0x22, // SPS
            0, 0, 1, 0x68, 0x33, 0x44, // PPS
            0, 0, 0, 1, 0x65, 0xaa, 0xbb, // IDR
        ];
        let nals = nal_units(&annexb);
        assert_eq!(nals.len(), 3);
        assert_eq!(nal_type(&nals[0]), Some(7));
        assert_eq!(nal_type(&nals[1]), Some(8));
        assert_eq!(nal_type(&nals[2]), Some(5));
    }

    #[test]
    fn builds_avcc_length_prefixed_sample() {
        let nals = vec![vec![0x65, 0x01, 0x02], vec![0x41, 0x03]];
        let avcc = nal_units_to_avcc_sample(&nals);
        assert_eq!(
            avcc,
            vec![
                0, 0, 0, 3, 0x65, 0x01, 0x02, //
                0, 0, 0, 2, 0x41, 0x03
            ]
        );
    }
}
