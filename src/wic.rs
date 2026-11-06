//! Opening pictures through Windows' own image codecs (WIC): HEIC / HEIF, AVIF, JPEG XR, camera
//! RAW (DNG, CR2, NEF, …) and whatever else a Windows codec extension adds. The decoders belong
//! to Windows or to Microsoft Store extensions that Microsoft licenses, so Jabro Draw itself
//! ships no HEVC decoder (HEVC is patent-encumbered).

use std::path::Path;

/// Formats Windows can usually open (depending on which codec extensions are installed).
pub const EXTENSIONS: [&str; 14] =
    ["heic", "heif", "hif", "avif", "jxr", "wdp", "hdp", "dng", "cr2", "cr3", "nef", "arw", "orf", "rw2"];

/// What to tell the user when Windows lacks the codec.
pub fn codec_hint(ext: &str) -> &'static str {
    match ext {
        "heic" | "heif" | "hif" => {
            "Windows needs the \"HEIF Image Extensions\" (free) and the \"HEVC Video Extensions\" (a small fee, or preinstalled by many PC makers) from the Microsoft Store to open HEIC photos."
        }
        "avif" => "Windows needs the free \"AV1 Video Extension\" from the Microsoft Store to open AVIF pictures.",
        "dng" | "cr2" | "cr3" | "nef" | "arw" | "orf" | "rw2" => {
            "Windows needs the free \"Raw Image Extension\" from the Microsoft Store to open camera RAW files."
        }
        _ => "Windows has no decoder for this picture format.",
    }
}

/// Decode with WIC into straight (unpremultiplied) RGBA.
#[cfg(windows)]
pub fn load(path: &Path) -> Result<image::RgbaImage, String> {
    use windows::core::HSTRING;
    use windows::Win32::Foundation::GENERIC_READ;
    use windows::Win32::Graphics::Imaging::*;
    use windows::Win32::System::Com::*;
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let no_codec = |e: windows::core::Error| format!("{} ({})", codec_hint(&ext), e.message());
    unsafe {
        // the UI thread may already have COM set up in another mode; either way it is usable
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let factory: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).map_err(|e| e.message().to_string())?;
        let name = HSTRING::from(path.as_os_str());
        let decoder = factory
            .CreateDecoderFromFilename(&name, None, GENERIC_READ, WICDecodeMetadataCacheOnDemand)
            .map_err(no_codec)?;
        let frame = decoder.GetFrame(0).map_err(no_codec)?;
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h).map_err(|e| e.message().to_string())?;
        if w == 0 || h == 0 || w as u64 * h as u64 > 400_000_000 {
            return Err(format!("unsupported picture size {w} × {h}"));
        }
        let rgba = WICConvertBitmapSource(&GUID_WICPixelFormat32bppRGBA, &frame).map_err(no_codec)?;
        let mut buf = vec![0u8; w as usize * h as usize * 4];
        rgba.CopyPixels(std::ptr::null(), w * 4, &mut buf).map_err(no_codec)?;
        image::RgbaImage::from_raw(w, h, buf).ok_or_else(|| "decoding failed".into())
    }
}

#[cfg(not(windows))]
pub fn load(path: &Path) -> Result<image::RgbaImage, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    Err(codec_hint(&ext).into())
}
