//! A printed page as pixels: the image in its window on the paper, sharpened and adjusted for
//! print, with border, crop marks and caption; and the page as a PDF or JPEG file. The printer
//! itself is driven by the app (Windows).

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, anyhow, ensure};
use image::{ImageBuffer, Rgb};
use lcms2::{ColorSpaceSignature, DisallowCache, Flags, PixelFormat, Profile, ThreadContext, Transform};
use rayon::prelude::*;

use crate::color::OutputSpace;
use crate::image::LinearImage;
use crate::print::{CAPTION_MM, Layout, MARK_LINE_MM, PrintSettings};

/// Pages are rendered at most this fine (pixels per inch); the printer driver scales them to its
/// own resolution. Finer brings nothing visible on photo paper.
pub const MAX_DPI: f32 = 360.0;
/// Pages written to files.
pub const FILE_DPI: f32 = 300.0;

/// A page: 8-bit RGB, rows top to bottom.
pub struct Page {
    pub width: usize,
    pub height: usize,
    pub rgb: Vec<u8>,
    /// The paper (mm).
    pub paper: [f32; 2],
}

/// The conversion into the printer's colors (its ICC profile for printer and paper).
pub struct PrinterProfile {
    // Dropped before the context it was created in.
    transform: Transform<[u16; 3], [u8; 3], ThreadContext, DisallowCache>,
    _context: KeepAlive,
    /// The profile, for files printed with it.
    pub icc: Vec<u8>,
}

/// Keeps the Little CMS context alive for the transform made in it; it is not used afterwards,
/// so sharing it between threads is fine (the transform itself is `Sync` without a cache).
struct KeepAlive(#[allow(dead_code)] ThreadContext);

unsafe impl Sync for KeepAlive {}

/// The color space images are developed into before the printer profile takes them: wide enough
/// for every printer.
pub const PROFILE_SOURCE: OutputSpace = OutputSpace::ProPhoto;

impl PrinterProfile {
    pub fn new(path: &Path, intent: crate::softproof::RenderIntent) -> Result<Self> {
        let ctx = ThreadContext::new();
        let icc = std::fs::read(path).with_context(|| format!("{} nicht lesbar", path.display()))?;
        let printer = Profile::new_icc_context(&ctx, &icc).map_err(|e| anyhow!("{}: kein gültiges ICC-Profil ({e})", path.display()))?;
        // Printer drivers take RGB; CMYK profiles are for print shops (a RIP).
        ensure!(printer.color_space() == ColorSpaceSignature::RgbData, "{}: kein RGB-Profil (Druckertreiber nehmen nur RGB)", path.display());
        let source = Profile::new_icc_context(&ctx, &crate::icc::profile_for(PROFILE_SOURCE)).map_err(|e| anyhow!("{e}"))?;
        let transform = Transform::new_flags_context(
            &ctx,
            &source,
            PixelFormat::RGB_16,
            &printer,
            PixelFormat::RGB_8,
            intent.lcms(),
            Flags::NO_CACHE | Flags::BLACKPOINT_COMPENSATION,
        )
        .map_err(|e| anyhow!("Farbumrechnung ins Druckerprofil nicht möglich: {e}"))?;
        Ok(Self { transform, _context: KeepAlive(ctx), icc })
    }

    fn apply(&self, src: &[f32], dst: &mut [u8]) {
        let input: Vec<[u16; 3]> = src.as_chunks::<3>().0.iter().map(|p| p.map(|v| (v.clamp(0.0, 1.0) * 65535.0 + 0.5) as u16)).collect();
        let out: &mut [[u8; 3]] = bytemuck::cast_slice_mut(dst);
        self.transform.transform_pixels(&input, out);
    }
}

/// Brightness and contrast of the print adjustment (-50..50) on an encoded value.
fn adjust(v: f32, brightness: f32, contrast: f32) -> f32 {
    let mut v = v.clamp(0.0, 1.0);
    if brightness != 0.0 {
        v = v.powf(2f32.powf(-brightness / 100.0));
    }
    if contrast != 0.0 {
        let s = v * v * (3.0 - 2.0 * v);
        v += (contrast / 50.0) * (s - v);
    }
    v.clamp(0.0, 1.0)
}

/// The page for `processed` (developed, encoded 0..1: in sRGB, or in [`PROFILE_SOURCE`] with a
/// profile) laid out by `layout`, at `dpi`.
pub fn render(processed: &LinearImage, layout: &Layout, dpi: f32, s: &PrintSettings, caption: Option<&str>, profile: Option<&PrinterProfile>) -> Result<Page> {
    let k = dpi / 25.4;
    let px = |mm: f32| (mm * k).round().max(0.0) as i64;
    let (pw, ph) = (px(layout.paper[0]).max(1) as usize, px(layout.paper[1]).max(1) as usize);
    ensure!(pw * ph <= 400_000_000, "Seite zu groß ({pw}×{ph} Pixel)");
    let mut rgb = vec![255u8; pw * ph * 3];

    // The printed part of the image, cut from the developed one, at the page's resolution.
    let [wx, wy, ww, wh] = layout.window_rect();
    let (x0, y0) = (px(wx), px(wy));
    let (tw, th) = (px(wx + ww) - x0, px(wy + wh) - y0);
    ensure!(tw > 0 && th > 0, "Bild zu klein für die Seite");
    let [cx, cy, cw, ch] = layout.window_in_image();
    let (sw, sh) = (processed.width as f32, processed.height as f32);
    let (sx0, sy0) = ((cx * sw).round() as usize, (cy * sh).round() as usize);
    let (sx1, sy1) = (((cx + cw) * sw).round().min(sw) as usize, ((cy + ch) * sh).round().min(sh) as usize);
    ensure!(sx1 > sx0 && sy1 > sy0, "Bild zu klein für die Seite");
    let crop: Vec<f32> = (sy0..sy1).flat_map(|y| processed.data[y * processed.width + sx0..y * processed.width + sx1].iter().flatten().copied()).collect();
    let crop: ImageBuffer<Rgb<f32>, Vec<f32>> = ImageBuffer::from_raw((sx1 - sx0) as u32, (sy1 - sy0) as u32, crop).context("Puffergröße ungültig")?;
    let mut buf = crate::export::resize_lanczos3(&crop, tw as u32, th as u32);
    if let Some((amount, radius_mm)) = s.sharpening.strength(s.paper) {
        crate::export::sharpen(&mut buf, amount, (radius_mm * k).max(0.5));
    }
    let (b, c) = (finite(s.brightness).clamp(-50.0, 50.0), finite(s.contrast).clamp(-50.0, 50.0));
    if b != 0.0 || c != 0.0 {
        buf.par_iter_mut().for_each(|v| *v = adjust(*v, b, c));
    }
    let mut image = vec![0u8; buf.len()];
    match profile {
        Some(p) => image.par_chunks_mut(3 * 4096).zip(buf.as_raw().par_chunks(3 * 4096)).for_each(|(d, s)| p.apply(s, d)),
        None => image.par_iter_mut().zip(buf.as_raw().par_iter()).for_each(|(d, v)| *d = (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8),
    }
    let (tw, th) = (tw as usize, th as usize);
    for y in 0..th {
        let py = y0 + y as i64;
        if py < 0 || py >= ph as i64 {
            continue;
        }
        for x in 0..tw {
            let pxx = x0 + x as i64;
            if pxx < 0 || pxx >= pw as i64 {
                continue;
            }
            let (d, s) = ((py as usize * pw + pxx as usize) * 3, (y * tw + x) * 3);
            rgb[d..d + 3].copy_from_slice(&image[s..s + 3]);
        }
    }

    let mut fill = |x0: i64, y0: i64, x1: i64, y1: i64, v: u8| {
        for y in y0.max(0)..y1.min(ph as i64) {
            for x in x0.max(0)..x1.min(pw as i64) {
                let i = (y as usize * pw + x as usize) * 3;
                rgb[i..i + 3].fill(v);
            }
        }
    };
    // The border, around the image.
    let border = finite(s.border_mm).clamp(0.0, 20.0);
    if border > 0.0 {
        let b = px(border).max(1);
        let (x1, y1) = (x0 + tw as i64, y0 + th as i64);
        fill(x0 - b, y0 - b, x1 + b, y0, 0);
        fill(x0 - b, y1, x1 + b, y1 + b, 0);
        fill(x0 - b, y0, x0, y1, 0);
        fill(x1, y0, x1 + b, y1, 0);
    }
    if s.crop_marks {
        let line = px(MARK_LINE_MM).max(1);
        for ([ax, ay], [bx, by]) in layout.crop_marks(border) {
            let (lx, ly, hx, hy) = (px(ax.min(bx)), px(ay.min(by)), px(ax.max(bx)), px(ay.max(by)));
            if ly == hy {
                fill(lx, ly - line / 2, hx, ly - line / 2 + line, 0);
            } else {
                fill(lx - line / 2, ly, lx - line / 2 + line, hy, 0);
            }
        }
    }
    if let Some(text) = caption
        && let Some(o) = crate::watermark::text_overlay(text, CAPTION_MM * k * 1.35, pw as f32 * 0.9, 0.3)
    {
        let (ox, oy) = ((pw as i64 - o.width as i64) / 2, px(layout.caption_top()) - (CAPTION_MM * k * 0.1) as i64);
        for y in 0..o.height {
            for x in 0..o.width {
                let (gx, gy) = (ox + x as i64, oy + y as i64);
                if gx < 0 || gy < 0 || gx >= pw as i64 || gy >= ph as i64 {
                    continue;
                }
                let (color, a) = o.pixels[y * o.width + x];
                let i = (gy as usize * pw + gx as usize) * 3;
                for (v, c) in rgb[i..i + 3].iter_mut().zip(color) {
                    *v = (*v as f32 * (1.0 - a) + c * 255.0 * a).round() as u8;
                }
            }
        }
    }
    Ok(Page { width: pw, height: ph, rgb, paper: layout.paper })
}

impl Page {
    /// The page turned a quarter clockwise, paper and all: for a printer whose sheet lies the
    /// other way than the page.
    pub fn turned(&self) -> Page {
        let (w, h) = (self.width, self.height);
        let mut rgb = vec![0u8; self.rgb.len()];
        rgb.par_chunks_mut(h * 3).enumerate().for_each(|(y, row)| {
            // The new row y is the old column y, read from the bottom up.
            for (x, px) in row.chunks_exact_mut(3).enumerate() {
                let i = ((h - 1 - x) * w + y) * 3;
                px.copy_from_slice(&self.rgb[i..i + 3]);
            }
        });
        Page { width: h, height: w, rgb, paper: [self.paper[1], self.paper[0]] }
    }
}

fn finite(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

/// The ICC profile of a page's colors: the printer's, or sRGB.
pub fn page_icc(profile: Option<&PrinterProfile>) -> Vec<u8> {
    profile.map_or_else(|| crate::icc::profile_for(OutputSpace::Srgb), |p| p.icc.clone())
}

/// The page as a JPEG with its resolution and profile.
pub fn jpeg(page: &Page, icc: &[u8]) -> Result<Vec<u8>> {
    let mut data = crate::export::encode_jpeg(&page.rgb, page.width as u32, page.height as u32, 95, Some(icc), &[])?;
    let dpi = (page.width as f32 / (page.paper[0] / 25.4)).round() as u16;
    set_jpeg_dpi(&mut data, dpi);
    Ok(data)
}

/// Writes the resolution into a JPEG's JFIF header (if it has one).
fn set_jpeg_dpi(jpeg: &mut [u8], dpi: u16) {
    if let Some(i) = jpeg.iter().take(64).collect::<Vec<_>>().windows(5).position(|w| w.iter().map(|b| **b).eq(*b"JFIF\0")) {
        // JFIF\0, version (2), units (1), x and y density (2 each).
        let u = i + 7;
        if jpeg.len() > u + 5 {
            jpeg[u] = 1;
            jpeg[u + 1..u + 3].copy_from_slice(&dpi.to_be_bytes());
            jpeg[u + 3..u + 5].copy_from_slice(&dpi.to_be_bytes());
        }
    }
}

/// The pages (as JPEGs, see [`jpeg`]) in one PDF, each on its paper size.
pub fn pdf(pages: &[(Vec<u8>, usize, usize, [f32; 2])], icc: &[u8]) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = b"%PDF-1.4\n%\xe2\xe3\xcf\xd3\n".to_vec();
    let mut offsets: Vec<usize> = Vec::new();
    let n = pages.len();
    // 1 catalog, 2 pages, 3 profile, then page, content and image for each page.
    let page_id = |i: usize| 4 + 3 * i;
    let object = |out: &mut Vec<u8>, offsets: &mut Vec<usize>, head: String, stream: Option<&[u8]>| -> Result<()> {
        offsets.push(out.len());
        write!(out, "{} 0 obj\n{head}", offsets.len())?;
        if let Some(data) = stream {
            out.extend_from_slice(b"\nstream\n");
            out.extend_from_slice(data);
            out.extend_from_slice(b"\nendstream");
        }
        out.extend_from_slice(b"\nendobj\n");
        Ok(())
    };
    object(&mut out, &mut offsets, "<< /Type /Catalog /Pages 2 0 R >>".into(), None)?;
    let kids: Vec<String> = (0..n).map(|i| format!("{} 0 R", page_id(i))).collect();
    object(&mut out, &mut offsets, format!("<< /Type /Pages /Kids [{}] /Count {n} >>", kids.join(" ")), None)?;
    let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(icc)?;
    let icc_z = z.finish()?;
    object(&mut out, &mut offsets, format!("<< /N 3 /Filter /FlateDecode /Length {} >>", icc_z.len()), Some(&icc_z))?;
    for (i, (jpeg, w, h, paper)) in pages.iter().enumerate() {
        let (pw, ph) = (paper[0] / 25.4 * 72.0, paper[1] / 25.4 * 72.0);
        let id = page_id(i);
        object(
            &mut out,
            &mut offsets,
            format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2} {ph:.2}] /Resources << /XObject << /Im0 {} 0 R >> >> /Contents {} 0 R >>", id + 2, id + 1),
            None,
        )?;
        let content = format!("q {pw:.2} 0 0 {ph:.2} 0 0 cm /Im0 Do Q");
        object(&mut out, &mut offsets, format!("<< /Length {} >>", content.len()), Some(content.as_bytes()))?;
        object(
            &mut out,
            &mut offsets,
            format!("<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace [/ICCBased 3 0 R] /BitsPerComponent 8 /Filter /DCTDecode /Length {} >>", jpeg.len()),
            Some(jpeg),
        )?;
    }
    let xref = out.len();
    writeln!(out, "xref\n0 {}\n0000000000 65535 f ", offsets.len() + 1)?;
    for o in &offsets {
        writeln!(out, "{o:010} 00000 n ")?;
    }
    writeln!(out, "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF", offsets.len() + 1)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::print::{Caption, Fit, PrintFormat};

    fn gray(w: usize, h: usize, v: f32) -> LinearImage {
        LinearImage::new(w, h, vec![[v; 3]; w * h])
    }

    #[test]
    fn the_page_has_the_image_in_its_window_and_marks_outside() {
        let s = PrintSettings {
            format: PrintFormat::new(100.0, 150.0),
            fit: Fit::Contain,
            margin_mm: 15.0,
            border_mm: 0.5,
            crop_marks: true,
            caption: Caption::FileName,
            sharpening: crate::print::PrintSharpening::Off,
            ..Default::default()
        };
        let img = gray(600, 400, 0.5);
        let l = crate::print::layout(600.0, 400.0, &s);
        let page = render(&img, &l, 100.0, &s, Some("DSC09197.ARW"), None).unwrap();
        let k: f32 = 100.0 / 25.4;
        assert_eq!((page.width, page.height), ((150.0 * k).round() as usize, (100.0 * k).round() as usize));
        let at = |x: f32, y: f32| page.rgb[((y * k).round() as usize * page.width + (x * k).round() as usize) * 3];
        let [x, y, w, h] = l.window_rect();
        assert_eq!(at(x + w / 2.0, y + h / 2.0), 128, "Bildmitte");
        assert_eq!(at(2.0, 2.0), 255, "Papier");
        assert_eq!(at(x - 0.25, y + h / 2.0), 0, "Rahmen");
        // A crop mark beside the top left corner, on the line of the image's top edge.
        assert_eq!(at(x - 0.5 - crate::print::MARK_GAP_MM - 3.0, y), 0, "Schnittmarke");
        // Some dark pixels of the caption below the image.
        let cap = (l.caption_top() * k) as usize;
        let dark = (cap..cap + (CAPTION_MM * k) as usize).flat_map(|py| (0..page.width).map(move |px| (px, py))).filter(|&(px, py)| page.rgb[(py * page.width + px) * 3] < 128).count();
        assert!(dark > 20, "Beschriftung: {dark}");
    }

    #[test]
    fn turning_moves_the_top_left_to_the_top_right() {
        // 3 × 2: a marked pixel at the top left.
        let mut rgb = vec![255u8; 3 * 2 * 3];
        rgb[..3].copy_from_slice(&[0, 0, 0]);
        let page = Page { width: 3, height: 2, rgb, paper: [30.0, 20.0] };
        let t = page.turned();
        assert_eq!((t.width, t.height, t.paper), (2, 3, [20.0, 30.0]));
        assert_eq!(&t.rgb[3..6], &[0, 0, 0], "oben rechts");
        assert_eq!(t.rgb.iter().filter(|&&v| v == 0).count(), 3);
    }

    #[test]
    fn brightness_lifts_and_contrast_spreads() {
        assert!(adjust(0.5, 30.0, 0.0) > 0.55);
        assert!(adjust(0.5, -30.0, 0.0) < 0.45);
        assert!(adjust(0.25, 0.0, 40.0) < 0.25 && adjust(0.75, 0.0, 40.0) > 0.75);
        assert_eq!(adjust(0.0, 30.0, 30.0), 0.0);
        assert_eq!(adjust(1.0, 30.0, 30.0), 1.0);
    }

    #[test]
    fn pdf_and_jpeg_are_well_formed() {
        let s = PrintSettings { format: PrintFormat::new(100.0, 150.0), ..Default::default() };
        let img = gray(300, 200, 0.3);
        let l = crate::print::layout(300.0, 200.0, &s);
        let page = render(&img, &l, 50.0, &s, None, None).unwrap();
        let icc = page_icc(None);
        let jpeg = jpeg(&page, &icc).unwrap();
        let decoded = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((decoded.width() as usize, decoded.height() as usize), (page.width, page.height));
        let i = jpeg.windows(5).position(|w| w == b"JFIF\0").unwrap();
        assert_eq!(&jpeg[i + 7..i + 10], &[1, 0, 50]);
        let pdf = pdf(&[(jpeg.clone(), page.width, page.height, page.paper), (jpeg, page.width, page.height, page.paper)], &icc).unwrap();
        let text = String::from_utf8_lossy(&pdf);
        assert!(text.starts_with("%PDF-1.4") && text.ends_with("%%EOF\n"));
        assert!(text.contains("/Count 2") && text.contains("/MediaBox [0 0 425.20 283.46]"));
        // The cross-reference table points at the objects (byte offsets).
        let tail = String::from_utf8_lossy(&pdf[pdf.len() - 40..]).to_string();
        let start: usize = tail.rsplit("startxref\n").next().unwrap().lines().next().unwrap().parse().unwrap();
        assert!(pdf[start..].starts_with(b"xref"));
        let table = String::from_utf8_lossy(&pdf[start..start + 80]).to_string();
        let first: usize = table.lines().nth(3).unwrap()[..10].parse().unwrap();
        assert!(pdf[first..].starts_with(b"1 0 obj"));
    }
}
