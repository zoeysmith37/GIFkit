//! Ctrl+V, read straight from the system clipboard. egui on its own only
//! gets text (and on Wayland nothing at all for copied files), so a GIF
//! copied in a file manager or a browser would arrive as one still frame or
//! not at all. Here a file list comes in as files, and a picture copied in a
//! browser is fetched from where it lives so an animated GIF stays animated.

use eframe::egui;
use image::RgbaImage;
use std::path::PathBuf;

/// What a paste brought that isn't plain text.
pub enum Pasted {
    /// files copied in a file manager
    Files(Vec<PathBuf>),
    /// a picture copied in a browser: its address, and the still copy the
    /// browser put on the clipboard in case fetching it fails
    Url(String, Option<RgbaImage>),
}

/// The first `<img src="…">` in copied HTML.
fn img_src(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let mut from = 0;
    while let Some(i) = lower[from..].find("<img") {
        let tag_start = from + i;
        let tag_end = lower[tag_start..].find('>').map(|e| tag_start + e).unwrap_or(lower.len());
        let tag = &html[tag_start..tag_end];
        let tl = &lower[tag_start..tag_end];
        if let Some(s) = tl.find("src=") {
            let rest = &tag[s + 4..];
            let (q, body) = match rest.chars().next() {
                Some(c @ ('"' | '\'')) => (Some(c), &rest[1..]),
                _ => (None, rest),
            };
            let end = match q {
                Some(c) => body.find(c).unwrap_or(body.len()),
                None => body.find(|c: char| c.is_whitespace()).unwrap_or(body.len()),
            };
            let url = body[..end].replace("&amp;", "&");
            if !url.is_empty() {
                return Some(url);
            }
        }
        from = tag_end;
    }
    None
}

/// Read the clipboard for a Ctrl+V. Plain text goes back as an egui paste
/// event (text fields and pasted paths use it); anything else comes back as
/// `Pasted`. With a text field focused only text is read.
pub fn read(text_only: bool) -> (Vec<egui::Event>, Option<Pasted>) {
    let Ok(mut cb) = arboard::Clipboard::new() else { return (Vec::new(), None) };
    if !text_only {
        if let Ok(files) = cb.get().file_list() {
            let files: Vec<PathBuf> = files.into_iter().filter(|p| p.is_file()).collect();
            if !files.is_empty() {
                return (Vec::new(), Some(Pasted::Files(files)));
            }
        }
    }
    let image = if text_only {
        None
    } else {
        cb.get_image().ok().and_then(|i| RgbaImage::from_raw(i.width as u32, i.height as u32, i.bytes.into_owned()))
    };
    // a picture from a browser: fetch the file itself (it may be animated)
    if image.is_some() {
        if let Some(url) = cb.get().html().ok().and_then(|h| img_src(&h)) {
            if url.starts_with("http://") || url.starts_with("https://") {
                return (Vec::new(), Some(Pasted::Url(url, image)));
            }
            if let Some(p) = url.strip_prefix("file://") {
                let p = PathBuf::from(p);
                if p.is_file() {
                    return (Vec::new(), Some(Pasted::Files(vec![p])));
                }
            }
        }
    }
    if let Ok(t) = cb.get_text() {
        if !t.is_empty() {
            return (vec![egui::Event::Paste(t.replace("\r\n", "\n"))], None);
        }
    }
    match image {
        Some(img) => {
            let size = [img.width() as usize, img.height() as usize];
            let c = egui::ColorImage::from_rgba_unmultiplied(size, img.as_raw());
            (vec![egui::Event::PasteImage(std::sync::Arc::new(c))], None)
        }
        None => (Vec::new(), None),
    }
}

/// Fetch `url` to a temporary file with curl (Linux, macOS and Windows 10+
/// all ship it). The name keeps the file's own extension when it has one.
pub fn download(url: &str) -> anyhow::Result<PathBuf> {
    let name = url.split(['?', '#']).next().unwrap_or(url).rsplit('/').next().unwrap_or("pasted");
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).filter(|e| e.len() <= 5 && e.chars().all(|c| c.is_ascii_alphanumeric())).unwrap_or_else(|| "gif".into());
    let out = std::env::temp_dir().join(format!("gifkit-pasted-{}.{ext}", std::process::id()));
    let mut cmd = crate::io::hidden_cmd("curl");
    cmd.args(["-sSL", "--fail", "--max-time", "60", "-A", "Mozilla/5.0 GIFkit", "-o"]).arg(&out).arg(url);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let st = cmd.status().map_err(|e| anyhow::anyhow!("couldn't run curl to fetch the picture ({e})"))?;
    anyhow::ensure!(st.success() && out.metadata().map(|m| m.len() > 0).unwrap_or(false), "couldn't fetch {url}");
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_img_src() {
        assert_eq!(super::img_src(r#"<meta charset="utf-8"><img src="https://x.org/a.gif?x=1&amp;y=2" alt="">"#).as_deref(), Some("https://x.org/a.gif?x=1&y=2"));
        assert_eq!(super::img_src("<IMG alt='x' SRC='http://y/b.gif'>").as_deref(), Some("http://y/b.gif"));
        assert_eq!(super::img_src("<p>no pictures</p>"), None);
    }
}
