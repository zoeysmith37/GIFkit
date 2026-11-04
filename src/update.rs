//! New versions: asking GitHub for the latest release, then downloading and
//! installing it over this copy of GIFkit.
//!
//! Downloads go through the system's curl (on Windows 10 and up, macOS and
//! Linux), so proxies and certificates work as they do for everything else.

use anyhow::{anyhow, bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// This build's version: the release tag for release builds.
pub const VERSION: &str = env!("GIFKIT_VERSION");

const LATEST: &str = "https://api.github.com/repos/zoeysmith37/GIFkit/releases/latest";

#[derive(Clone, Debug)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub asset: Option<Asset>,
}

#[derive(Clone, Debug)]
pub struct Asset {
    pub url: String,
    pub size: u64,
    pub sha256: Option<String>,
}

/// The version this copy reports (GIFKIT_FAKE_VERSION pretends, for testing).
pub fn current() -> String {
    std::env::var("GIFKIT_FAKE_VERSION").unwrap_or_else(|_| VERSION.to_string())
}

fn parse(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim().trim_start_matches(['v', 'V']);
    let v = v.split(['-', '+']).next()?;
    let mut it = v.split('.').map(|p| p.parse::<u32>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0)))
}

/// Is `a` a later version than `b`?
pub fn newer(a: &str, b: &str) -> bool {
    match (parse(a), parse(b)) {
        (Some(a), Some(b)) => a > b,
        _ => false,
    }
}

/// The release file for this computer.
fn asset_name() -> Option<&'static str> {
    if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        Some(if appimage().is_some() { "gifkit-linux-x86_64.AppImage" } else { "gifkit-linux-x86_64.tar.gz" })
    } else if cfg!(all(windows, target_arch = "x86_64")) {
        Some("gifkit-windows-x86_64.zip")
    } else if cfg!(target_os = "macos") {
        Some("gifkit-macos-universal.zip")
    } else {
        None
    }
}

/// Running as an AppImage: the .AppImage file (the program itself sits in
/// a read-only mount while it runs).
fn appimage() -> Option<PathBuf> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    std::env::var_os("APPIMAGE").map(PathBuf::from).filter(|p| p.is_file())
}

fn curl() -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // the one in System32, never a curl.exe sitting in the current folder
        let sys = std::env::var_os("SystemRoot").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let mut c = Command::new(sys.join("System32").join("curl.exe"));
        // no console window flashing up
        c.creation_flags(0x0800_0000);
        c
    }
    #[cfg(not(windows))]
    Command::new("curl")
}

/// The latest release on GitHub.
pub fn check() -> Result<Release> {
    let out = curl()
        .args(["-fsSL", "--max-time", "20", "-H", "Accept: application/vnd.github+json", "-A"])
        .arg(format!("GIFkit/{VERSION}"))
        .arg(std::env::var("GIFKIT_UPDATE_URL").unwrap_or_else(|_| LATEST.to_string()))
        .stdin(std::process::Stdio::null())
        .output()
        .context("curl isn't available")?;
    if !out.status.success() {
        bail!("{}", curl_err(&String::from_utf8_lossy(&out.stderr)));
    }
    parse_release(&out.stdout)
}

/// curl's "curl: (6) Could not resolve host: x" as "Could not resolve host: x".
fn curl_err(e: &str) -> String {
    let l = e.lines().find(|l| !l.trim().is_empty()).unwrap_or("No connection").trim();
    let l = l.strip_prefix("curl:").unwrap_or(l).trim();
    let l = match l.strip_prefix('(') {
        Some(r) => r.split_once(')').map(|(_, t)| t.trim()).unwrap_or(r),
        None => l,
    };
    if l.is_empty() { "No connection".into() } else { l.to_string() }
}

fn parse_release(body: &[u8]) -> Result<Release> {
    let j: serde_json::Value = serde_json::from_slice(body).context("unexpected reply from GitHub")?;
    let tag = j["tag_name"].as_str().ok_or_else(|| anyhow!("unexpected reply from GitHub"))?;
    let asset = asset_name().and_then(|want| {
        j["assets"].as_array()?.iter().find(|a| a["name"].as_str() == Some(want)).map(|a| Asset {
            url: a["browser_download_url"].as_str().unwrap_or_default().to_string(),
            size: a["size"].as_u64().unwrap_or(0),
            sha256: a["digest"].as_str().and_then(|d| d.strip_prefix("sha256:")).map(|d| d.to_ascii_lowercase()),
        })
    });
    Ok(Release {
        version: tag.trim_start_matches(['v', 'V']).to_string(),
        notes: tidy_notes(j["body"].as_str().unwrap_or_default()),
        page: j["html_url"].as_str().unwrap_or("https://github.com/zoeysmith37/GIFkit/releases").to_string(),
        asset: asset.filter(|a| !a.url.is_empty()),
    })
}

/// Release notes as plain lines: no markdown marks, no "full changelog" link.
fn tidy_notes(s: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    for l in s.lines() {
        let l = l.trim_end();
        if l.contains("Full Changelog") || l.starts_with("<!--") {
            continue;
        }
        let t = l.trim_start();
        let line = if let Some(h) = t.strip_prefix('#') {
            h.trim_start_matches('#').trim().to_string()
        } else if let Some(b) = t.strip_prefix("- ").or_else(|| t.strip_prefix("* ")) {
            format!("•  {b}")
        } else {
            t.to_string()
        };
        let line = line.replace("**", "").replace('`', "");
        if line.is_empty() && out.last().is_none_or(|p| p.is_empty()) {
            continue;
        }
        out.push(line);
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop();
    }
    out.join("\n")
}

/// Where this copy is installed, and so what an update replaces.
#[derive(Clone, Debug)]
pub enum Target {
    /// the program file (Linux, Windows: its folder holds the rest)
    File(PathBuf),
    /// GIFkit.app (macOS)
    Bundle(PathBuf),
}

impl Target {
    /// The program to start after updating.
    pub fn program(&self) -> PathBuf {
        match self {
            Target::File(p) => p.clone(),
            Target::Bundle(b) => b.join("Contents/MacOS/gifkit"),
        }
    }
}

/// What an update would replace, or why this copy can't update itself.
pub fn target() -> Result<Target, String> {
    if asset_name().is_none() {
        return Err("No download for this kind of computer".into());
    }
    let exe = std::env::current_exe().and_then(|p| p.canonicalize()).map_err(|_| "Can't find where GIFkit is installed".to_string())?;
    let t = if cfg!(target_os = "macos") {
        let b = exe.ancestors().nth(3).filter(|b| b.extension().is_some_and(|e| e == "app")).ok_or("Not running from GIFkit.app")?;
        if b.to_string_lossy().contains("/AppTranslocation/") {
            return Err("Move GIFkit to Applications first".into());
        }
        Target::Bundle(b.to_path_buf())
    } else if let Some(img) = appimage() {
        Target::File(img)
    } else {
        Target::File(exe)
    };
    let dir = match &t {
        Target::File(p) | Target::Bundle(p) => p.parent().ok_or("Can't find where GIFkit is installed")?.to_path_buf(),
    };
    // installed by a package manager or for every user: theirs to update
    if !writable(&dir) {
        return Err("GIFkit's folder can't be written to".into());
    }
    Ok(t)
}

fn writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".gifkit-probe-{}", std::process::id()));
    let ok = std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// Progress of a download: bytes so far and in all.
#[derive(Default)]
pub struct Progress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub stop: AtomicBool,
}

/// Download `rel` and install it over `target`.
pub fn download_and_install(rel: &Release, target: &Target, p: &Arc<Progress>) -> Result<()> {
    let asset = rel.asset.as_ref().ok_or_else(|| anyhow!("No download for this kind of computer"))?;
    p.total.store(asset.size, Ordering::Relaxed);
    let tmp = std::env::temp_dir().join(format!("gifkit-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let r = (|| {
        let file = tmp.join(asset_name().unwrap_or("update"));
        download(&asset.url, &file, p)?;
        if let Some(want) = &asset.sha256 {
            if sha256_file(&file)? != *want {
                bail!("The download was damaged. Try again");
            }
        }
        install(&file, target, &rel.version)
    })();
    let _ = std::fs::remove_dir_all(&tmp);
    r
}

fn download(url: &str, to: &Path, p: &Progress) -> Result<()> {
    let mut child = curl()
        .args(["-fsSL", "--retry", "2", "--connect-timeout", "20", "-A"])
        .arg(format!("GIFkit/{VERSION}"))
        .arg("-o")
        .arg(to)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("curl isn't available")?;
    loop {
        if let Some(st) = child.try_wait()? {
            if !st.success() {
                let mut e = String::new();
                if let Some(mut s) = child.stderr.take() {
                    let _ = std::io::Read::read_to_string(&mut s, &mut e);
                }
                bail!("Download failed: {}", curl_err(&e));
            }
            break;
        }
        if p.stop.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!("Stopped");
        }
        p.done.store(std::fs::metadata(to).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    p.done.store(std::fs::metadata(to).map(|m| m.len()).unwrap_or(0), Ordering::Relaxed);
    Ok(())
}

fn sha256_file(path: &Path) -> Result<String> {
    use sha2::Digest;
    let mut h = sha2::Sha256::new();
    let mut f = std::fs::File::open(path)?;
    std::io::copy(&mut f, &mut h)?;
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Does `prog` start and say it's `version`? (A download for the wrong
/// machine, or a broken one, never replaces the copy that works.)
fn runs_as(prog: &Path, version: &str) -> Result<()> {
    let mut c = Command::new(prog);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(0x0800_0000);
    }
    let bad = || anyhow!("The new version doesn't start on this computer");
    let mut child = c.arg("--version").stdin(std::process::Stdio::null()).stdout(std::process::Stdio::piped()).stderr(std::process::Stdio::null()).spawn().map_err(|_| bad())?;
    // a version too old to know --version opens a window instead: not it
    let t0 = std::time::Instant::now();
    let st = loop {
        if let Some(st) = child.try_wait()? {
            break st;
        }
        if t0.elapsed() > std::time::Duration::from_secs(20) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(bad());
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    };
    let mut said = String::new();
    if let Some(mut o) = child.stdout.take() {
        let _ = std::io::Read::read_to_string(&mut o, &mut said);
    }
    if !st.success() || said.trim() != format!("GIFkit {version}") {
        return Err(bad());
    }
    Ok(())
}

/// `gifkit --install-update <archive>`: install a release file over this
/// copy (for testing the installer on each system).
pub fn install_file(archive: &Path) -> Result<()> {
    let t = target().map_err(|e| anyhow!(e))?;
    install(archive, &t, VERSION)
}

fn install(archive: &Path, target: &Target, version: &str) -> Result<()> {
    match target {
        Target::File(img) if archive.to_string_lossy().ends_with(".AppImage") => install_appimage(archive, img, version),
        Target::File(exe) => install_files(archive, exe, version),
        Target::Bundle(app) => install_bundle(archive, app, version),
    }
}

/// AppImage: the new file in place of the old one. Tried out from the
/// same folder first (/tmp may not allow running programs).
fn install_appimage(new: &Path, img: &Path, version: &str) -> Result<()> {
    let tmp = img.with_file_name(format!(".gifkit-update-{}.AppImage", std::process::id()));
    std::fs::copy(new, &tmp).with_context(|| format!("Couldn't write {}", img.display()))?;
    let r = (|| {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o755))?;
        }
        runs_as(&tmp, version)?;
        std::fs::rename(&tmp, img).with_context(|| format!("Couldn't replace {}", img.display()))
    })();
    let _ = std::fs::remove_file(&tmp);
    r
}

/// Linux and Windows: the release's files into the program's folder. The
/// running program is renamed out of the way (allowed while it runs) and
/// the new one put in its place.
fn install_files(archive: &Path, exe: &Path, version: &str) -> Result<()> {
    let dir = exe.parent().ok_or_else(|| anyhow!("no folder"))?;
    let stage = dir.join(format!(".gifkit-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let r = (|| {
        unpack(archive, &stage)?;
        let prog_name = if cfg!(windows) { "gifkit.exe" } else { "gifkit" };
        let new_prog = find(&stage, prog_name).ok_or_else(|| anyhow!("The download has no {prog_name} in it"))?;
        let from = new_prog.parent().unwrap().to_path_buf();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&new_prog, std::fs::Permissions::from_mode(0o755))?;
        }
        runs_as(&new_prog, version)?;
        // installed by install.sh: only the program goes in ~/.local/bin
        let local_bin = std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/bin/gifkit"));
        let by_script = !cfg!(windows) && local_bin.as_deref() == Some(exe);
        let mut names: Vec<std::ffi::OsString> = Vec::new();
        for e in std::fs::read_dir(&from)? {
            let e = e?;
            if e.file_type()?.is_file() && (!by_script || e.file_name() == prog_name) {
                names.push(e.file_name());
            }
        }
        // the program last, so a failure part way leaves it as it was
        names.sort_by_key(|n| n == prog_name);
        let mut done: Vec<(PathBuf, Option<PathBuf>, bool)> = Vec::new();
        for n in &names {
            let dest = if n == prog_name { exe.to_path_buf() } else { dir.join(n) };
            let existed = dest.exists();
            if let Err(e) = replace(&from.join(n), &dest).map(|old| done.push((dest.clone(), old, existed))) {
                // put back what was replaced (the program, last, is never among it)
                for (d, old, existed) in done.into_iter().rev() {
                    if let Some(o) = old {
                        let _ = std::fs::rename(&o, &d);
                    } else if !existed {
                        let _ = std::fs::remove_file(&d);
                    }
                }
                return Err(e);
            }
        }
        // the menu entry, file type and thumbnails as the new version has them
        if by_script {
            if let Some(script) = Some(from.join("install.sh")).filter(|s| s.is_file()) {
                let _ = Command::new("sh").arg(&script).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status();
            }
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

/// Put `src` at `dest`. What was there is renamed to `.old` first (Windows
/// won't let a running program or a loaded DLL be overwritten or deleted,
/// only renamed); returns that name.
fn replace(src: &Path, dest: &Path) -> Result<Option<PathBuf>> {
    let tmp = dest.with_file_name(format!(".{}.new", dest.file_name().unwrap().to_string_lossy()));
    std::fs::copy(src, &tmp).with_context(|| format!("Couldn't write {}", dest.display()))?;
    let old = if dest.exists() {
        if cfg!(windows) {
            let mut old = dest.with_file_name(format!("{}.old", dest.file_name().unwrap().to_string_lossy()));
            let mut k = 1;
            while old.exists() && std::fs::remove_file(&old).is_err() {
                old = dest.with_file_name(format!("{}.old{k}", dest.file_name().unwrap().to_string_lossy()));
                k += 1;
            }
            std::fs::rename(dest, &old).with_context(|| format!("Couldn't replace {}", dest.display()))?;
            Some(old)
        } else {
            None
        }
    } else {
        None
    };
    if let Err(e) = std::fs::rename(&tmp, dest) {
        if let Some(o) = &old {
            let _ = std::fs::rename(o, dest);
        }
        let _ = std::fs::remove_file(&tmp);
        return Err(anyhow!(e).context(format!("Couldn't replace {}", dest.display())));
    }
    Ok(old)
}

/// Windows: files an update left behind (in use until GIFkit restarted).
pub fn clean_up() {
    if !cfg!(windows) {
        return;
    }
    let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf)) else { return };
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            // "x.old" or "x.old2" next to the "x" that replaced it
            let Some((base, k)) = n.rsplit_once(".old") else { continue };
            if k.chars().all(|c| c.is_ascii_digit()) && dir.join(base).is_file() {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

/// macOS: the new GIFkit.app takes the old one's place.
fn install_bundle(archive: &Path, app: &Path, version: &str) -> Result<()> {
    let parent = app.parent().ok_or_else(|| anyhow!("no folder"))?;
    let stage = parent.join(format!(".gifkit-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    std::fs::create_dir_all(&stage)?;
    let r = (|| {
        unpack(archive, &stage)?;
        let new_app = stage.join("GIFkit.app");
        anyhow::ensure!(new_app.is_dir(), "The download has no GIFkit.app in it");
        runs_as(&new_app.join("Contents/MacOS/gifkit"), version)?;
        let old = stage.join("old.app");
        std::fs::rename(app, &old).context("Couldn't move the old GIFkit.app")?;
        if let Err(e) = std::fs::rename(&new_app, app) {
            let _ = std::fs::rename(&old, app);
            return Err(anyhow!(e).context("Couldn't put the new GIFkit.app in place"));
        }
        // Finder and Quick Look pick up the new copy
        let _ = Command::new("/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister").arg("-f").arg(app).status();
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&stage);
    r
}

fn unpack(archive: &Path, to: &Path) -> Result<()> {
    let name = archive.to_string_lossy();
    if name.ends_with(".tar.gz") {
        let st = Command::new("tar").arg("-xzf").arg(archive).arg("-C").arg(to).status().context("tar isn't available")?;
        anyhow::ensure!(st.success(), "Couldn't unpack the download");
        return Ok(());
    }
    if cfg!(target_os = "macos") {
        // ditto keeps the app's links, permissions and signature intact
        let st = Command::new("ditto").args(["-x", "-k"]).arg(archive).arg(to).status().context("ditto isn't available")?;
        anyhow::ensure!(st.success(), "Couldn't unpack the download");
        return Ok(());
    }
    let mut z = zip::ZipArchive::new(std::fs::File::open(archive)?).context("Couldn't unpack the download")?;
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        let Some(rel) = f.enclosed_name() else { continue };
        let out = to.join(rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut w = std::fs::File::create(&out)?;
        std::io::copy(&mut f, &mut w)?;
    }
    Ok(())
}

fn find(dir: &Path, name: &str) -> Option<PathBuf> {
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let p = e.path();
        if p.is_dir() {
            if let Some(f) = find(&p, name) {
                return Some(f);
            }
        } else if e.file_name() == name {
            return Some(p);
        }
    }
    None
}

/// Start the program at `prog` (opening `file`), for a restart.
pub fn relaunch(target: &Target, file: Option<&Path>) -> Result<()> {
    if let Target::Bundle(app) = target {
        let mut c = Command::new("open");
        c.arg("-n").arg(app);
        if let Some(f) = file {
            c.arg("--args").arg(f);
        }
        c.spawn()?;
        return Ok(());
    }
    let mut c = Command::new(target.program());
    if let Some(f) = file {
        c.arg(f);
    }
    c.stdin(std::process::Stdio::null());
    c.spawn()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions() {
        assert!(newer("1.6.1", "1.6.0"));
        assert!(newer("v2.0", "1.9.9"));
        assert!(newer("1.10.0", "1.9.0"));
        assert!(!newer("1.6.0", "1.6.0"));
        assert!(!newer("1.5.9", "1.6.0"));
        assert!(!newer("junk", "1.0.0"));
        assert!(newer("1.7.0-rc1", "1.6.0"));
    }

    #[test]
    fn release_reply() {
        let body = br###"{"tag_name":"v1.7.0","html_url":"https://x/r","body":"## What's new\n- **Updates** in the app\n* Faster `saving`\n\n\n**Full Changelog**: a...b",
            "assets":[{"name":"gifkit-linux-x86_64.tar.gz","size":10,"browser_download_url":"https://x/l","digest":"sha256:AB"},
                      {"name":"gifkit-windows-x86_64.zip","size":11,"browser_download_url":"https://x/w"},
                      {"name":"gifkit-macos-universal.zip","size":12,"browser_download_url":"https://x/m"}]}"###;
        let r = parse_release(body).unwrap();
        assert_eq!(r.version, "1.7.0");
        assert_eq!(r.notes, "What's new\n•  Updates in the app\n•  Faster saving");
        let a = r.asset.unwrap();
        if cfg!(target_os = "linux") {
            assert_eq!((a.url.as_str(), a.size, a.sha256.as_deref()), ("https://x/l", 10, Some("ab")));
        }
    }

    #[test]
    fn replace_keeps_or_restores() {
        let d = std::env::temp_dir().join(format!("gifkit-repl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("new"), b"new").unwrap();
        std::fs::write(d.join("prog"), b"old").unwrap();
        replace(&d.join("new"), &d.join("prog")).unwrap();
        assert_eq!(std::fs::read(d.join("prog")).unwrap(), b"new");
        // a second update over the first leaves no stray files (but .old on Windows)
        replace(&d.join("new"), &d.join("prog")).unwrap();
        let left: Vec<String> = std::fs::read_dir(&d).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).filter(|n| !n.ends_with(".old")).collect();
        assert_eq!(left.len(), 2, "{left:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}
