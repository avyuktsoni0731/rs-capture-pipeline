use openh264::encoder::EncodedBitStream;

/// Concatenate NAL units with 4-byte Annex-B start codes (`00 00 00 01`).
pub fn encoded_bitstream_to_annex_b(bs: &EncodedBitStream<'_>) -> Vec<u8> {
    let mut out = Vec::new();
    for li in 0..bs.num_layers() {
        let Some(layer) = bs.layer(li) else {
            continue;
        };
        for ni in 0..layer.nal_count() {
            if let Some(nal) = layer.nal_unit(ni) {
                out.extend_from_slice(&[0, 0, 0, 1]);
                out.extend_from_slice(nal);
            }
        }
    }
    out
}
