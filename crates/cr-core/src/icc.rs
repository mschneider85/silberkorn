//! Minimal ICC v2 matrix/TRC display profile writer, used to tag exported images.

use crate::color::{Mat3, OutputSpace, Trc, srgb_decode};

const D50: [f32; 3] = [0.9642, 1.0, 0.8249];

fn s15f16(v: f32) -> [u8; 4] {
    ((v as f64 * 65536.0).round() as i32).to_be_bytes()
}

fn tag_xyz(xyz: [f32; 3]) -> Vec<u8> {
    let mut d = b"XYZ \0\0\0\0".to_vec();
    for v in xyz {
        d.extend_from_slice(&s15f16(v));
    }
    d
}

fn tag_curve(trc: Trc) -> Vec<u8> {
    let mut d = b"curv\0\0\0\0".to_vec();
    match trc {
        Trc::Gamma(g) => {
            d.extend_from_slice(&1u32.to_be_bytes());
            // u8Fixed8Number
            d.extend_from_slice(&((g * 256.0).round() as u16).to_be_bytes());
        }
        Trc::Srgb => {
            // Table maps encoded -> linear.
            const N: usize = 1024;
            d.extend_from_slice(&(N as u32).to_be_bytes());
            for i in 0..N {
                let v = srgb_decode(i as f32 / (N - 1) as f32);
                d.extend_from_slice(&((v * 65535.0).round() as u16).to_be_bytes());
            }
        }
    }
    d
}

fn tag_desc(text: &str) -> Vec<u8> {
    let mut d = b"desc\0\0\0\0".to_vec();
    let ascii: Vec<u8> = text.bytes().filter(|b| b.is_ascii()).chain(std::iter::once(0)).collect();
    d.extend_from_slice(&(ascii.len() as u32).to_be_bytes());
    d.extend_from_slice(&ascii);
    d.extend_from_slice(&[0; 4]); // unicode language code
    d.extend_from_slice(&[0; 4]); // unicode count
    d.extend_from_slice(&[0; 2]); // scriptcode code
    d.push(0); // scriptcode count
    d.extend_from_slice(&[0; 67]);
    d
}

fn tag_text(text: &str) -> Vec<u8> {
    let mut d = b"text\0\0\0\0".to_vec();
    d.extend(text.bytes().filter(|b| b.is_ascii()));
    d.push(0);
    d
}

/// Builds an RGB display profile from primaries (RGB -> XYZ D50) and a transfer curve.
pub fn rgb_profile(description: &str, rgb_to_xyz_d50: &Mat3, trc: Trc) -> Vec<u8> {
    let col = |c: usize| [rgb_to_xyz_d50[0][c], rgb_to_xyz_d50[1][c], rgb_to_xyz_d50[2][c]];
    let curve = tag_curve(trc);
    let tags: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"desc", tag_desc(description)),
        (b"cprt", tag_text("No copyright, use freely")),
        (b"wtpt", tag_xyz(D50)),
        (b"rXYZ", tag_xyz(col(0))),
        (b"gXYZ", tag_xyz(col(1))),
        (b"bXYZ", tag_xyz(col(2))),
        (b"rTRC", curve.clone()),
        (b"gTRC", curve.clone()),
        (b"bTRC", curve),
    ];

    let table_len = 4 + 12 * tags.len();
    let mut offset = 128 + table_len;
    let mut table = (tags.len() as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    for (sig, body) in &tags {
        table.extend_from_slice(*sig);
        table.extend_from_slice(&(offset as u32).to_be_bytes());
        table.extend_from_slice(&(body.len() as u32).to_be_bytes());
        data.extend_from_slice(body);
        offset += body.len();
        while data.len() % 4 != 0 {
            data.push(0);
            offset += 1;
        }
    }

    let total = 128 + table.len() + data.len();
    let mut h = Vec::with_capacity(total);
    h.extend_from_slice(&(total as u32).to_be_bytes());
    h.extend_from_slice(&[0; 4]); // preferred CMM
    h.extend_from_slice(&0x0240_0000u32.to_be_bytes()); // version 2.4
    h.extend_from_slice(b"mntr");
    h.extend_from_slice(b"RGB ");
    h.extend_from_slice(b"XYZ ");
    h.extend_from_slice(&[0x07, 0xea, 0, 1, 0, 1, 0, 0, 0, 0, 0, 0]); // 2026-01-01
    h.extend_from_slice(b"acsp");
    h.extend_from_slice(&[0; 4]); // platform
    h.extend_from_slice(&[0; 4]); // flags
    h.extend_from_slice(&[0; 4]); // manufacturer
    h.extend_from_slice(&[0; 4]); // model
    h.extend_from_slice(&[0; 8]); // attributes
    h.extend_from_slice(&0u32.to_be_bytes()); // perceptual intent
    for v in D50 {
        h.extend_from_slice(&s15f16(v));
    }
    h.extend_from_slice(&[0; 4]); // creator
    h.extend_from_slice(&[0; 16]); // profile id
    h.extend_from_slice(&[0; 28]); // reserved
    debug_assert_eq!(h.len(), 128);
    h.extend_from_slice(&table);
    h.extend_from_slice(&data);
    h
}

pub fn profile_for(space: OutputSpace) -> Vec<u8> {
    let name = match space {
        OutputSpace::Srgb => "sRGB (Capture Room)",
        OutputSpace::AdobeRgb => "Adobe RGB (1998) compatible (Capture Room)",
        OutputSpace::DisplayP3 => "Display P3 (Capture Room)",
        OutputSpace::ProPhoto => "ProPhoto RGB (Capture Room)",
    };
    rgb_profile(name, &space.to_xyz_d50(), space.trc())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn be32(d: &[u8], at: usize) -> u32 {
        u32::from_be_bytes(d[at..at + 4].try_into().unwrap())
    }

    #[test]
    fn profile_structure_is_valid() {
        for space in OutputSpace::ALL {
            let p = profile_for(space);
            assert_eq!(be32(&p, 0) as usize, p.len());
            assert_eq!(&p[36..40], b"acsp");
            let n = be32(&p, 128) as usize;
            assert_eq!(n, 9);
            for i in 0..n {
                let e = 132 + i * 12;
                let (off, size) = (be32(&p, e + 4) as usize, be32(&p, e + 8) as usize);
                assert_eq!(off % 4, 0);
                assert!(off + size <= p.len());
            }
            // Sum of colorants equals the D50 white point.
            let m = space.to_xyz_d50();
            for (row, white) in m.iter().zip(D50) {
                assert!((row.iter().sum::<f32>() - white).abs() < 2e-3, "{space:?}");
            }
        }
    }
}
