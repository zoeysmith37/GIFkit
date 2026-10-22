//! Finding ffmpeg, remembering where the user said it is, and on Windows
//! fetching a copy into GIFkit's own folder.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

const EXE: &str = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
const PROBE_EXE: &str = if cfg!(windows) { "ffprobe.exe" } else { "ffprobe" };

/// The setting holding a path the user picked.
pub const SETTING: &str = "ffmpeg_path";

/// Where a downloaded copy goes.
pub fn own_dir() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").or_else(|| std::env::var_os("APPDATA")).map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support"))
    } else {
        std::env::var_os("XDG_DATA_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
    }?;
    Some(base.join("gifkit").join("ffmpeg"))
}

/// Does this file run as ffmpeg?
pub fn works(p: &Path) -> bool {
    #[allow(unused_mut)]
    let mut cmd = std::process::Command::new(p);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.arg("-version").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}

/// Places ffmpeg often ends up that aren't always on the PATH.
fn usual_places() -> Vec<PathBuf> {
    let mut v = Vec::new();
    let env = |k: &str| std::env::var_os(k).map(PathBuf::from);
    if let Ok(me) = std::env::current_exe() {
        if let Some(d) = me.parent() {
            v.push(d.join(EXE));
            v.push(d.join("ffmpeg").join(EXE));
            v.push(d.join("ffmpeg").join("bin").join(EXE));
        }
    }
    if cfg!(windows) {
        if let Some(l) = env("LOCALAPPDATA") {
            v.push(l.join("Microsoft/WinGet/Links").join(EXE));
            // winget's own copy, in a folder named after the version
            let pk = l.join("Microsoft/WinGet/Packages");
            for e in std::fs::read_dir(&pk).into_iter().flatten().flatten() {
                if e.file_name().to_string_lossy().to_lowercase().contains("ffmpeg") {
                    for inner in std::fs::read_dir(e.path()).into_iter().flatten().flatten() {
                        v.push(inner.path().join("bin").join(EXE));
                    }
                    v.push(e.path().join("bin").join(EXE));
                }
            }
        }
        if let Some(h) = env("USERPROFILE") {
            v.push(h.join("scoop/shims").join(EXE));
            v.push(h.join("scoop/apps/ffmpeg/current/bin").join(EXE));
        }
        if let Some(pd) = env("ProgramData") {
            v.push(pd.join("chocolatey/bin").join(EXE));
        }
        for pf in [env("ProgramFiles"), env("ProgramFiles(x86)")].into_iter().flatten() {
            v.push(pf.join("ffmpeg/bin").join(EXE));
            v.push(pf.join("ffmpeg").join(EXE));
        }
        v.push(PathBuf::from(r"C:\ffmpeg\bin").join(EXE));
        v.push(PathBuf::from(r"C:\ffmpeg").join(EXE));
    } else {
        // apps started from a launcher may not get the shell's PATH
        for d in ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin", "/usr/bin", "/snap/bin"] {
            v.push(Path::new(d).join(EXE));
        }
    }
    v
}

/// Look for ffmpeg: where the user said, our own copy, the PATH, then the
/// usual install folders.
fn search() -> Option<PathBuf> {
    if let Some(p) = crate::theme::get_setting(SETTING).filter(|s| !s.is_empty()).map(PathBuf::from) {
        if works(&p) {
            return Some(p);
        }
    }
    if let Some(d) = own_dir() {
        let p = d.join(EXE);
        if p.is_file() && works(&p) {
            return Some(p);
        }
    }
    // testing hook: act as if it isn't installed
    if std::env::var_os("GIFKIT_FFMPEG_HIDE").is_some() {
        return None;
    }
    if works(Path::new("ffmpeg")) {
        return Some(PathBuf::from("ffmpeg"));
    }
    usual_places().into_iter().find(|p| p.is_file() && works(p))
}

static FOUND: RwLock<Option<Option<PathBuf>>> = RwLock::new(None);

/// ffmpeg's path, looked for once and kept until `recheck`.
pub fn path() -> Option<PathBuf> {
    if let Some(v) = FOUND.read().ok().and_then(|g| g.clone()) {
        return v;
    }
    let v = search();
    if let Ok(mut g) = FOUND.write() {
        *g = Some(v.clone());
    }
    v
}

/// Look again (after an install, a download or the user picking a file).
pub fn recheck() -> bool {
    if let Ok(mut g) = FOUND.write() {
        *g = None;
    }
    path().is_some()
}

/// What to run for `program`: ffmpeg and ffprobe from wherever ffmpeg was found.
pub fn program(name: &str) -> PathBuf {
    match name {
        "ffmpeg" => path().unwrap_or_else(|| PathBuf::from("ffmpeg")),
        "ffprobe" => path()
            .and_then(|p| p.parent().filter(|d| !d.as_os_str().is_empty()).map(|d| d.join(PROBE_EXE)))
            .filter(|p| p.is_file())
            .unwrap_or_else(|| PathBuf::from("ffprobe")),
        _ => PathBuf::from(name),
    }
}

/// The user picked a file: keep it if it runs as ffmpeg.
pub fn use_file(p: &Path) -> anyhow::Result<()> {
    anyhow::ensure!(works(p), "{} doesn't run as ffmpeg", p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
    crate::theme::set_setting(SETTING, &p.to_string_lossy());
    anyhow::ensure!(recheck(), "ffmpeg still couldn't be used");
    Ok(())
}

/// Can GIFkit fetch ffmpeg itself here? (Windows; the env var is for testing.)
pub fn can_download() -> bool {
    cfg!(windows) || std::env::var_os("GIFKIT_FFMPEG_DL").is_some()
}

fn arch() -> &'static str {
    if cfg!(target_arch = "aarch64") {
        "winarm64"
    } else {
        "win64"
    }
}

/// Builds with everything GIFkit uses (x264, VP9 and AV1), best first.
/// The shared ones are under half the download: ffmpeg and ffprobe share
/// their libraries. The "latest" release is rebuilt every day and its files
/// go missing for a while, so the current names come from GitHub first.
fn download_urls() -> Vec<String> {
    let mut v = Vec::new();
    if let Ok(o) = crate::io::hidden_cmd("curl")
        .args(["-sSL", "--max-time", "20", "-A", "GIFkit", "-H", "Accept: application/vnd.github+json", "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest"])
        .output()
    {
        if let Ok(j) = serde_json::from_slice::<serde_json::Value>(&o.stdout) {
            v.extend(pick_assets(&j, arch()));
        }
    }
    v.push(format!("https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-{}-gpl-shared.zip", arch()));
    v
}

/// From a release's file list: the shared GPL builds for `arch`, release
/// versions first (newest first), then the development one.
fn pick_assets(j: &serde_json::Value, arch: &str) -> Vec<String> {
    let tag = format!("-{arch}-gpl-shared");
    let mut found: Vec<(Vec<u32>, String)> = Vec::new();
    for a in j["assets"].as_array().into_iter().flatten() {
        let (Some(name), Some(url)) = (a["name"].as_str(), a["browser_download_url"].as_str()) else { continue };
        let Some(rest) = name.strip_suffix(".zip").and_then(|n| n.split_once(&tag)).map(|(_, r)| r) else { continue };
        let ver = match rest {
            // the development build
            "" => Vec::new(),
            r => match r.strip_prefix('-').map(|v| v.split('.').map(|x| x.parse::<u32>()).collect::<Result<Vec<_>, _>>()) {
                Some(Ok(v)) if !v.is_empty() => v,
                _ => continue,
            },
        };
        found.push((ver, url.to_string()));
    }
    // versions first, highest first; the development build (no version) last
    found.sort_by(|a, b| match (a.0.is_empty(), b.0.is_empty()) {
        (true, false) => std::cmp::Ordering::Greater,
        (false, true) => std::cmp::Ordering::Less,
        _ => b.0.cmp(&a.0),
    });
    found.into_iter().map(|f| f.1).collect()
}

/// How a download is going, shared with the window showing it.
#[derive(Default)]
pub struct Progress {
    pub got: AtomicU64,
    pub total: AtomicU64,
    /// set once the file is in and being unpacked
    pub unpacking: AtomicBool,
    pub cancel: AtomicBool,
}

/// Download ffmpeg into `own_dir()`, unpack ffmpeg and ffprobe, and check
/// they run. Blocks: call it from a thread.
pub fn download(pr: Arc<Progress>) -> anyhow::Result<PathBuf> {
    use anyhow::Context;
    let dir = own_dir().context("no folder to put ffmpeg in (LOCALAPPDATA isn't set)")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("couldn't make {}", dir.display()))?;
    let zip_path = dir.join("download.zip");
    let mut last = anyhow::anyhow!("no download found");
    let mut done = false;
    for url in download_urls() {
        match fetch(&url, &zip_path, &pr) {
            Ok(()) => {
                done = true;
                break;
            }
            Err(e) => {
                if pr.cancel.load(Ordering::Relaxed) {
                    return Err(e);
                }
                last = e;
            }
        }
    }
    if !done {
        return Err(last);
    }
    pr.unpacking.store(true, Ordering::Relaxed);
    let unpacked = unpack(&zip_path, &dir);
    let _ = std::fs::remove_file(&zip_path);
    unpacked?;
    let exe = dir.join(EXE);
    anyhow::ensure!(works(&exe), "ffmpeg was downloaded but doesn't run");
    recheck();
    Ok(exe)
}

/// Download `url` to `to` with curl, keeping `pr` up to date.
fn fetch(url: &str, to: &Path, pr: &Progress) -> anyhow::Result<()> {
    use anyhow::Context;
    pr.got.store(0, Ordering::Relaxed);
    pr.total.store(0, Ordering::Relaxed);
    // the size, for the progress bar (the last length after redirects)
    if let Ok(o) = crate::io::hidden_cmd("curl").args(["-sIL", "--max-time", "20", url]).output() {
        let head = String::from_utf8_lossy(&o.stdout);
        if let Some(n) = head.lines().filter_map(|l| l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<u64>().ok())).flatten().filter(|&n| n > 0).last() {
            pr.total.store(n, Ordering::Relaxed);
        }
    }
    let mut child = crate::io::hidden_cmd("curl")
        .args(["-sSL", "--fail", "--retry", "3", "--retry-delay", "2", "-A", "GIFkit", "-o"])
        .arg(to)
        .arg(url)
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("couldn't run curl to download ffmpeg")?;
    let status = loop {
        if pr.cancel.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(to);
            anyhow::bail!("Download cancelled");
        }
        if let Ok(m) = std::fs::metadata(to) {
            pr.got.store(m.len(), Ordering::Relaxed);
        }
        if let Some(s) = child.try_wait()? {
            break s;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    };
    if !status.success() {
        let mut err = String::new();
        if let Some(mut e) = child.stderr.take() {
            use std::io::Read;
            let _ = e.read_to_string(&mut err);
        }
        let _ = std::fs::remove_file(to);
        anyhow::bail!("The download failed{}", err.lines().last().map(|l| format!(": {l}")).unwrap_or_default());
    }
    Ok(())
}

/// Take the build's bin folder out of its zip: ffmpeg, ffprobe and the
/// libraries they use (not the ffplay player). An older copy goes first.
fn unpack(zip_path: &Path, dir: &Path) -> anyhow::Result<()> {
    use anyhow::Context;
    let f = std::fs::File::open(zip_path)?;
    let mut z = zip::ZipArchive::new(f).context("the download isn't a zip file")?;
    let wanted = |name: &str| -> Option<String> {
        let name = name.replace('\\', "/");
        let (folder, file) = name.rsplit_once('/')?;
        let keep = (folder == "bin" || folder.ends_with("/bin")) && !file.is_empty() && !file.to_ascii_lowercase().starts_with("ffplay") && !file.contains("..");
        keep.then(|| file.to_string())
    };
    let names: Vec<String> = (0..z.len()).filter_map(|i| z.by_index(i).ok().and_then(|e| wanted(e.name()))).collect();
    anyhow::ensure!(names.iter().any(|n| n.eq_ignore_ascii_case(EXE) || n.eq_ignore_ascii_case("ffmpeg.exe")), "ffmpeg wasn't in the download");
    // what an earlier download left (library names change between versions)
    for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let n = e.file_name().to_string_lossy().to_ascii_lowercase();
        if n.ends_with(".dll") || n.ends_with(".exe") || n == "ffmpeg" || n == "ffprobe" {
            let _ = std::fs::remove_file(e.path());
        }
    }
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let Some(file) = wanted(e.name()) else { continue };
        if e.is_dir() {
            continue;
        }
        let tmp = dir.join(format!("{file}.part"));
        let mut w = std::fs::File::create(&tmp)?;
        std::io::copy(&mut e, &mut w)?;
        drop(w);
        std::fs::rename(&tmp, dir.join(&file))?;
    }
    Ok(())
}

/// The command that installs ffmpeg here, to show (and copy).
pub fn install_hint() -> &'static str {
    if cfg!(windows) {
        "winget install Gyan.FFmpeg"
    } else if cfg!(target_os = "macos") {
        "brew install ffmpeg"
    } else {
        let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default().to_lowercase();
        let like = |s: &str| os.lines().any(|l| (l.starts_with("id=") || l.starts_with("id_like=")) && l.contains(s));
        if like("arch") {
            "sudo pacman -S ffmpeg"
        } else if like("fedora") || like("rhel") {
            "sudo dnf install ffmpeg"
        } else if like("suse") {
            "sudo zypper install ffmpeg"
        } else {
            "sudo apt install ffmpeg"
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn picks_builds() {
        let j: serde_json::Value = serde_json::from_str(r#"{"assets":[
            {"name":"ffmpeg-N-1-gabc-win64-gpl-shared.zip","browser_download_url":"u/master"},
            {"name":"ffmpeg-N-1-gabc-win64-gpl.zip","browser_download_url":"u/static"},
            {"name":"ffmpeg-n8.1.3-1-gabc-win64-gpl-shared-8.1.zip","browser_download_url":"u/8.1"},
            {"name":"ffmpeg-n9.0.2-33-g4cf-win64-gpl-shared-9.0.zip","browser_download_url":"u/9.0"},
            {"name":"ffmpeg-n9.0.2-33-g4cf-win64-lgpl-shared-9.0.zip","browser_download_url":"u/lgpl"},
            {"name":"ffmpeg-n9.0.2-33-g4cf-winarm64-gpl-shared-9.0.zip","browser_download_url":"u/arm"},
            {"name":"ffmpeg-N-1-gabc-linux64-gpl-shared.tar.xz","browser_download_url":"u/linux"}]}"#).unwrap();
        assert_eq!(super::pick_assets(&j, "win64"), ["u/9.0", "u/8.1", "u/master"]);
        assert_eq!(super::pick_assets(&j, "winarm64"), ["u/arm"]);
        assert!(super::pick_assets(&serde_json::json!({"message": "rate limited"}), "win64").is_empty());
    }

    #[test]
    fn unpacks_bin_only() {
        let dir = std::env::temp_dir().join(format!("gifkit-fftest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zp = dir.join("t.zip");
        {
            let mut w = zip::ZipWriter::new(std::fs::File::create(&zp).unwrap());
            let o: zip::write::SimpleFileOptions = Default::default();
            for (n, body) in [("ffmpeg-x/bin/".to_string() + super::EXE, "a"), ("ffmpeg-x/bin/".to_string() + super::PROBE_EXE, "b"), ("ffmpeg-x/doc/".to_string() + super::EXE, "c"), ("ffmpeg-x/bin/avcodec-1.dll".into(), "d"), ("ffmpeg-x/bin/ffplay.exe".into(), "e")] {
                w.start_file(n, o).unwrap();
                std::io::Write::write_all(&mut w, body.as_bytes()).unwrap();
            }
            w.finish().unwrap();
        }
        let out = dir.join("out");
        std::fs::create_dir_all(&out).unwrap();
        // an older version's library goes
        std::fs::write(out.join("avcodec-0.dll"), "old").unwrap();
        super::unpack(&zp, &out).unwrap();
        assert_eq!(std::fs::read_to_string(out.join(super::EXE)).unwrap(), "a");
        assert_eq!(std::fs::read_to_string(out.join(super::PROBE_EXE)).unwrap(), "b");
        assert_eq!(std::fs::read_to_string(out.join("avcodec-1.dll")).unwrap(), "d");
        assert!(!out.join("avcodec-0.dll").exists());
        assert!(!out.join("ffplay.exe").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The real download's layout (FFZIP=path to a downloaded build).
    #[test]
    #[ignore]
    fn unpacks_real_build() {
        let Some(z) = std::env::var_os("FFZIP") else { return };
        let out = std::env::temp_dir().join(format!("gifkit-ffreal-{}", std::process::id()));
        std::fs::create_dir_all(&out).unwrap();
        super::unpack(std::path::Path::new(&z), &out).unwrap();
        let mut n: Vec<String> = std::fs::read_dir(&out).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect();
        n.sort();
        eprintln!("{n:?}");
        let _ = std::fs::remove_dir_all(&out);
    }
}
