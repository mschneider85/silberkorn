//! Gives the Windows program its icon (Explorer, Start menu, taskbar): `assets/silberkorn.ico` as
//! a compiled resource file, written here and handed to the linker. No resource compiler is
//! needed, so it works the same on Windows and with cargo-xwin. (The window icon of the running
//! app is set in `main.rs`.)
//!
//! The icon itself is drawn in `assets/icon.svg`; `icon-256.png` and `silberkorn.ico` (16 to 256
//! pixels, the small ones without the grain) are rendered from it.

use std::path::PathBuf;

fn main() {
    let ico = "assets/silberkorn.ico";
    println!("cargo:rerun-if-changed={ico}");
    let windows_msvc = std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") && std::env::var("CARGO_CFG_TARGET_ENV").as_deref() == Ok("msvc");
    if !windows_msvc {
        return;
    }
    let data = std::fs::read(ico).expect("Icon fehlt");
    let res = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR")).join("silberkorn.res");
    std::fs::write(&res, resource_file(&data)).expect("Ressourcendatei schreiben");
    // link.exe and lld-link take a .res file like an object file.
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// A .res file with the images of an .ico file (one RT_ICON each) and their directory
/// (RT_GROUP_ICON 1, which Windows shows as the program's icon).
fn resource_file(ico: &[u8]) -> Vec<u8> {
    let u16_at = |i: usize| u16::from_le_bytes([ico[i], ico[i + 1]]);
    let u32_at = |i: usize| u32::from_le_bytes([ico[i], ico[i + 1], ico[i + 2], ico[i + 3]]);
    assert!(ico.len() >= 6 && u16_at(2) == 1, "keine .ico-Datei");
    let count = u16_at(4) as usize;
    // The empty resource that marks a 32-bit .res file.
    let mut out = Vec::new();
    push_resource(&mut out, 0, 0, &[]);
    let mut group = Vec::new();
    group.extend_from_slice(&[0, 0, 1, 0]);
    group.extend_from_slice(&(count as u16).to_le_bytes());
    for i in 0..count {
        let e = 6 + 16 * i;
        let (size, offset) = (u32_at(e + 8) as usize, u32_at(e + 12) as usize);
        let id = i as u16 + 1;
        push_resource(&mut out, RT_ICON, id, &ico[offset..offset + size]);
        // Width, height, colors, reserved; one plane; bits per pixel; size; resource id.
        group.extend_from_slice(&ico[e..e + 4]);
        group.extend_from_slice(&1u16.to_le_bytes());
        group.extend_from_slice(&ico[e + 6..e + 12]);
        group.extend_from_slice(&id.to_le_bytes());
    }
    push_resource(&mut out, RT_GROUP_ICON, 1, &group);
    out
}

/// One resource: its header (type and name as numbers, neutral language), the data, padding.
fn push_resource(out: &mut Vec<u8>, kind: u16, id: u16, data: &[u8]) {
    let mut header = Vec::with_capacity(32);
    header.extend_from_slice(&(data.len() as u32).to_le_bytes());
    header.extend_from_slice(&32u32.to_le_bytes());
    for v in [0xFFFF, kind, 0xFFFF, id] {
        header.extend_from_slice(&v.to_le_bytes());
    }
    // Data version, memory flags (moveable, pure, discardable for real resources), language,
    // version, characteristics.
    header.extend_from_slice(&0u32.to_le_bytes());
    header.extend_from_slice(&(if kind == 0 { 0u16 } else { 0x1030 }).to_le_bytes());
    header.extend_from_slice(&0u16.to_le_bytes());
    header.extend_from_slice(&[0; 8]);
    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    while out.len() % 4 != 0 {
        out.push(0);
    }
}
