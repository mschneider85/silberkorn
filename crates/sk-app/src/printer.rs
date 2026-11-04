//! The printer (Windows): the pages as bitmaps to the printer chosen in the print bar, through
//! GDI. The pages come ready from `sk_core::print_page`; the printer gets them at the size of
//! its sheet and scales them to its own resolution.
//!
//! The printer and its settings ([`Choice`]) are put together on the UI thread, where the
//! driver's own settings dialog belongs to the window; the print job then opens the printer on
//! its own thread, renders and sends the pages there. (The system's print dialog isn't used:
//! Windows 11's ignores the printer it is given to start with.)

#[cfg(not(windows))]
use anyhow::Result;

/// A paper a printer offers, as its driver names it.
#[derive(Clone, Debug, PartialEq)]
pub struct PaperSize {
    pub name: String,
    /// The driver's number for it (`DMPAPER_*` or its own).
    pub code: i16,
    /// Short and long side (mm).
    pub mm: [f32; 2],
}

impl PaperSize {
    pub fn format(&self) -> sk_core::print::PrintFormat {
        sk_core::print::PrintFormat::new(self.mm[0], self.mm[1])
    }
}

/// The installed printers with their papers.
#[derive(Clone, Debug, Default)]
pub struct Printers {
    pub list: Vec<(String, Vec<PaperSize>)>,
    pub default: Option<String>,
}

impl Printers {
    /// The papers of the printer a setting means (by name; `None`: the default printer), or
    /// `None` if it isn't installed.
    pub fn papers(&self, name: Option<&str>) -> Option<&[PaperSize]> {
        let name = name.or(self.default.as_deref())?;
        self.list.iter().find(|(n, _)| n == name).map(|(_, p)| p.as_slice())
    }
}

/// The paper of `format`'s size among a printer's (either way round, within 1.5 mm).
pub fn paper_of(papers: &[PaperSize], format: sk_core::print::PrintFormat) -> Option<&PaperSize> {
    let (short, long) = (format.w_mm.min(format.h_mm), format.w_mm.max(format.h_mm));
    papers.iter().find(|p| (p.mm[0] - short).abs() < 1.5 && (p.mm[1] - long).abs() < 1.5)
}

/// The papers as the driver lists them (codes, names, sizes in 0.1 mm): without empty sizes
/// and repeated names.
#[cfg_attr(not(windows), allow(dead_code))]
fn paper_list(codes: &[u16], names: &[String], sizes: &[[i32; 2]]) -> Vec<PaperSize> {
    let mut out: Vec<PaperSize> = Vec::new();
    for ((&code, name), size) in codes.iter().zip(names).zip(sizes) {
        if size[0] <= 0 || size[1] <= 0 || name.trim().is_empty() || out.iter().any(|p| p.name == *name) {
            continue;
        }
        let (a, b) = (size[0] as f32 / 10.0, size[1] as f32 / 10.0);
        out.push(PaperSize { name: name.trim().to_string(), code: code as i16, mm: [a.min(b), a.max(b)] });
    }
    out
}

/// What the job asks of the printer before its dialog shows.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Request {
    /// The printer; `None`: the default printer.
    pub printer: Option<String>,
    /// The paper (mm), turned like the first page.
    pub paper_mm: [f32; 2],
    /// Silberkorn converts the colors with the printer's profile: the driver must not again.
    pub own_colors: bool,
}

/// The printer and its settings: plain data, for the print job's thread.
#[derive(Clone)]
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Choice {
    /// The printer's name, NUL-terminated (UTF-16).
    device: Vec<u16>,
    /// Its settings (a DEVMODEW with the driver's part), aligned.
    devmode: Vec<u64>,
}

/// The resolution the pages are rendered at for a printer of `device_dpi`: a whole fraction of
/// it (the driver scales by a whole factor, without resampling patterns), at most
/// [`sk_core::print_page::MAX_DPI`]: 300 for 600 and 1200 dpi, 360 for 720 and 1440 dpi.
pub fn page_dpi(device_dpi: f32) -> f32 {
    let max = sk_core::print_page::MAX_DPI;
    if device_dpi <= max {
        return device_dpi.max(1.0);
    }
    device_dpi / (device_dpi / max).ceil()
}

/// Rows of a page sent to the printer at a time: drivers handle bands better than a whole
/// sheet in one piece (an A2 page at 360 ppi is 150 MB).
#[cfg_attr(not(windows), allow(dead_code))]
const BAND_ROWS: usize = 512;

/// Where each band of `rows` page rows lands on a sheet of `device_rows`: start and height in
/// device rows, without gaps or overlaps between the bands.
#[cfg_attr(not(windows), allow(dead_code))]
fn bands(rows: usize, device_rows: i32) -> Vec<(usize, usize, i32, i32)> {
    let at = |y: usize| (y as f64 * device_rows as f64 / rows.max(1) as f64).round() as i32;
    (0..rows)
        .step_by(BAND_ROWS)
        .map(|y0| {
            let y1 = (y0 + BAND_ROWS).min(rows);
            (y0, y1, at(y0), at(y1) - at(y0))
        })
        .collect()
}

impl Choice {
    /// The printer's name.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn name(&self) -> String {
        String::from_utf16_lossy(self.device.strip_suffix(&[0]).unwrap_or(&self.device))
    }
}

#[cfg(windows)]
pub use win::{Printer, prepared, printers, properties};

#[cfg(not(windows))]
pub struct Printer;

/// No printers outside Windows.
#[cfg(not(windows))]
pub fn printers() -> Printers {
    Printers::default()
}

#[cfg(not(windows))]
pub fn prepared(_req: &Request, _base: Option<&Choice>) -> Result<Choice> {
    anyhow::bail!("Drucken gibt es nur unter Windows")
}

#[cfg(not(windows))]
pub fn properties(_owner: isize, _choice: &mut Choice) -> Result<bool> {
    anyhow::bail!("Drucken gibt es nur unter Windows")
}

#[cfg(not(windows))]
impl Printer {
    pub fn open(_choice: &Choice, _output: Option<&std::path::Path>) -> Result<Self> {
        anyhow::bail!("Drucken gibt es nur unter Windows")
    }

    pub fn paper_mm(&self) -> [f32; 2] {
        [0.0, 0.0]
    }

    pub fn dpi(&self) -> f32 {
        0.0
    }

    pub fn start(&mut self, _title: &str) -> Result<()> {
        Ok(())
    }

    pub fn page(&mut self, _page: &sk_core::print_page::Page) -> Result<()> {
        Ok(())
    }

    pub fn finish(&mut self) -> Result<()> {
        Ok(())
    }
}

/// The standard paper sizes Windows knows by number (`DMPAPER_*`), in mm (short × long side).
#[cfg_attr(not(windows), allow(dead_code))]
const PAPER_CODES: [(i16, f32, f32); 5] = [(9, 210.0, 297.0), (8, 297.0, 420.0), (11, 148.0, 210.0), (1, 215.9, 279.4), (66, 420.0, 594.0)];

/// The `DMPAPER_*` number of a paper (short × long side), or `None` for a custom size.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn paper_code(short: f32, long: f32) -> Option<i16> {
    PAPER_CODES.iter().find(|(_, s, l)| (s - short).abs() < 1.0 && (l - long).abs() < 1.0).map(|(c, ..)| *c)
}

#[cfg(windows)]
mod win {
    use anyhow::{Result, bail};
    use windows_sys::Win32::Foundation::{HWND, POINT};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODEW, DIB_RGB_COLORS, DM_ICMMETHOD, DM_IN_PROMPT, DM_ORIENTATION, DM_PAPERLENGTH, DM_PAPERSIZE, DM_PAPERWIDTH, DMICMMETHOD_NONE,
        DMORIENT_LANDSCAPE, DeleteDC, GetDeviceCaps, HALFTONE, HDC, LOGPIXELSX, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, SRCCOPY, SetBrushOrgEx,
        SetStretchBltMode, StretchDIBits,
    };
    use windows_sys::Win32::Graphics::Printing::{
        ClosePrinter, DocumentPropertiesW, EnumPrintersW, GetDefaultPrinterW, OpenPrinterW, PRINTER_ENUM_CONNECTIONS, PRINTER_ENUM_LOCAL, PRINTER_INFO_4W,
    };
    use windows_sys::Win32::Storage::Xps::{AbortDoc, DC_PAPERNAMES, DC_PAPERS, DC_PAPERSIZE, DOCINFOW, DeviceCapabilitiesW, EndDoc, EndPage, StartDocW, StartPage};
    use windows_sys::Win32::UI::WindowsAndMessaging::IDOK;

    use super::{BAND_ROWS, Choice, PaperSize, Printers, Request};

    /// `DMPAPER_USER`: a size given in millimeters.
    const PAPER_USER: i16 = 256;
    const DM_OUT_BUFFER: u32 = 2;
    const DM_IN_BUFFER: u32 = 8;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(Some(0)).collect()
    }

    /// Paper, orientation and color management into a printer's settings.
    ///
    /// # Safety
    /// `dm` must point to a valid DEVMODEW.
    unsafe fn prepare(dm: *mut DEVMODEW, req: &Request, papers: &[PaperSize]) {
        let dm = unsafe { &mut *dm };
        let (short, long) = (req.paper_mm[0].min(req.paper_mm[1]), req.paper_mm[0].max(req.paper_mm[1]));
        // SAFETY: the union's first variant is the printer one.
        let p = unsafe { &mut dm.Anonymous1.Anonymous1 };
        // The driver's own number where it offers the size (a 10 × 15 isn't standard), else
        // the standard one, else the size itself.
        let own = super::paper_of(papers, sk_core::print::PrintFormat::new(short, long)).map(|p| p.code);
        match own.or_else(|| super::paper_code(short, long)) {
            Some(code) => {
                p.dmPaperSize = code;
                dm.dmFields |= DM_PAPERSIZE;
            }
            None => {
                p.dmPaperSize = PAPER_USER;
                p.dmPaperWidth = (short * 10.0).round() as i16;
                p.dmPaperLength = (long * 10.0).round() as i16;
                dm.dmFields |= DM_PAPERSIZE | DM_PAPERWIDTH | DM_PAPERLENGTH;
            }
        }
        p.dmOrientation = if req.paper_mm[0] > req.paper_mm[1] { DMORIENT_LANDSCAPE as i16 } else { 1 };
        dm.dmFields |= DM_ORIENTATION;
        if req.own_colors {
            dm.dmICMMethod = DMICMMETHOD_NONE;
            dm.dmFields |= DM_ICMMETHOD;
        }
    }

    /// The printer the request names (or the default one) with paper, orientation and colors
    /// put in, starting from `base` if that is its settings (from the driver's dialog).
    pub fn prepared(req: &Request, base: Option<&Choice>) -> Result<Choice> {
        let Some(name) = req.printer.clone().or_else(default_printer) else { bail!("Kein Drucker installiert") };
        let device = wide(&name);
        let base = base.filter(|b| b.device == device).map(|b| b.devmode.as_slice());
        let Some(devmode) = settings_of(&device, base, req) else { bail!("Drucker „{name}“ nicht gefunden") };
        Ok(Choice { device, devmode })
    }

    /// The driver's own settings dialog for the printer (paper type, quality, …); whether it
    /// was confirmed. On the UI thread, with the app window as `owner`.
    pub fn properties(owner: isize, choice: &mut Choice) -> Result<bool> {
        let name = choice.name();
        // SAFETY: plain Win32 calls; the settings buffer is as large as the driver asks.
        unsafe {
            let mut handle = 0;
            if OpenPrinterW(choice.device.as_ptr(), &mut handle, std::ptr::null()) == 0 {
                bail!("Drucker „{name}“ nicht gefunden");
            }
            let size = DocumentPropertiesW(0, handle, choice.device.as_ptr(), std::ptr::null_mut(), std::ptr::null(), 0);
            if size <= 0 {
                ClosePrinter(handle);
                bail!("Einstellungen von „{name}“ nicht lesbar");
            }
            let mut out = vec![0u64; (size as usize).div_ceil(8)];
            let r = DocumentPropertiesW(
                owner as HWND,
                handle,
                choice.device.as_ptr(),
                out.as_mut_ptr() as *mut DEVMODEW,
                choice.devmode.as_ptr() as *const DEVMODEW,
                DM_IN_BUFFER | DM_OUT_BUFFER | DM_IN_PROMPT,
            );
            ClosePrinter(handle);
            match r {
                IDOK => {
                    choice.devmode = out;
                    Ok(true)
                }
                r if r < 0 => bail!("Einstellungen von „{name}“ nicht geöffnet"),
                _ => Ok(false),
            }
        }
    }

    /// A printer's settings (NUL-terminated name; from `base` if given) with paper and colors
    /// put in, as the driver accepts them; `None` if there is no such printer.
    fn settings_of(device: &[u16], base: Option<&[u64]>, req: &Request) -> Option<Vec<u64>> {
        // SAFETY: plain Win32 calls; the settings buffer is as large as the driver asks.
        unsafe {
            let mut handle = 0;
            if OpenPrinterW(device.as_ptr(), &mut handle, std::ptr::null()) == 0 {
                return None;
            }
            let size = DocumentPropertiesW(0, handle, device.as_ptr(), std::ptr::null_mut(), std::ptr::null(), 0);
            if size <= 0 {
                ClosePrinter(handle);
                return None;
            }
            let mut devmode = vec![0u64; (size as usize).div_ceil(8)];
            let dm = devmode.as_mut_ptr() as *mut DEVMODEW;
            let from = base.map_or(std::ptr::null(), |b| b.as_ptr() as *const DEVMODEW);
            DocumentPropertiesW(0, handle, device.as_ptr(), dm, from, DM_OUT_BUFFER | if base.is_some() { DM_IN_BUFFER } else { 0 });
            prepare(dm, req, &papers(&String::from_utf16_lossy(device.strip_suffix(&[0]).unwrap_or(device))));
            DocumentPropertiesW(0, handle, device.as_ptr(), dm, dm, DM_IN_BUFFER | DM_OUT_BUFFER);
            ClosePrinter(handle);
            Some(devmode)
        }
    }

    /// A NUL-terminated wide string from Windows.
    ///
    /// # Safety
    /// `p` is null or points to a NUL-terminated string.
    unsafe fn from_wide(p: *const u16) -> String {
        if p.is_null() {
            return String::new();
        }
        unsafe {
            let len = (0..).take_while(|&i| *p.add(i) != 0).count();
            String::from_utf16_lossy(std::slice::from_raw_parts(p, len))
        }
    }

    /// The installed printers (local and connected) with their papers, and the default one.
    /// Slow with network printers: off the UI thread.
    pub fn printers() -> Printers {
        // SAFETY: plain Win32 calls with buffers as large as Windows asks.
        unsafe {
            let flags = PRINTER_ENUM_LOCAL | PRINTER_ENUM_CONNECTIONS;
            let (mut needed, mut count) = (0u32, 0u32);
            EnumPrintersW(flags, std::ptr::null(), 4, std::ptr::null_mut(), 0, &mut needed, &mut count);
            let mut buf = vec![0u64; (needed as usize).div_ceil(8).max(1)];
            if EnumPrintersW(flags, std::ptr::null(), 4, buf.as_mut_ptr() as *mut u8, needed, &mut needed, &mut count) == 0 {
                count = 0;
            }
            let infos = std::slice::from_raw_parts(buf.as_ptr() as *const PRINTER_INFO_4W, count as usize);
            let names: Vec<String> = infos.iter().map(|i| from_wide(i.pPrinterName)).filter(|n| !n.is_empty()).collect();
            let list = names.into_iter().map(|n| (n.clone(), papers(&n))).collect();
            Printers { list, default: default_printer() }
        }
    }

    /// Windows' default printer.
    fn default_printer() -> Option<String> {
        // SAFETY: the buffer is as long as Windows asks.
        unsafe {
            let mut len = 0u32;
            GetDefaultPrinterW(std::ptr::null_mut(), &mut len);
            let mut name = vec![0u16; len as usize + 1];
            (len > 0 && GetDefaultPrinterW(name.as_mut_ptr(), &mut len) != 0).then(|| from_wide(name.as_ptr()))
        }
    }

    /// The papers a printer offers.
    fn papers(name: &str) -> Vec<PaperSize> {
        let device = wide(name);
        // SAFETY: each buffer is as large as the driver says for its query (names: 64 characters each).
        unsafe {
            let ask = |what, out: *mut u16| DeviceCapabilitiesW(device.as_ptr(), std::ptr::null(), what, out, std::ptr::null());
            let n = ask(DC_PAPERS, std::ptr::null_mut());
            if n <= 0 {
                return Vec::new();
            }
            let n = n as usize;
            let mut codes = vec![0u16; n];
            let mut names = vec![0u16; n * 64];
            let mut sizes = vec![POINT { x: 0, y: 0 }; n];
            if ask(DC_PAPERS, codes.as_mut_ptr()) <= 0 || ask(DC_PAPERNAMES, names.as_mut_ptr()) <= 0 || ask(DC_PAPERSIZE, sizes.as_mut_ptr() as *mut u16) <= 0 {
                return Vec::new();
            }
            let names: Vec<String> = names
                .chunks_exact(64)
                .map(|c| String::from_utf16_lossy(&c[..c.iter().position(|&ch| ch == 0).unwrap_or(64)]))
                .collect();
            let sizes: Vec<[i32; 2]> = sizes.iter().map(|p| [p.x, p.y]).collect();
            super::paper_list(&codes, &names, &sizes)
        }
    }

    /// A printer opened for a print job: its device context and sheet.
    pub struct Printer {
        dc: HDC,
        /// A file the printer writes to (printing to a file, for the tests).
        output: Option<Vec<u16>>,
        dpi: f32,
        physical: [i32; 2],
        offset: [i32; 2],
        started: bool,
        finished: bool,
    }

    // Opened, used and closed by the print job's thread.
    unsafe impl Send for Printer {}

    impl Printer {
        /// Opens the chosen printer (on the thread that prints), writing to `output` if given.
        pub fn open(choice: &Choice, output: Option<&std::path::Path>) -> Result<Self> {
            // SAFETY: the settings are a DEVMODEW the driver filled; the strings end with NUL.
            let dc = unsafe { CreateDCW(wide("WINSPOOL").as_ptr(), choice.device.as_ptr(), std::ptr::null(), choice.devmode.as_ptr() as *const DEVMODEW) };
            if dc == 0 {
                bail!("Der Drucker ist nicht bereit");
            }
            // SAFETY: a valid device context.
            let cap = |i: u32| unsafe { GetDeviceCaps(dc, i as i32) };
            Ok(Self {
                dc,
                output: output.map(|o| wide(&o.to_string_lossy())),
                dpi: cap(LOGPIXELSX).max(1) as f32,
                physical: [cap(PHYSICALWIDTH), cap(PHYSICALHEIGHT)],
                offset: [cap(PHYSICALOFFSETX), cap(PHYSICALOFFSETY)],
                started: false,
                finished: false,
            })
        }

        /// The printer's sheet (mm), as it will come out.
        pub fn paper_mm(&self) -> [f32; 2] {
            [self.physical[0] as f32 / self.dpi * 25.4, self.physical[1] as f32 / self.dpi * 25.4]
        }

        pub fn dpi(&self) -> f32 {
            self.dpi
        }

        pub fn start(&mut self, title: &str) -> Result<()> {
            let name = wide(title);
            // SAFETY: the strings live until the call returns.
            unsafe {
                let mut di: DOCINFOW = std::mem::zeroed();
                di.cbSize = std::mem::size_of::<DOCINFOW>() as i32;
                di.lpszDocName = name.as_ptr();
                di.lpszOutput = self.output.as_ref().map_or(std::ptr::null(), |o| o.as_ptr());
                if StartDocW(self.dc, &di) <= 0 {
                    bail!("Der Drucker nimmt den Auftrag nicht an");
                }
            }
            self.started = true;
            Ok(())
        }

        /// One page, over the whole sheet (the printer leaves out what it can't print), in bands.
        pub fn page(&mut self, page: &sk_core::print_page::Page) -> Result<()> {
            let w = page.width;
            // Bottom-up BGR rows, padded to four bytes, as every driver takes them.
            let stride = (w * 3).div_ceil(4) * 4;
            let mut bits = vec![0u8; stride * BAND_ROWS.min(page.height)];
            // SAFETY: plain GDI calls on a started document; each band's header describes its buffer.
            unsafe {
                if StartPage(self.dc) <= 0 {
                    bail!("Seite beim Drucker nicht begonnen");
                }
                SetStretchBltMode(self.dc, HALFTONE);
                SetBrushOrgEx(self.dc, 0, 0, std::ptr::null_mut());
                for (y0, y1, dy, dh) in super::bands(page.height, self.physical[1]) {
                    let h = y1 - y0;
                    for y in 0..h {
                        let src = &page.rgb[(y1 - 1 - y) * w * 3..(y1 - y) * w * 3];
                        let dst = &mut bits[y * stride..y * stride + w * 3];
                        for (d, s) in dst.chunks_exact_mut(3).zip(src.chunks_exact(3)) {
                            d.copy_from_slice(&[s[2], s[1], s[0]]);
                        }
                    }
                    let mut bmi: BITMAPINFO = std::mem::zeroed();
                    bmi.bmiHeader = BITMAPINFOHEADER {
                        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                        biWidth: w as i32,
                        biHeight: h as i32,
                        biPlanes: 1,
                        biBitCount: 24,
                        biCompression: BI_RGB,
                        biSizeImage: (stride * h) as u32,
                        biXPelsPerMeter: 0,
                        biYPelsPerMeter: 0,
                        biClrUsed: 0,
                        biClrImportant: 0,
                    };
                    // Device coordinates start at the printable area.
                    let drawn = StretchDIBits(
                        self.dc,
                        -self.offset[0],
                        dy - self.offset[1],
                        self.physical[0],
                        dh,
                        0,
                        0,
                        w as i32,
                        h as i32,
                        bits.as_ptr().cast(),
                        &bmi,
                        DIB_RGB_COLORS,
                        SRCCOPY,
                    );
                    if drawn == 0 || drawn == -1 {
                        bail!("Der Drucker nimmt das Bild nicht an");
                    }
                }
                if EndPage(self.dc) <= 0 {
                    bail!("Seite beim Drucker nicht abgeschlossen");
                }
            }
            Ok(())
        }

        pub fn finish(&mut self) -> Result<()> {
            // SAFETY: a started document on a valid device context.
            if self.started && unsafe { EndDoc(self.dc) } <= 0 {
                bail!("Druckauftrag nicht abgeschlossen");
            }
            self.finished = true;
            Ok(())
        }
    }

    impl Drop for Printer {
        fn drop(&mut self) {
            // SAFETY: the device context belongs to this printer.
            unsafe {
                if self.started && !self.finished {
                    AbortDoc(self.dc);
                }
                DeleteDC(self.dc);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn papers_of_a_printer() {
        let names: Vec<String> = ["A4", "10 x 15 cm", "Benutzerdefiniert", "A4"].iter().map(|s| s.to_string()).collect();
        let papers = paper_list(&[9, 300, 256, 9], &names, &[[2100, 2970], [1520, 1020], [0, 0], [2100, 2970]]);
        assert_eq!(papers.len(), 2);
        assert_eq!(papers[1].mm, [102.0, 152.0]);
        let ten = sk_core::print::PrintFormat::new(152.0, 101.6);
        assert_eq!(paper_of(&papers, ten).map(|p| p.code), Some(300));
        assert!(paper_of(&papers, sk_core::print::PrintFormat::new(130.0, 180.0)).is_none());
        let printers = Printers { list: vec![("Foto".into(), papers.clone())], default: Some("Foto".into()) };
        assert_eq!(printers.papers(None).map(|p| p.len()), Some(2));
        assert!(printers.papers(Some("Weg")).is_none());
    }

    #[test]
    fn standard_papers_go_by_number() {
        assert_eq!(paper_code(210.0, 297.0), Some(9));
        assert_eq!(paper_code(102.0, 152.0), None);
    }

    #[test]
    fn pages_are_a_whole_fraction_of_the_printer() {
        assert_eq!(page_dpi(600.0), 300.0);
        assert_eq!(page_dpi(1200.0), 300.0);
        assert_eq!(page_dpi(720.0), 360.0);
        assert_eq!(page_dpi(1440.0), 360.0);
        assert_eq!(page_dpi(2880.0), 360.0);
        assert_eq!(page_dpi(300.0), 300.0);
        assert_eq!(page_dpi(360.0), 360.0);
    }

    #[test]
    fn bands_cover_the_sheet_without_gaps() {
        let b = bands(1300, 2600);
        assert_eq!(b.len(), 3);
        assert_eq!((b[0].0, b[0].2), (0, 0));
        for w in b.windows(2) {
            assert_eq!(w[0].1, w[1].0);
            assert_eq!(w[0].2 + w[0].3, w[1].2);
        }
        let last = b.last().unwrap();
        assert_eq!((last.1, last.2 + last.3), (1300, 2600));
    }
}
