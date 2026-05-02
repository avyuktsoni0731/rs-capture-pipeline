use anyhow::ensure;

/// Pack separate full-res **Y** and half-res **UV** (interleaved Cb,Cr rows from our GPU readback)
/// into a single **I420** buffer for OpenH264 (`Y`, then `U`, then `V` planes).
pub fn nv12_readback_to_i420(
    y: &[u8],
    uv: &[u8],
    width: u32,
    height: u32,
) -> anyhow::Result<Vec<u8>> {
    let w = width as usize;
    let h = height as usize;
    let uw = ((width + 1) / 2) as usize;
    let uh = ((height + 1) / 2) as usize;
    let cw = w / 2;
    let ch = h / 2;

    ensure!(w % 2 == 0 && h % 2 == 0, "I420 path expects even width/height");
    ensure!(y.len() >= w * h, "Y plane too small");
    ensure!(uv.len() >= uw * uh * 2, "UV plane too small");

    let mut out = vec![0u8; w * h + 2 * cw * ch];
    out[..w * h].copy_from_slice(&y[..w * h]);

    let u_base = w * h;
    let v_base = u_base + cw * ch;

    for row in 0..ch {
        for col in 0..cw {
            let o = (row * uw + col) * 2;
            let cb = uv[o];
            let cr = uv[o + 1];
            out[u_base + row * cw + col] = cb;
            out[v_base + row * cw + col] = cr;
        }
    }

    Ok(out)
}
