//! The Handbook: a short guide inside the app, there without a connection.
//! Lines starting "- " are list items and "## " starts a sub-heading.

use super::*;

pub(super) const TOPICS: &[(&str, &str)] = &[
    (
        "Getting Started",
        "Right then. Open a GIF or a video with Ctrl+O, or just drop it on the window. Videos come in through ffmpeg.

The tools live down the left rail. Pick one and its settings turn up on the right. They come in groups: looks, drawing, covering things up, adding pictures, the frame, time and output.

Nothing touches your frames until you export. Every edit is a layer or a frame edit, so you can change your mind as often as you like.

## Panels
- Number fields: drag across them to change, or click and type
- Sliders aren't fenced in: type past the end and the slider stretches to fit
- Shift while dragging is finer. Ctrl lands on round numbers
- A blue dot in a field means it's been changed. Right-click any number to reset it
- More opens the rest of a section, and stays open till you close it
- Effects you haven't switched on stay folded out of the way

## The views
- The preview shows the result as it'll look
- Split view (C) puts the original next to it
- The compressed view shows the exported file and how big it is

## Saving
- Export (Ctrl+E) writes your GIF, or whichever format you fancy
- Save (Ctrl+S) keeps a .gifkit project with every layer still editable
- Copy Result (Ctrl+Shift+C) pops the finished file on the clipboard, ready for a chat",
    ),
    (
        "Layers",
        "Adjust, Effects, Text, Shapes, Emoji, Markup, Censor, Image, Watermark and AI shield each add a layer.

They stack like in any image editor. Higher ones draw on top. Hide or lock them from the Layers panel or the timeline.

With no layer of a tool's kind picked, changing a setting adds one. With one picked, you're editing that one.

## Picking several
- Ctrl+click a layer to add it to the pick. Shift+click picks a whole run
- The last one you clicked is the active one, with the brighter highlight
- Settings show the active layer. Changes go to it alone
- Hold Alt while changing a setting to change it on every picked layer of that kind
- Drag any picked layer and the rest come along too

## Groups
@demo groups
- Ctrl+G tucks the picked layers into a group, a folder in the Layers panel
- Click a group's row for its own Opacity and Blend at the top of the panel
- Pass Through blends its layers one by one. Any other mode puts the group together first
- Drag layers into a group, or back out again
- A group's eye and lock look after all its layers in one go
- Drag a group's row to shift the whole lot
- Ctrl+Shift+G ungroups and keeps the layers

## Layer styles
@demo styles
- The fx button beside Blend opens Layer Style, laid out as Photoshop's
- Bevel and emboss, stroke, inner shadow and glow, satin, colour and gradient overlays, outer glow and drop shadow
- Blending Options has Fill: the layer's own pixels fade, its effects don't
- Text, shapes, emoji and image layers can all be styled
- Right-click a layer for Layer Style, or to copy, paste and clear one
- Each one has its own switch. They work on text, shapes and emoji

## The Layers panel
- Opacity and Blend sit at the top, for the active layer
- Ctrl+] and Ctrl+[ move a layer up or down. Add Shift for the top or bottom
- Clip shows a layer only where the layer under it has drawn
- Arrow keys nudge the picked layer 1 px, with the pointer over the picture
- Shift with an arrow moves it 10 px
- Mask shows it only inside shapes you draw
- Masking the GIF hides it outside the mask. Layers on top stay put
- In the Mask tool, Draw lets you draw a shape by hand. Esc stops

## Colours
Any colour button opens a picker. Under the number boxes sit your recent colours, and below those the main colours in the frame on screen. Handy.

## Image layers
- Replace Image swaps the picture and keeps the rest: place, size, motion and effects

## Text
- Drag text on the preview to put it where you want it
- Typing makes it appear letter by letter or word by word
- Fonts include the ones on your computer

## Shapes and Markup
- Drag on the preview to draw a shape. Lines and arrows go from start to end
- Shift keeps a line straight, in Shapes and in Markup

## Effects
- Blur, glitch, wobbles, styles, frames and more. Colour work lives in Adjust
- Older projects keep their colour settings under Older colour settings
- Transparency knocks out a colour. From Corner picks the background for you
- Only the background keeps that colour inside the subject, like a black shirt or outlines
- Clean edges makes edge pixels part see-through and takes the colour out of them, so there's no halo
- Soft edge and Remove colour spill tidy up a green screen

## Animation
Moves a layer over time with ready-made motions like slide, hop and spin. Keyframes on the timeline (K) give you finer control.",
    ),
    (
        "Adjustments",
        "The Adjust tool (A) makes adjustment layers, just like Photoshop. They change the colours of everything underneath and never lay a finger on the frames.

Add one from the Add Adjustment menu, Edit > Adjustments, the command palette or a shortcut. A layer holds a list of them, run top to bottom.

@demo curves

## Shortcuts
- Ctrl+L Levels and Ctrl+M Curves
- Ctrl+U Hue/Saturation and Ctrl+B Color Balance
- Ctrl+I Invert and Ctrl+Shift+U Desaturate
- Ctrl+Shift+L Auto Tone and Ctrl+Shift+B Auto Color

## Curves and Levels
- Both draw over the frame's histogram
- Curves: drag a point, click the line for another, drag it off to bin it
- Levels: drag the black, grey and white handles, or let Auto have a go
- The eyedroppers set the black, grey or white point. Click them on the Original view
- Color Balance starts on All Tones, one shift for the whole picture
- Pick Shadows, Midtones or Highlights in Color Balance for finer control

## Your presets
The bookmark on each adjustment's card saves its settings by name. They're there for that adjustment in every project after.

## Every adjustment
- Levels, Curves, Exposure, Brightness/Contrast, Vibrance and Hue/Saturation
- Color Balance, Black & White, Photo Filter, Channel Mixer and Selective Color
- Invert, Posterize, Threshold and Gradient Map
- Looks are ready-made grades. Color Lookup loads .cube files from video editors

## Before and after
Hold \\ to see the frame without its adjustment and effects layers. Let go and they're back.",
    ),
    (
        "Blend Modes",
        "Every layer has a blend mode in the Layers panel, next to its opacity. It decides how the layer gets on with what's underneath.

## Which to use
- Multiply darkens. White in the layer vanishes
- Screen lightens. Black in the layer vanishes
- Overlay and Soft Light add a bit of punch
- Color takes only the layer's colour. Luminosity takes only its light
- Difference shows what's changed between two layers

## With adjustments
Curves in Luminosity adds contrast without making colours garish. Hue/Saturation in Color tints without touching the light.",
    ),
    (
        "Shortcuts and Palette",
        "Press Ctrl+K for the command palette. Type a bit of any tool, adjustment, command or recent file and hit Enter. Bob's your uncle.

@demo palette

Each entry shows its shortcut, so you'll pick them up as you go.

## Tools
- T Text, U Shapes, B Markup and E Emoji
- Q Mask, A Adjust and G Effects
- Space plays. The arrow keys step a frame

## Your own keys
Settings > Keyboard Shortcuts lets you change any of them.",
    ),
    (
        "Rulers and Guides",
        "View > Rulers (Ctrl+Shift+R) puts rulers along the top and left of each view, in that view's own pixels. They're off until you want them.

@demo guides

## Guides
- Drag from a ruler onto the picture to make a guide
- Drag a guide back onto its ruler to get rid of it
- Layers snap to guides as you move them
- Guides belong to the view they're made in: the original or the result
- A guide pulls towards the frame's edges and middle. Hold Ctrl to place it freely
- View > Show Guides (Ctrl+;) hides them. View > Clear Guides clears the lot
- Guides are saved with the project

## Grid
View > Grid (Ctrl+') lays a grid over the picture. Grid Size sets the spacing. Snap to Grid lines up whatever you move or resize.

## Smart guides
While you move or resize a layer, pink lines show where it lines up with the frame and other layers, and it snaps there. Pink numbers give the gap to the nearest layer on each side.
- View > Smart Guides turns them off
- Hold Ctrl while dragging to move freely, no snapping",
    ),
    (
        "Timeline",
        "The timeline sits under the preview. The GIF gets its own row and so does each layer.

## Clips
- Drag a clip to move it, and its edges to trim it
- Trimmed frames are kept. Pull the edge back out to get them back
- S splits at the playhead and Delete removes a clip
- Those two keys work with the pointer over the timeline
- Right-click a GIF clip for Clip Speed and transitions

## Speed
- The Speed tool speeds up or slows down parts of the GIF
- Click the GIF track to add a point. Drag it up for faster
- Double-click a point to put it back to 100%. Right-click for exact values
- Clip Speed does one clip the same way

## Selecting
- Drag a box on empty space to select clips and keyframes
- Ctrl or Shift adds to the selection and Alt takes away

## Playing
- Space plays and the arrow keys step a frame
- I and O set in and out points. Export can save just that bit
- New markers ask for a name straight away. Click a marker's name to change it
- Trim to Last Clip chops off the empty end
- Shift+O shows onion skin: frames before in red, the next in green

## Moving layers
- K adds a keyframe. Auto key adds one whenever you change a value
- The easing list also sets what new keyframes start as
- Hold Alt while changing a keyframe to change every selected one
- The keyframe controls grey out while playing
- Ease Custom opens a curve to shape the timing. Back overshoots
- Draw Path: drag on the preview and the layer follows the line
- With keyframes, dragging a layer on the preview keys it at the playhead
- Track: click something (or box it) and the layer follows it
- Pick it on more frames where it changes. Then press Track again or Enter
- The picked layer's path shows as dots, one a frame",
    ),
    (
        "Frame Edits",
        "Resize, Crop, Rotate, Reverse, Speed, Cut and Frames change the whole animation.

## Loops
- Reverse holds Boomerang and the Seamless Loop tools
- Find Loop Point hunts for the two frames that match best
- Blend the Seam fades the end into the start

## Crop
Drag on the preview to set the crop. Preview picks whether you see it with your edits or as the original.

## Speed and frames
- Speed changes the delay of every frame
- Change Frame Rate makes more or fewer frames for the same length
- Layers and keyframes stay in time when the frame rate changes
- Frames skips the frames you don't want and adds images as new ones
- Repeated Frames finds frames that copy the one before and merges them
- Tolerance sets how alike they must be. The button counts what it'll find
- Deleting frames moves keyframes, clips, markers and in and out points along with them

## In-betweens
Frames > In-Betweens adds new frames between each pair for smoother motion.
- Follow motion moves things along instead of fading them
- Method AI uses RIFE, a network made for in-betweens. Sharp even on tricky moves
- Method Tracker uses GIFkit's own tracking. Unsure sets what it does when lost
- Burned-in text and logos stay put. A cut holds the nearer frame
- The length stays the same unless frames would drop under 2 cs",
    ),
    (
        "Export and Size",
        "Pick the format in Export. GIF, WebP, AVIF, APNG, MP4, WebM, single frames and sprite sheets are all there.

## Hitting a size
- Set a limit in Optimize and press Find Settings
- Quality goes first. The frames only shrink if they really must
- Keep frame size stops it shrinking them at all
- Platforms sets the limit for Discord, Slack, Twitch and others
- Emoji sizes shrink the frames too
- Near matches merges frames that barely change. Grand for screen recordings

## Cutting the end
Cut empty end frames drops blank frames at the end when exporting.",
    ),
    (
        "Presets and Batch",
        "A preset keeps frame edits, layers and compression to use again on another file. Make one in the Presets tool.

## Batch
File > Batch puts the same edits on loads of files at once.
- Edits can be this file's edits, a saved preset or none at all
- For picks a platform limit to fit every file under
- Files are saved next to the originals unless you choose a folder
- Nothing gets overwritten. Names get a number when they're taken",
    ),
    (
        "Memory and Speed",
        "GIFkit keeps frames packed up in memory and unpacks what it needs, when it needs it.

## Memory Use
Settings > Memory Use picks how much it keeps ready.
- Low saves memory. Scrubbing and undo do a bit more work
- High keeps more ready and gives you more steps of undo
- The status bar shows how much memory is in use

## Preview
- Preview Resolution keeps big files quick to edit
- Hardware Acceleration runs effects on the graphics card

## Autosave
Unsaved work is tucked away every minute. If it all goes pear-shaped, GIFkit offers it back next time.",
    ),
];

/// Little recordings of GIFkit doing things, shown in the topics with
/// "@demo name".
const DEMOS: &[(&str, &[u8])] = &[
    ("guides", include_bytes!("../assets/handbook/guides.gif")),
    ("groups", include_bytes!("../assets/handbook/groups.gif")),
    ("styles", include_bytes!("../assets/handbook/styles.gif")),
    ("curves", include_bytes!("../assets/handbook/curves.gif")),
    ("palette", include_bytes!("../assets/handbook/palette.gif")),
];

/// A demo playing: its frames read one at a time into a single picture,
/// so a long recording never sits in memory whole.
struct Player {
    frames: image::Frames<'static>,
    tex: Option<egui::TextureHandle>,
    size: Vec2,
    next: f64,
}

thread_local! {
    static PLAYERS: std::cell::RefCell<std::collections::HashMap<&'static str, Player>> = Default::default();
}

fn open_demo(bytes: &'static [u8]) -> Option<image::Frames<'static>> {
    use image::AnimationDecoder;
    let d = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes)).ok()?;
    Some(d.into_frames())
}

/// Shows demo `name`, playing and looping, as wide as fits.
fn demo(ui: &mut egui::Ui, name: &str) {
    let Some(&(key, bytes)) = DEMOS.iter().find(|d| d.0 == name) else { return };
    let now = ui.input(|i| i.time);
    let ctx = ui.ctx().clone();
    let shown = PLAYERS.with(|ps| {
        let mut ps = ps.borrow_mut();
        if !ps.contains_key(key) {
            let frames = open_demo(bytes)?;
            ps.insert(key, Player { frames, tex: None, size: Vec2::ZERO, next: 0.0 });
        }
        let pl = ps.get_mut(key)?;
        if now >= pl.next || pl.tex.is_none() {
            let f = match pl.frames.next() {
                Some(Ok(f)) => f,
                // the end: round again
                _ => {
                    pl.frames = open_demo(bytes)?;
                    pl.frames.next()?.ok()?
                }
            };
            let (n, d) = f.delay().numer_denom_ms();
            let ms = (n as f64 / d.max(1) as f64).max(20.0);
            let img = f.into_buffer();
            let ci = egui::ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw());
            pl.size = vec2(img.width() as f32, img.height() as f32);
            match &mut pl.tex {
                Some(t) => t.set(ci, egui::TextureOptions::LINEAR),
                None => pl.tex = Some(ctx.load_texture(format!("demo-{key}"), ci, egui::TextureOptions::LINEAR)),
            }
            pl.next = now + ms / 1000.0;
        }
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((pl.next - now).max(0.01)));
        Some((pl.tex.clone()?, pl.size))
    });
    if let Some((tex, size)) = shown {
        let w = ui.available_width().min(size.x);
        let r = ui.add(egui::Image::new((tex.id(), vec2(w, w * size.y / size.x))).corner_radius(4.0));
        ui.painter().rect_stroke(r.rect, 4.0, Stroke::new(1.0, p().border), egui::StrokeKind::Outside);
    }
}

/// Draw one topic's text.
fn body(ui: &mut egui::Ui, text: &str) {
    let pal = *p();
    ui.spacing_mut().item_spacing.y = 6.0;
    for block in text.split("\n\n") {
        for line in block.lines() {
            if let Some(name) = line.strip_prefix("@demo ") {
                ui.add_space(2.0);
                demo(ui, name.trim());
                ui.add_space(2.0);
            } else if let Some(h) = line.strip_prefix("## ") {
                ui.add_space(4.0);
                ui.label(RichText::new(h).font(theme::medium(14.0)).color(pal.fg));
            } else if let Some(item) = line.strip_prefix("- ") {
                ui.horizontal_wrapped(|ui| {
                    ui.add_space(6.0);
                    ui.label(RichText::new("•").color(pal.fg_muted));
                    ui.label(item);
                });
            } else {
                ui.add(egui::Label::new(line).wrap());
            }
        }
        ui.add_space(8.0);
    }
}

impl GifKitApp {
    pub(super) fn handbook_window(&mut self, ctx: &egui::Context) {
        if !self.handbook_open {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut topic = self.handbook_topic.min(TOPICS.len() - 1);
        dialog(ctx, "handbook", "GIFkit Handbook", vec2(760.0, 540.0), true, &mut open, |ui| {
            ui.horizontal_top(|ui| {
                // topics down the side, as KDE's help centre has them
                ui.allocate_ui_with_layout(vec2(180.0, ui.available_height()), egui::Layout::top_down_justified(egui::Align::LEFT), |ui| {
                    ui.spacing_mut().item_spacing.y = 2.0;
                    for (i, (t, _)) in TOPICS.iter().enumerate() {
                        let on = i == topic;
                        let r = ui.add(egui::Button::new(RichText::new(*t).color(pal.fg)).min_size(vec2(180.0, 30.0)).fill(if on { mix(pal.window, pal.highlight, 0.3) } else { Color32::TRANSPARENT }).stroke(if on { Stroke::new(1.0, pal.highlight) } else { Stroke::NONE }));
                        if r.clicked() {
                            topic = i;
                        }
                    }
                });
                ui.separator();
                ui.vertical(|ui| {
                    egui::ScrollArea::vertical().id_salt(("handbook", topic)).auto_shrink([false, false]).show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(560.0));
                        ui.label(RichText::new(TOPICS[topic].0).font(theme::medium(18.0)).color(pal.fg));
                        ui.add_space(8.0);
                        body(ui, TOPICS[topic].1);
                    });
                });
            });
        });
        self.handbook_topic = topic;
        if !open {
            self.handbook_open = false;
        }
    }
}

/// What's new in this version: a headline, a line about it, and the
/// Handbook topic that explains it.
const WHATS_NEW: &[(&str, &str, &str)] = &[
    ("Adjustment layers", "Levels, Curves and fifteen more, like Photoshop's. Press A or Ctrl+L", "Adjustments"),
    ("Layer groups and styles", "Ctrl+G tucks layers in a folder. The fx button adds stroke, shadow and glow", "Layers"),
    ("Pick several layers", "Ctrl+click or Shift+click in the Layers panel. Hold Alt to change them all", "Layers"),
    ("Blend modes", "Multiply, Screen, Overlay and the rest on every layer", "Blend Modes"),
    ("Command palette", "Ctrl+K finds any tool, adjustment or command by name", "Shortcuts and Palette"),
    ("Rulers, grid and smart guides", "All in View. Layers snap to guides, the grid, the middle and each other", "Rulers and Guides"),
    ("Before and after", "Hold \\ to see the frame without its adjustments", "Adjustments"),
];

/// Show What's New now? After an update (not on a first run), unless
/// turned off in Settings. Remembers the version so it shows once.
pub(super) fn whats_new_due() -> bool {
    let now = crate::update::current();
    let seen = theme::get_setting("seen_version");
    theme::set_setting("seen_version", &now);
    if theme::get_setting("whats_new").as_deref() == Some("0") || std::env::var_os("GIFKIT_NO_WHATS_NEW").is_some() {
        return false;
    }
    match seen {
        Some(s) => crate::update::newer(&now, &s),
        // a copy from before this setting existed has used GIFkit before
        None => theme::get_setting("recent").is_some_and(|r| r.len() > 2),
    }
}

impl GifKitApp {
    pub(super) fn whats_new_window(&mut self, ctx: &egui::Context) {
        if !self.whats_new_open {
            return;
        }
        let pal = *p();
        let mut open = true;
        let mut go: Option<&str> = None;
        let was = theme::get_setting("whats_new").as_deref() != Some("0");
        let mut show_again = was;
        let title = format!("What's New in GIFkit {}", crate::update::current());
        dialog(ctx, "whats-new", &title, vec2(520.0, 470.0), false, &mut open, |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            for (head, line, topic) in WHATS_NEW {
                let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 52.0), Sense::click());
                if resp.hovered() {
                    ui.painter().rect_filled(r, 4.0, mix(pal.window, pal.highlight, 0.12));
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                ui.painter().circle_filled(r.left_center() + vec2(12.0, 0.0), 4.0, pal.highlight);
                ui.painter().text(r.left_top() + vec2(26.0, 8.0), egui::Align2::LEFT_TOP, *head, theme::medium(14.0), pal.fg);
                ui.painter().text(r.left_top() + vec2(26.0, 29.0), egui::Align2::LEFT_TOP, *line, egui::FontId::proportional(12.5), pal.fg_muted);
                if resp.on_hover_text("Read about it in the Handbook").clicked() {
                    go = Some(topic);
                }
            }
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                check(ui, &mut show_again, "Show this after updates");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.add(egui::Button::new("Close").min_size(vec2(84.0, CHIP_H))).clicked() {
                        self.whats_new_open = false;
                    }
                    if ui.add(egui::Button::new("Open Handbook").min_size(vec2(0.0, CHIP_H))).clicked() {
                        go = Some(WHATS_NEW[0].2);
                    }
                });
            });
        });
        if show_again != was {
            theme::set_setting("whats_new", if show_again { "1" } else { "0" });
        }
        if let Some(t) = go {
            self.handbook_topic = TOPICS.iter().position(|x| x.0 == t).unwrap_or(0);
            self.handbook_open = true;
            self.whats_new_open = false;
        }
        if !open {
            self.whats_new_open = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whats_new_points_at_real_topics() {
        for (head, _, topic) in WHATS_NEW {
            assert!(TOPICS.iter().any(|t| t.0 == *topic), "{head}: no topic {topic}");
        }
    }

    #[test]
    fn handbook_text_is_tidy() {
        for (t, text) in TOPICS {
            assert!(!text.trim().is_empty(), "{t}");
            for line in text.lines() {
                assert_eq!(line, line.trim_end(), "{t}: trailing space");
                if let Some(name) = line.strip_prefix("@demo ") {
                    assert!(DEMOS.iter().any(|d| d.0 == name.trim()), "{t}: no demo {name}");
                }
                // list items stay short: one thought each
                if let Some(item) = line.strip_prefix("- ") {
                    assert!(item.split_whitespace().count() <= 16, "{t}: {item}");
                }
            }
        }
    }
}
