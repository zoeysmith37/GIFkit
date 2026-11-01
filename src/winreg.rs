//! Windows: `.gifkit` projects open with GIFkit, show its icon, and get
//! thumbnails in Explorer from gifkit_thumbs.dll next to the program. All of
//! it for the current user only (no admin rights). Done at start-up, so it
//! follows the program if its folder moves.

use std::ffi::c_void;
use std::path::Path;

type Hkey = *mut c_void;
const HKEY_CURRENT_USER: Hkey = 0x8000_0001u32 as i32 as isize as Hkey;
const KEY_WRITE: u32 = 0x20006;
const REG_SZ: u32 = 1;

#[link(name = "advapi32")]
extern "system" {
    fn RegCreateKeyExW(hkey: Hkey, sub: *const u16, reserved: u32, class: *const u16, options: u32, sam: u32, sec: *const c_void, out: *mut Hkey, disposition: *mut u32) -> u32;
    fn RegSetValueExW(hkey: Hkey, name: *const u16, reserved: u32, kind: u32, data: *const u8, len: u32) -> u32;
    fn RegCloseKey(hkey: Hkey) -> u32;
}

#[link(name = "shell32")]
extern "system" {
    fn SHChangeNotify(event: i32, flags: u32, a: *const c_void, b: *const c_void);
}

/// The thumbnail class (as in tools/winthumb).
const CLSID: &str = "{6F3B1C2E-9A4D-4E7B-8C51-2D7A0B9E4F13}";
/// Explorer's "thumbnail provider" slot.
const THUMB_SLOT: &str = "{e357fccd-a995-4576-b01f-234630154e96}";

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Set `value` (None: the default) of HKCU\Software\Classes\`key`.
fn set(key: &str, value: Option<&str>, data: &str) -> Result<(), String> {
    let path = wide(&format!("Software\\Classes\\{key}"));
    let mut h: Hkey = std::ptr::null_mut();
    let r = unsafe { RegCreateKeyExW(HKEY_CURRENT_USER, path.as_ptr(), 0, std::ptr::null(), 0, KEY_WRITE, std::ptr::null(), &mut h, std::ptr::null_mut()) };
    if r != 0 {
        return Err(format!("can't open {key} ({r})"));
    }
    let name = value.map(wide);
    let d = wide(data);
    let r = unsafe { RegSetValueExW(h, name.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()), 0, REG_SZ, d.as_ptr() as *const u8, (d.len() * 2) as u32) };
    unsafe { RegCloseKey(h) };
    if r != 0 {
        return Err(format!("can't write {key} ({r})"));
    }
    Ok(())
}

/// Register projects with `exe` (and thumbnails, when the DLL is beside it).
pub fn register(exe: &Path) -> Result<(), String> {
    let exe_s = exe.display().to_string();
    set(".gifkit", None, "GIFkit.Project")?;
    set(".gifkit", Some("Content Type"), "application/x-gifkit")?;
    set("GIFkit.Project", None, "GIFkit project")?;
    set("GIFkit.Project\\DefaultIcon", None, &format!("\"{exe_s}\",0"))?;
    set("GIFkit.Project\\shell\\open\\command", None, &format!("\"{exe_s}\" \"%1\""))?;
    let dll = exe.with_file_name("gifkit_thumbs.dll");
    if dll.exists() {
        set(&format!("CLSID\\{CLSID}"), None, "GIFkit project thumbnails")?;
        set(&format!("CLSID\\{CLSID}\\InprocServer32"), None, &dll.display().to_string())?;
        set(&format!("CLSID\\{CLSID}\\InprocServer32"), Some("ThreadingModel"), "Apartment")?;
        set(&format!(".gifkit\\ShellEx\\{THUMB_SLOT}"), None, CLSID)?;
        set(&format!("GIFkit.Project\\ShellEx\\{THUMB_SLOT}"), None, CLSID)?;
    }
    // Explorer reads associations again
    const SHCNE_ASSOCCHANGED: i32 = 0x0800_0000;
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, 0, std::ptr::null(), std::ptr::null()) };
    Ok(())
}
