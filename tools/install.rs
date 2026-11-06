//! Installing and uninstalling Jabro Draw for the current Windows user (no administrator
//! rights needed). Shared by the setup program and by the app's `--uninstall`.
//!
//! * files go to `%LOCALAPPDATA%\Programs\Jabro Draw` (or a folder the user picks),
//! * Start menu (and optionally desktop) shortcuts,
//! * an entry in Settings > Apps so it can be removed like any other program,
//! * optionally: `.jabro` files open with Jabro Draw.
#![allow(dead_code)]

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::process::Command;

pub const APP_NAME: &str = "Jabro Draw";
pub const EXE_NAME: &str = "jabro-draw.exe";
/// Written into the install folder; the uninstaller only ever deletes a folder that has it.
pub const MARKER: &str = ".jabro-draw-install";
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\JabroDraw";
const CLASS_KEY: &str = r"Software\Classes\JabroDraw.Document";
const EXT_KEY: &str = r"Software\Classes\.jabro";

#[cfg(windows)]
mod sys {
    use super::c_void;
    pub type Hkey = isize;
    pub const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize;
    pub const KEY_READ: u32 = 0x2_0019;
    pub const KEY_WRITE: u32 = 0x2_0006;
    pub const REG_SZ: u32 = 1;
    pub const REG_DWORD: u32 = 4;
    pub const RRF_RT_REG_SZ: u32 = 0x2;
    #[link(name = "advapi32")]
    extern "system" {
        pub fn RegCreateKeyExW(
            key: Hkey,
            sub: *const u16,
            reserved: u32,
            class: *const u16,
            options: u32,
            sam: u32,
            security: *const c_void,
            result: *mut Hkey,
            disposition: *mut u32,
        ) -> i32;
        pub fn RegSetValueExW(key: Hkey, name: *const u16, reserved: u32, kind: u32, data: *const u8, len: u32) -> i32;
        pub fn RegCloseKey(key: Hkey) -> i32;
        pub fn RegDeleteTreeW(key: Hkey, sub: *const u16) -> i32;
        pub fn RegGetValueW(
            key: Hkey,
            sub: *const u16,
            value: *const u16,
            flags: u32,
            kind: *mut u32,
            data: *mut c_void,
            len: *mut u32,
        ) -> i32;
    }
    #[link(name = "shell32")]
    extern "system" {
        pub fn SHChangeNotify(event: i32, flags: u32, a: *const c_void, b: *const c_void);
    }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

/// A registry value under HKEY_CURRENT_USER: text, or a number.
pub enum Value<'a> {
    Text(&'a str),
    Number(u32),
}

#[cfg(windows)]
pub fn reg_set(key: &str, name: Option<&str>, value: Value) -> Result<(), String> {
    use sys::*;
    let mut h: Hkey = 0;
    let k = wide(key);
    let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, std::ptr::null(), 0, KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut()) };
    if r != 0 {
        return Err(format!("could not write the registry key {key} (error {r})"));
    }
    let n = name.map(wide);
    let name_ptr = n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let r = match value {
        Value::Text(t) => {
            let data = wide(t);
            unsafe { RegSetValueExW(h, name_ptr, 0, REG_SZ, data.as_ptr() as *const u8, (data.len() * 2) as u32) }
        }
        Value::Number(v) => unsafe { RegSetValueExW(h, name_ptr, 0, REG_DWORD, &v as *const u32 as *const u8, 4) },
    };
    unsafe { RegCloseKey(h) };
    if r != 0 {
        return Err(format!("could not write a registry value under {key} (error {r})"));
    }
    Ok(())
}

#[cfg(windows)]
pub fn reg_get(key: &str, name: Option<&str>) -> Option<String> {
    use sys::*;
    let k = wide(key);
    let n = name.map(wide);
    let name_ptr = n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr());
    let mut buf = vec![0u16; 2048];
    let mut len = (buf.len() * 2) as u32;
    let r = unsafe {
        RegGetValueW(HKEY_CURRENT_USER, k.as_ptr(), name_ptr, RRF_RT_REG_SZ, std::ptr::null_mut(), buf.as_mut_ptr() as *mut c_void, &mut len)
    };
    if r != 0 {
        return None;
    }
    let chars = (len as usize / 2).saturating_sub(1);
    Some(String::from_utf16_lossy(&buf[..chars]))
}

#[cfg(windows)]
pub fn reg_delete_tree(key: &str) {
    let k = wide(key);
    unsafe { sys::RegDeleteTreeW(sys::HKEY_CURRENT_USER, k.as_ptr()) };
    // the (now empty) key itself
    if let Some((parent, leaf)) = key.rsplit_once('\\') {
        let p = wide(parent);
        let mut h: sys::Hkey = 0;
        let r = unsafe {
            sys::RegCreateKeyExW(sys::HKEY_CURRENT_USER, p.as_ptr(), 0, std::ptr::null(), 0, sys::KEY_WRITE | sys::KEY_READ, std::ptr::null(), &mut h, std::ptr::null_mut())
        };
        if r == 0 {
            let l = wide(leaf);
            unsafe {
                sys::RegDeleteTreeW(h, l.as_ptr());
                RegDeleteKeyW(h, l.as_ptr());
                sys::RegCloseKey(h);
            }
        }
    }
}

#[cfg(windows)]
#[link(name = "advapi32")]
extern "system" {
    fn RegDeleteKeyW(key: sys::Hkey, sub: *const u16) -> i32;
}

/// Tell Explorer that file associations changed (so icons update right away).
pub fn notify_shell() {
    #[cfg(windows)]
    unsafe {
        sys::SHChangeNotify(0x0800_0000, 0, std::ptr::null(), std::ptr::null());
    }
}

/// Run a PowerShell snippet without showing a window.
fn powershell(script: &str) -> Result<(), String> {
    let mut cmd = Command::new("powershell");
    cmd.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().map_err(|e| format!("could not run PowerShell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn ps_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

/// The folders shortcuts go to, as PowerShell expressions ("Programs" is the Start menu).
fn link_folders(desktop: bool) -> Vec<&'static str> {
    let mut v = vec!["[Environment]::GetFolderPath('Programs')"];
    if desktop {
        v.push("[Environment]::GetFolderPath('Desktop')");
    }
    v
}

/// Start menu (and desktop) shortcuts to `exe`. With `folder_override` they go there instead
/// (used by the self-test so nothing lands in the real Start menu).
pub fn create_shortcuts(exe: &Path, desktop: bool, folder_override: Option<&Path>) -> Result<(), String> {
    let exe_s = exe.to_string_lossy();
    let dir = exe.parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let folders: Vec<String> = match folder_override {
        Some(f) => vec![ps_quote(&f.to_string_lossy())],
        None => link_folders(desktop).into_iter().map(String::from).collect(),
    };
    let mut script = String::from("$ErrorActionPreference = 'Stop'; $sh = New-Object -ComObject WScript.Shell; ");
    for f in folders {
        script.push_str(&format!(
            "$l = $sh.CreateShortcut((Join-Path ({f}) {name})); $l.TargetPath = {exe}; $l.WorkingDirectory = {dir}; \
             $l.IconLocation = {icon}; $l.Description = 'Vector and paint editor'; $l.Save(); ",
            name = ps_quote(&format!("{APP_NAME}.lnk")),
            exe = ps_quote(&exe_s),
            dir = ps_quote(&dir),
            icon = ps_quote(&format!("{exe_s},0")),
        ));
    }
    powershell(&script).map_err(|e| format!("could not create the shortcuts: {e}"))
}

pub fn remove_shortcuts() {
    let mut script = String::new();
    for f in link_folders(true) {
        script.push_str(&format!(
            "$p = Join-Path ({f}) {name}; if (Test-Path -LiteralPath $p) {{ Remove-Item -LiteralPath $p -Force }}; ",
            name = ps_quote(&format!("{APP_NAME}.lnk"))
        ));
    }
    let _ = powershell(&script);
}

fn folder_size_kb(dir: &Path) -> u32 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .map(|e| match e.metadata() {
                    Ok(m) if m.is_dir() => folder_size_kb(&e.path()) as u64 * 1024,
                    Ok(m) => m.len(),
                    Err(_) => 0,
                })
                .sum::<u64>()
        })
        .map_or(0, |b| (b / 1024) as u32)
}

/// The Settings > Apps entry, and optionally the `.jabro` file type.
#[cfg(windows)]
pub fn register(dir: &Path, version: &str, associate: bool) -> Result<(), String> {
    let exe = dir.join(EXE_NAME);
    let exe_s = exe.to_string_lossy().to_string();
    let k = UNINSTALL_KEY;
    reg_set(k, Some("DisplayName"), Value::Text(APP_NAME))?;
    reg_set(k, Some("DisplayVersion"), Value::Text(version))?;
    reg_set(k, Some("Publisher"), Value::Text(APP_NAME))?;
    reg_set(k, Some("DisplayIcon"), Value::Text(&format!("{exe_s},0")))?;
    reg_set(k, Some("InstallLocation"), Value::Text(&dir.to_string_lossy()))?;
    reg_set(k, Some("UninstallString"), Value::Text(&format!("\"{exe_s}\" --uninstall")))?;
    reg_set(k, Some("NoModify"), Value::Number(1))?;
    reg_set(k, Some("NoRepair"), Value::Number(1))?;
    reg_set(k, Some("EstimatedSize"), Value::Number(folder_size_kb(dir)))?;
    if associate {
        reg_set(EXT_KEY, None, Value::Text("JabroDraw.Document"))?;
        reg_set(CLASS_KEY, None, Value::Text("Jabro Draw drawing"))?;
        reg_set(&format!(r"{CLASS_KEY}\DefaultIcon"), None, Value::Text(&format!("{exe_s},0")))?;
        reg_set(&format!(r"{CLASS_KEY}\shell\open\command"), None, Value::Text(&format!("\"{exe_s}\" \"%1\"")))?;
        notify_shell();
    }
    Ok(())
}

/// Where Jabro Draw is installed, according to the Settings > Apps entry.
#[cfg(windows)]
pub fn installed_dir() -> Option<PathBuf> {
    reg_get(UNINSTALL_KEY, Some("InstallLocation")).map(PathBuf::from)
}

#[cfg(windows)]
pub fn unregister() {
    reg_delete_tree(UNINSTALL_KEY);
    // only drop the file type if it still points at us
    if reg_get(EXT_KEY, None).as_deref() == Some("JabroDraw.Document") {
        reg_delete_tree(EXT_KEY);
    }
    reg_delete_tree(CLASS_KEY);
    notify_shell();
}

pub fn default_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join(APP_NAME)
}

/// Is `dir` a folder our setup made (safe to delete completely)?
pub fn is_install_dir(dir: &Path) -> bool {
    dir.join(MARKER).is_file() && dir.join(EXE_NAME).is_file()
}

/// Delete the install folder once this program has exited (it cannot delete its own .exe
/// while running). Refuses anything that is not a folder our setup made.
pub fn delete_dir_after_exit(dir: &Path) -> Result<(), String> {
    if !is_install_dir(dir) {
        return Err(format!("{} is not a Jabro Draw install folder", dir.display()));
    }
    let mut cmd = Command::new("cmd");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.raw_arg(format!("/c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"", dir.display()));
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}
