//! The printer (Windows): the system's print dialog, then the pages as bitmaps to the chosen
//! printer through GDI. The pages come ready from `sk_core::print_page`; the printer gets them
//! at the size of its sheet and scales them to its own resolution.
//!
//! The dialog runs on the UI thread (it belongs to the window, like the file dialogs) and only
//! hands back the printer and its settings ([`Choice`]); the print job then opens the printer on
//! its own thread, renders and sends the pages there.

#[cfg(not(windows))]
use anyhow::Result;

/// What the job asks of the printer before its dialog shows.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Request {
    /// The paper (mm), turned like the first page.
    pub paper_mm: [f32; 2],
    /// Silberkorn converts the colors with the printer's profile: the driver must not again.
    pub own_colors: bool,
}

/// The printer and its settings as chosen in the dialog: plain data, for the print job's thread.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct Choice {
    /// The printer's name, NUL-terminated (UTF-16).
    device: Vec<u16>,
    /// Its settings (a DEVMODEW with the driver's part), aligned.
    devmode: Vec<u64>,
    /// Copies the driver doesn't make itself: each page is sent this often.
    copies: u32,
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

#[cfg(windows)]
pub use win::{Printer, choose, named};

#[cfg(not(windows))]
pub struct Printer;

#[cfg(not(windows))]
pub fn choose(_owner: isize, _req: &Request) -> Result<Option<Choice>> {
    anyhow::bail!("Drucken gibt es nur unter Windows")
}

#[cfg(not(windows))]
pub fn named(_name: &str, _req: &Request) -> Result<Choice> {
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

    pub fn copies(&self) -> u32 {
        1
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
    use windows_sys::Win32::Foundation::{GlobalFree, HGLOBAL, HWND};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateDCW, DEVMODEW, DIB_RGB_COLORS, DM_ICMMETHOD, DM_ORIENTATION, DM_PAPERLENGTH, DM_PAPERSIZE, DM_PAPERWIDTH, DMICMMETHOD_NONE,
        DMORIENT_LANDSCAPE, DeleteDC, GetDeviceCaps, HALFTONE, HDC, LOGPIXELSX, PHYSICALHEIGHT, PHYSICALOFFSETX, PHYSICALOFFSETY, PHYSICALWIDTH, SRCCOPY, SetBrushOrgEx,
        SetStretchBltMode, StretchDIBits,
    };
    use windows_sys::Win32::Graphics::Printing::{ClosePrinter, DocumentPropertiesW, OpenPrinterW};
    use windows_sys::Win32::Storage::Xps::{AbortDoc, DOCINFOW, EndDoc, EndPage, StartDocW, StartPage};
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows_sys::Win32::UI::Controls::Dialogs::{
        CommDlgExtendedError, DEVNAMES, PD_NOPAGENUMS, PD_NOSELECTION, PD_RETURNDEFAULT, PD_USEDEVMODECOPIESANDCOLLATE, PRINTDLGW, PrintDlgW,
    };

    use super::{BAND_ROWS, Choice, Request};

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
    unsafe fn prepare(dm: *mut DEVMODEW, req: &Request) {
        let dm = unsafe { &mut *dm };
        let (short, long) = (req.paper_mm[0].min(req.paper_mm[1]), req.paper_mm[0].max(req.paper_mm[1]));
        // SAFETY: the union's first variant is the printer one.
        let p = unsafe { &mut dm.Anonymous1.Anonymous1 };
        match super::paper_code(short, long) {
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

    /// A copy of the global memory's bytes, aligned for a DEVMODEW.
    ///
    /// # Safety
    /// `g` must be a valid global memory handle.
    unsafe fn copy_global(g: HGLOBAL) -> Vec<u64> {
        unsafe {
            let size = GlobalSize(g);
            let p = GlobalLock(g) as *const u8;
            if p.is_null() || size == 0 {
                return Vec::new();
            }
            let mut out = vec![0u64; size.div_ceil(8)];
            std::ptr::copy_nonoverlapping(p, out.as_mut_ptr() as *mut u8, size);
            GlobalUnlock(g);
            out
        }
    }

    /// The system's print dialog (printer, copies, its settings), with paper and colors
    /// preset; `None` if it was cancelled. On the UI thread, with the app window as `owner`.
    pub fn choose(owner: isize, req: &Request) -> Result<Option<Choice>> {
        // SAFETY: plain Win32 calls with properly sized, zeroed structs; the global memory the
        // dialog hands back is copied and freed here.
        unsafe {
            let mut pd: PRINTDLGW = std::mem::zeroed();
            pd.lStructSize = std::mem::size_of::<PRINTDLGW>() as u32;
            pd.hwndOwner = owner as HWND;
            // The default printer's settings first, to put paper and colors in.
            pd.Flags = PD_RETURNDEFAULT;
            if PrintDlgW(&mut pd) != 0 && !pd.hDevMode.is_null() {
                let dm = GlobalLock(pd.hDevMode) as *mut DEVMODEW;
                if !dm.is_null() {
                    prepare(dm, req);
                    GlobalUnlock(pd.hDevMode);
                }
            }
            pd.Flags = PD_NOPAGENUMS | PD_NOSELECTION | PD_USEDEVMODECOPIESANDCOLLATE;
            pd.nCopies = 1;
            let ok = PrintDlgW(&mut pd) != 0;
            let choice = if ok && !pd.hDevMode.is_null() && !pd.hDevNames.is_null() {
                let devmode = copy_global(pd.hDevMode);
                let names = GlobalLock(pd.hDevNames) as *const DEVNAMES;
                let device = if names.is_null() {
                    Vec::new()
                } else {
                    let start = (names as *const u16).add((*names).wDeviceOffset as usize);
                    let len = (0..).take_while(|&i| *start.add(i) != 0).count();
                    let device = std::slice::from_raw_parts(start, len + 1).to_vec();
                    GlobalUnlock(pd.hDevNames);
                    device
                };
                Some(Choice { device, devmode, copies: pd.nCopies.max(1) as u32 })
            } else {
                None
            };
            for g in [pd.hDevMode, pd.hDevNames] {
                if !g.is_null() {
                    GlobalFree(g);
                }
            }
            if !ok {
                return match CommDlgExtendedError() {
                    0 => Ok(None),
                    e => bail!("Druckdialog fehlgeschlagen (Fehler {e:#x})"),
                };
            }
            match choice {
                Some(c) if c.device.len() > 1 && !c.devmode.is_empty() => Ok(Some(c)),
                _ => bail!("Der Druckdialog hat keinen Drucker geliefert"),
            }
        }
    }

    /// A printer by name with paper and colors set, without a dialog (for tests with
    /// "Microsoft Print to PDF").
    pub fn named(name: &str, req: &Request) -> Result<Choice> {
        let device = wide(name);
        // SAFETY: plain Win32 calls; the settings buffer is as large as the driver asks.
        unsafe {
            let mut handle = 0;
            if OpenPrinterW(device.as_ptr(), &mut handle, std::ptr::null()) == 0 {
                bail!("Drucker „{name}“ nicht gefunden");
            }
            let size = DocumentPropertiesW(0, handle, device.as_ptr(), std::ptr::null_mut(), std::ptr::null(), 0);
            if size <= 0 {
                ClosePrinter(handle);
                bail!("Einstellungen von „{name}“ nicht lesbar");
            }
            let mut devmode = vec![0u64; (size as usize).div_ceil(8)];
            let dm = devmode.as_mut_ptr() as *mut DEVMODEW;
            DocumentPropertiesW(0, handle, device.as_ptr(), dm, std::ptr::null(), DM_OUT_BUFFER);
            prepare(dm, req);
            DocumentPropertiesW(0, handle, device.as_ptr(), dm, dm, DM_IN_BUFFER | DM_OUT_BUFFER);
            ClosePrinter(handle);
            Ok(Choice { device, devmode, copies: 1 })
        }
    }

    /// A printer opened for a print job: its device context and sheet.
    pub struct Printer {
        dc: HDC,
        /// A file the printer writes to (printing to a file, for the tests).
        output: Option<Vec<u16>>,
        copies: u32,
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
                copies: choice.copies,
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

        /// Copies the driver doesn't make itself: each page is sent this often.
        pub fn copies(&self) -> u32 {
            self.copies
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
