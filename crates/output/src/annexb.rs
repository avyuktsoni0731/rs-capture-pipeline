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
