//! Encoders for the web formats WebP (lossy, via libwebp) and AVIF (10 bit, via rav1e).

use anyhow::{Context, Result, bail};

use crate::color::OutputSpace;

/// Lossy WebP with color profile and EXIF. `rgb`: 8-bit RGB, row-major.
pub fn encode_webp(rgb: &[u8], width: u32, height: u32, quality: u8, icc: &[u8], exif: &[u8]) -> Result<Vec<u8>> {
    if width > 16383 || height > 16383 {
        bail!("WebP erlaubt höchstens 16383 Pixel Kantenlänge (Bild: {width} × {height}); bitte im Rezept skalieren");
    }
    if rgb.len() != width as usize * height as usize * 3 {
        bail!("Puffergröße ungültig");
    }
    let bitstream = unsafe {
        let mut out: *mut u8 = std::ptr::null_mut();
        let len = libwebp_sys::WebPEncodeRGB(rgb.as_ptr(), width as i32, height as i32, width as i32 * 3, quality.clamp(1, 100) as f32, &mut out);
        if len == 0 || out.is_null() {
            bail!("WebP-Kodierung fehlgeschlagen");
        }
        let bytes = std::slice::from_raw_parts(out, len).to_vec();
        libwebp_sys::WebPFree(out as *mut _);
        bytes
    };
    webp_with_metadata(&bitstream, width, height, icc, exif)
}

/// Rewraps a simple WebP file ("RIFF…WEBPVP8 …") into the extended format with ICCP and EXIF chunks.
fn webp_with_metadata(simple: &[u8], width: u32, height: u32, icc: &[u8], exif: &[u8]) -> Result<Vec<u8>> {
    if simple.len() < 12 || &simple[0..4] != b"RIFF" || &simple[8..12] != b"WEBP" {
        bail!("libwebp lieferte keine WebP-Datei");
    }
    let image_chunks = &simple[12..];
    let chunk = |fourcc: &[u8; 4], data: &[u8]| {
        let mut c = Vec::with_capacity(data.len() + 9);
        c.extend_from_slice(fourcc);
        c.extend_from_slice(&(data.len() as u32).to_le_bytes());
        c.extend_from_slice(data);
        if data.len() % 2 == 1 {
            c.push(0);
        }
        c
    };
    // VP8X: flags (ICC 0x20, EXIF 0x08), reserved, canvas size minus one in 24 bits each.
    let mut flags = 0u8;
    if !icc.is_empty() {
        flags |= 0x20;
    }
    if !exif.is_empty() {
        flags |= 0x08;
    }
    let mut vp8x = vec![flags, 0, 0, 0];
    vp8x.extend_from_slice(&(width - 1).to_le_bytes()[..3]);
    vp8x.extend_from_slice(&(height - 1).to_le_bytes()[..3]);

    let mut body = b"WEBP".to_vec();
    body.extend(chunk(b"VP8X", &vp8x));
    if !icc.is_empty() {
        body.extend(chunk(b"ICCP", icc));
    }
    body.extend_from_slice(image_chunks);
    if !exif.is_empty() {
        body.extend(chunk(b"EXIF", exif));
    }
    let mut file = b"RIFF".to_vec();
    file.extend_from_slice(&(body.len() as u32).to_le_bytes());
    file.extend(body);
    Ok(file)
}

/// Whether AVIF can describe `space` without an ICC profile (AVIF here is tagged with CICP only).
pub fn avif_supports(space: OutputSpace) -> bool {
    matches!(space, OutputSpace::Srgb | OutputSpace::DisplayP3)
}

/// AVIF quantizer (0 best .. 255 worst) for a JPEG-like quality 1..100.
fn avif_quantizer(quality: u8) -> u8 {
    let loss = 1.0 - quality.clamp(1, 100) as f32 / 100.0;
    (255.0 * loss.powf(0.62)).round() as u8
}

/// 10-bit AVIF (4:4:4, full range, BT.709 YCbCr) with EXIF. `rgb`: encoded in `space`, 0..1, row-major.
pub fn encode_avif(rgb: &[f32], width: u32, height: u32, quality: u8, space: OutputSpace, exif: &[u8]) -> Result<Vec<u8>> {
    use rav1e::prelude::*;
    if !avif_supports(space) {
        bail!("AVIF unterstützt hier nur sRGB und Display P3, nicht {}", space.label());
    }
    let (w, h) = (width as usize, height as usize);
    if rgb.len() != w * h * 3 {
        bail!("Puffergröße ungültig");
    }
    let primaries = match space {
        OutputSpace::DisplayP3 => (ColorPrimaries::SMPTE432, avif_serialize::constants::ColorPrimaries::DisplayP3),
        _ => (ColorPrimaries::BT709, avif_serialize::constants::ColorPrimaries::Bt709),
    };
    let quantizer = avif_quantizer(quality);
    let threads = rayon::current_num_threads();
    let config = Config::new()
        .with_encoder_config(EncoderConfig {
            width: w,
            height: h,
            bit_depth: 10,
            chroma_sampling: ChromaSampling::Cs444,
            pixel_range: PixelRange::Full,
            color_description: Some(ColorDescription {
                color_primaries: primaries.0,
                transfer_characteristics: TransferCharacteristics::SRGB,
                matrix_coefficients: MatrixCoefficients::BT709,
            }),
            still_picture: true,
            min_key_frame_interval: 0,
            max_key_frame_interval: 0,
            low_latency: false,
            quantizer: quantizer as usize,
            min_quantizer: quantizer,
            tune: Tune::Psychovisual,
            // Tiles let the threads work in parallel; each tile should stay reasonably large.
            tiles: threads.min(w * h / (128 * 128)).max(1),
            speed_settings: SpeedSettings::from_preset(10),
            ..Default::default()
        })
        .with_threads(threads);
    let mut ctx: Context<u16> = config.new_context().map_err(|e| anyhow::anyhow!("AV1-Encoder: {e:?}"))?;
    let mut frame = ctx.new_frame();
    {
        // BT.709 luma weights; the Y'CbCr is computed from the encoded values, as the standard says.
        const KR: f32 = 0.2126;
        const KB: f32 = 0.0722;
        const KG: f32 = 1.0 - KR - KB;
        let mut planes = frame.planes.iter_mut();
        let mut py = planes.next().context("Y-Ebene")?.mut_slice(Default::default());
        let mut pu = planes.next().context("U-Ebene")?.mut_slice(Default::default());
        let mut pv = planes.next().context("V-Ebene")?.mut_slice(Default::default());
        let rows = py.rows_iter_mut().zip(pu.rows_iter_mut()).zip(pv.rows_iter_mut()).take(h);
        for (row, ((y, u), v)) in rgb.chunks_exact(w * 3).zip(rows) {
            for (x, c) in row.chunks_exact(3).enumerate() {
                let [r, g, b] = [c[0], c[1], c[2]].map(|v| v.clamp(0.0, 1.0));
                let luma = KR * r + KG * g + KB * b;
                let cb = (b - luma) / (2.0 * (1.0 - KB));
                let cr = (r - luma) / (2.0 * (1.0 - KR));
                let q = |v: f32| (v * 1023.0).round().clamp(0.0, 1023.0) as u16;
                y[x] = q(luma);
                u[x] = q(cb + 0.5);
                v[x] = q(cr + 0.5);
            }
        }
    }
    ctx.send_frame(frame).map_err(|e| anyhow::anyhow!("AV1-Encoder: {e:?}"))?;
    ctx.flush();
    let mut av1 = Vec::new();
    loop {
        match ctx.receive_packet() {
            Ok(mut packet) => av1.append(&mut packet.data),
            Err(EncoderStatus::Encoded) => continue,
            Err(EncoderStatus::LimitReached) => break,
            Err(e) => bail!("AV1-Encoder: {e:?}"),
        }
    }
    let mut avif = avif_serialize::Aviffy::new();
    avif.set_color_primaries(primaries.1)
        .set_transfer_characteristics(avif_serialize::constants::TransferCharacteristics::Srgb)
        .set_matrix_coefficients(avif_serialize::constants::MatrixCoefficients::Bt709)
        .set_full_color_range(true);
    if !exif.is_empty() {
        avif.set_exif(exif.to_vec());
    }
    Ok(avif.to_vec(&av1, None, width, height, 10))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: u32, h: u32) -> Vec<f32> {
        (0..h).flat_map(|y| (0..w).flat_map(move |x| [x as f32 / w as f32, y as f32 / h as f32, 0.3])).collect()
    }

    #[test]
    fn webp_carries_profile_and_exif_and_decodes() {
        let (w, h) = (64, 48);
        let rgb: Vec<u8> = gradient(w, h).iter().map(|v| (v * 255.0).round() as u8).collect();
        let icc = crate::icc::profile_for(OutputSpace::DisplayP3);
        let file = encode_webp(&rgb, w, h, 90, &icc, b"II*\0test").unwrap();
        let riff_len = u32::from_le_bytes(file[4..8].try_into().unwrap()) as usize;
        assert_eq!(riff_len + 8, file.len());
        assert_eq!(&file[12..16], b"VP8X");
        let has = |needle: &[u8]| file.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"ICCP") && has(b"EXIF") && has(b"II*\0test"));
        let img = image::load_from_memory(&file).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (w, h));
        let max_diff = img.as_raw().iter().zip(&rgb).map(|(a, b)| (*a as i32 - *b as i32).abs()).max().unwrap();
        assert!(max_diff < 24, "WebP weicht zu stark ab: {max_diff}");
    }

    #[test]
    fn avif_is_tagged_with_the_color_space() {
        let (w, h) = (48, 32);
        let file = encode_avif(&gradient(w, h), w, h, 80, OutputSpace::DisplayP3, b"II*\0test").unwrap();
        assert_eq!(&file[4..12], b"ftypavif");
        let has = |needle: &[u8]| file.windows(needle.len()).any(|w| w == needle);
        // colr box "nclx" with primaries 12 (P3), transfer 13 (sRGB), matrix 1 (BT.709), full range.
        assert!(has(&[b'n', b'c', b'l', b'x', 0, 12, 0, 13, 0, 1, 0x80]));
        assert!(has(b"Exif") && has(b"II*\0test"));
        assert!(encode_avif(&gradient(w, h), w, h, 80, OutputSpace::ProPhoto, &[]).is_err());
        assert!(avif_quantizer(100) == 0 && avif_quantizer(90) < avif_quantizer(50));
    }
}
