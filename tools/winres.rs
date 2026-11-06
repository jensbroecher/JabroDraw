//! Writes a compiled Windows resource file (.res) with an icon and version information,
//! without needing the Windows SDK resource compiler. Shared by the build scripts of the app
//! and of the setup program; the MSVC linker accepts .res files directly.

use std::path::Path;

const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;
const RT_VERSION: u16 = 16;

fn u16s(s: &str) -> Vec<u8> {
    s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect()
}

fn pad4(v: &mut Vec<u8>) {
    while v.len() % 4 != 0 {
        v.push(0);
    }
}

/// One entry of a .res file (type and name as numbers).
fn resource(out: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    out.extend((data.len() as u32).to_le_bytes());
    out.extend(32u32.to_le_bytes()); // header size
    out.extend([0xFF, 0xFF]);
    out.extend(kind.to_le_bytes());
    out.extend([0xFF, 0xFF]);
    out.extend(name.to_le_bytes());
    out.extend(0u32.to_le_bytes()); // data version
    out.extend(flags.to_le_bytes());
    out.extend(0x0409u16.to_le_bytes()); // English (US)
    out.extend(0u32.to_le_bytes()); // version
    out.extend(0u32.to_le_bytes()); // characteristics
    out.extend(data);
    pad4(out);
}

/// A node of the VS_VERSIONINFO tree.
fn version_node(key: &str, value: &[u8], value_len: u16, text: bool, children: &[Vec<u8>]) -> Vec<u8> {
    let mut v = vec![0, 0];
    v.extend(value_len.to_le_bytes());
    v.extend((text as u16).to_le_bytes());
    v.extend(u16s(key));
    pad4(&mut v);
    v.extend(value);
    for c in children {
        pad4(&mut v);
        v.extend(c);
    }
    let len = v.len() as u16;
    v[0..2].copy_from_slice(&len.to_le_bytes());
    v
}

fn version_info(version: &str, description: &str, filename: &str) -> Vec<u8> {
    let nums: Vec<u16> = version.split('.').map(|p| p.parse().unwrap_or(0)).chain([0, 0, 0, 0]).take(4).collect();
    let ms = ((nums[0] as u32) << 16) | nums[1] as u32;
    let ls = ((nums[2] as u32) << 16) | nums[3] as u32;
    let mut fixed = Vec::new();
    for v in [0xFEEF04BDu32, 0x0001_0000, ms, ls, ms, ls, 0x3F, 0, 0x0004_0004, 1, 0, 0, 0] {
        fixed.extend(v.to_le_bytes());
    }
    let string = |k: &str, v: &str| {
        let val = u16s(v);
        version_node(k, &val, (val.len() / 2) as u16, true, &[])
    };
    let strings = [
        string("CompanyName", "Jabro Draw"),
        string("FileDescription", description),
        string("FileVersion", version),
        string("InternalName", filename.trim_end_matches(".exe")),
        string("OriginalFilename", filename),
        string("ProductName", "Jabro Draw"),
        string("ProductVersion", version),
    ];
    let table = version_node("040904b0", &[], 0, true, &strings);
    let sfi = version_node("StringFileInfo", &[], 0, true, &[table]);
    let translation = version_node("Translation", &0x04B0_0409u32.to_le_bytes(), 4, false, &[]);
    let vfi = version_node("VarFileInfo", &[], 0, true, &[translation]);
    version_node("VS_VERSION_INFO", &fixed, fixed.len() as u16, false, &[sfi, vfi])
}

/// Icon from an .ico file plus version strings, written as a .res file to `out`.
pub fn write(ico_path: &Path, out: &Path, version: &str, description: &str, filename: &str) {
    let ico = std::fs::read(ico_path).expect("icon file (make it with --make-icon assets)");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let mut res = Vec::new();
    resource(&mut res, 0, 0, 0, &[]); // a .res file starts with an empty entry
    let mut group = vec![0, 0, 1, 0];
    group.extend((count as u16).to_le_bytes());
    for i in 0..count {
        let e = &ico[6 + i * 16..6 + (i + 1) * 16];
        let size = u32::from_le_bytes([e[8], e[9], e[10], e[11]]) as usize;
        let offset = u32::from_le_bytes([e[12], e[13], e[14], e[15]]) as usize;
        let id = (i + 1) as u16;
        resource(&mut res, RT_ICON, id, 0x1010, &ico[offset..offset + size]);
        group.extend(&e[0..12]); // width, height, colours, reserved, planes, bit count, bytes
        group.extend(id.to_le_bytes());
    }
    resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);
    resource(&mut res, RT_VERSION, 1, 0x0030, &version_info(version, description, filename));
    std::fs::write(out, res).expect("write .res");
}
