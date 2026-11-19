//! The Handbook: a short guide inside the app, there without a connection.
//! Lines starting "- " are list items and "## " starts a sub-heading.

use super::*;

pub(super) const TOPICS: &[(&str, &str)] = &[
    (
        "Getting Started",
        "Open a GIF or a video with Ctrl+O or drop it on the window. Videos open through ffmpeg.

The left rail holds the tools. Pick one and its settings show on the right.

Nothing you do changes the frames until you export. Every edit is a layer or a frame edit you can change or undo later.

## The views
- The preview shows the result as it will look
- Split view (C) puts the original beside it
- The compressed view shows the exported file and its size

## Saving
- Export (Ctrl+E) writes the GIF or other format
- Save (Ctrl+S) keeps a .gifkit project with every layer still editable
- Copy Result (Ctrl+Shift+C) puts the finished file on the clipboard to paste into a chat",
    ),
    (
        "Layers",
        "Adjust, Effects, Text, Shapes, Emoji, Markup, Censor, Image, Watermark and AI shield each add a layer.

Layers stack like in an image editor. Higher ones draw on top. Hide or lock them from the timeline.

With no layer of a tool's kind selected, changing a setting adds one. With one selected, you edit that one.

## Groups
- Ctrl+G puts the selected layer in a new group, a folder in the Layers panel
- Drag layers into a group, or out of it
- A group's eye and lock hide or lock all of its layers
- Drag a group's row to move all of its layers
- Ctrl+Shift+G ungroups and keeps the layers

## The Layers panel
- Opacity and Blend sit at the top, for the selected layer
- Ctrl+] and Ctrl+[ move a layer up or down. Add Shift for the top or bottom
- Clip shows a layer only where the layer under it has drawn
- The fx button beside Blend adds a stroke, drop shadow or outer glow
- Layer styles work on text, shapes and emoji
- Arrow keys nudge the selected layer 1 px, with the pointer on the picture
- Shift with an arrow moves it 10 px
- Mask shows it only inside shapes you draw
- Masking the GIF hides it outside the mask. Layers on top stay
- In the Mask tool, Draw lets you draw a shape by hand. Esc stops

## Colours
Any colour button opens a picker. Under its number boxes are your recent colours, and below them the main colours of the frame on screen.

## Text
- Drag text on the preview to place it
- Typing makes it appear letter by letter or word by word
- Fonts include the ones on your system

## Shapes and Markup
- Drag on the preview to draw a shape. Lines and arrows go from start to end
- Shift keeps a line straight, in Shapes and in Markup

## Effects
- Blur, glitch, distortions, styles, frames and more. Colour work lives in Adjust
- Older projects keep their colour settings under Older colour settings
- Transparency removes a colour. From Corner picks the background for you
- Soft edge and Remove colour spill clean up a green screen

## Animation
Moves a layer over time with ready-made motions like slide, hop and spin. Keyframes on the timeline (K) give finer control.",
    ),
    (
        "Adjustments",
        "The Adjust tool (A) makes adjustment layers, as in Photoshop. They change the colours of everything under them and never touch the frames.

Add one from the Add Adjustment menu, Edit > Adjustments, the command palette or a shortcut. A layer holds a list of them that runs top to bottom.

## Shortcuts
- Ctrl+L Levels and Ctrl+M Curves
- Ctrl+U Hue/Saturation and Ctrl+B Color Balance
- Ctrl+I Invert and Ctrl+Shift+U Desaturate
- Ctrl+Shift+L Auto Tone and Ctrl+Shift+B Auto Color

## Curves and Levels
- Both draw over the frame's histogram
- Curves: drag a point, click the line to add one, drag it off to remove it
- Levels: drag the black, grey and white handles, or use Auto
- The eyedroppers set the black, grey or white point. Click them on the Original view
- Color Balance starts on All Tones, one shift for the whole picture
- Pick Shadows, Midtones or Highlights in Color Balance for finer control

## Your presets
The bookmark on each adjustment's card saves its settings by name. They're there for that adjustment in every project.

## Every adjustment
- Levels, Curves, Exposure, Brightness/Contrast, Vibrance and Hue/Saturation
- Color Balance, Black & White, Photo Filter, Channel Mixer and Selective Color
- Invert, Posterize, Threshold and Gradient Map
- Looks are ready-made grades. Color Lookup loads .cube files from video editors

## Comparing
Hold \\ to see the frame without its adjustment and effects layers. Let go to see them again.",
    ),
    (
        "Blend Modes",
        "Every layer has a blend mode in the Layers panel, beside its opacity. It decides how the layer meets what's under it.

## Which to use
- Multiply darkens. White in the layer disappears
- Screen lightens. Black in the layer disappears
- Overlay and Soft Light add contrast
- Color takes only the layer's colour. Luminosity takes only its light
- Difference shows what's changed between two layers

## With adjustments
Curves in Luminosity adds contrast without making colours stronger. Hue/Saturation in Color tints without changing light.",
    ),
    (
        "Shortcuts and Palette",
        "Press Ctrl+K for the command palette. Type part of any tool, adjustment, command or recent file and press Enter.

Each entry shows its shortcut, so you learn them as you go.

## Tools
- T Text, U Shapes, B Markup and E Emoji
- Q Mask, A Adjust and G Effects
- Space plays. The arrow keys step a frame

## Your own keys
Settings > Keyboard Shortcuts changes any of them.",
    ),
    (
        "Rulers and Guides",
        "View > Rulers (Ctrl+Shift+R) shows rulers along the top and left of the preview, in pixels. They are off until you turn them on.

## Guides
- Drag from a ruler onto the picture to make a guide
- Drag a guide back onto its ruler to remove it
- Layers snap to guides as you move them
- Guides are saved with the project
- A guide pulls toward the frame's edges and middle. Hold Ctrl to place it freely
- View > Show Guides (Ctrl+;) hides them, and View > Clear Guides removes them all

## Grid
View > Grid (Ctrl+') lays a grid over the picture. Grid Size sets its spacing. Snap to Grid makes things you move or resize line up with it.

## Smart guides
While you move or resize a layer, lines show where it lines up with the frame and other layers. It snaps there too. Pink numbers show the gap to the nearest layer on each side.
- View > Smart Guides turns them off
- Hold Ctrl while dragging to move freely without snapping",
    ),
    (
        "Timeline",
        "The timeline sits under the preview. The GIF has its own row and each layer gets one too.

## Clips
- Drag a clip to move it and its edges to trim it
- Trimmed frames are kept. Pull the edge back out to get them back
- S splits at the playhead and Delete removes a clip
- Those two keys work with the pointer on the timeline
- Right-click a GIF clip for Clip Speed and transitions

## Speed
- The Speed tool changes the speed of parts of the GIF
- Click the GIF track to add a point. Drag it up for faster
- Double-click a point to set it back to 100%. Right-click for exact values
- Clip Speed sets one clip's speed the same way

## Selecting
- Drag a box on empty space to select clips and keyframes
- Ctrl or Shift adds to the selection and Alt takes away

## Playing
- Space plays and the arrow keys step a frame
- I and O set in and out points. Export can save just that part
- Trim to Last Clip cuts the empty end off
- Shift+O shows onion skin: frames before in red, the next in green

## Moving layers
- K adds a keyframe. Auto key adds one when you change a value
- Ease Custom opens a curve to shape the timing. Back overshoots
- Draw Path: drag on the preview and the layer follows the line
- Once a layer has keyframes, dragging it on the preview keys it at the playhead
- Track: click something (or box it) and the layer follows it
- Pick it on more frames where it changes. Then press Track again or Enter
- The selected layer's path shows as dots, one a frame",
    ),
    (
        "Frame Edits",
        "Resize, Crop, Rotate, Reverse, Speed, Cut and Frames change the whole animation.

## Loops
- Reverse holds Boomerang and the Seamless Loop tools
- Find Loop Point looks for the two frames that match best
- Blend the Seam fades the end into the start

## Crop
Drag on the preview to set the crop. Preview picks whether you see it with your edits or as the original.

## Speed and frames
- Speed changes the delay of every frame
- Change Frame Rate makes more or fewer frames for the same length
- Layers and keyframes stay in time when the frame rate changes
- Frames skips frames you don't want and adds images as new frames

## In-betweens
Frames > In-Betweens adds new frames between each pair for smoother motion.
- Follow motion moves things along instead of fading them
- Method AI uses RIFE, a network made for in-betweens. Sharp even on hard moves
- Method Tracker uses GIFkit's own tracking. Unsure sets what it does where it can't follow
- Burned-in text and logos stay put. A cut holds the nearer frame
- The length stays the same unless frames would get under 2 cs",
    ),
    (
        "Export and Size",
        "Pick the format in Export. GIF, WebP, AVIF, APNG, MP4, WebM, single frames and sprite sheets are there.

## Hitting a size
- Set a limit in Optimize and press Find Settings
- Quality drops first. The frames shrink only if they must
- Keep frame size stops it from shrinking them
- Platforms sets the limit for Discord, Slack, Twitch and others. Emoji sizes shrink the frames too
- Near matches merges frames that barely change. Good for screen recordings

## Cutting the end
Cut empty end frames drops blank frames at the end when exporting.",
    ),
    (
        "Presets and Batch",
        "A preset keeps frame edits, layers and compression to use again on another file. Make one in the Presets tool.

## Batch
File > Batch puts the same edits on many files at once.
- Edits can be this file's edits, a saved preset or none
- For picks a platform limit to fit every file under
- Files are saved next to the originals unless you choose a folder
- Nothing gets overwritten. Names get a number when taken",
    ),
    (
        "Memory and Speed",
        "GIFkit keeps frames packed in memory and unpacks what it needs.

## Memory Use
Settings > Memory Use picks how much it keeps ready.
- Low saves memory. Scrubbing and undo do more work
- High keeps more ready and gives more steps to undo
- The status bar shows how much memory is in use

## Preview
- Preview Resolution keeps big files quick to edit
- Hardware Acceleration runs effects on the graphics card

## Autosave
Unsaved work is kept every minute. After a crash GIFkit offers it back.",
    ),
];

/// Draw one topic's text.
fn body(ui: &mut egui::Ui, text: &str) {
    let pal = *p();
    ui.spacing_mut().item_spacing.y = 6.0;
    for block in text.split("\n\n") {
        for line in block.lines() {
            if let Some(h) = line.strip_prefix("## ") {
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
    ("Adjustment layers", "Levels, Curves and 15 more, like Photoshop's. Press A or Ctrl+L", "Adjustments"),
    ("Blend modes", "Multiply, Screen, Overlay and the rest on every layer, in the Layers panel", "Blend Modes"),
    ("Command palette", "Ctrl+K finds any tool, adjustment or command by name", "Shortcuts and Palette"),
    ("Layer groups and styles", "Ctrl+G puts layers in a folder. The fx button adds stroke, shadow and glow", "Layers"),
    ("Rulers, grid and smart guides", "In View. Layers snap to guides, the grid, the centre and each other", "Rulers and Guides"),
    ("Before and after", "Hold \\ to see the frame without its adjustments", "Adjustments"),
    ("Tool keys", "T Text, U Shapes, B Markup, E Emoji, Q Mask, A Adjust, G Effects", "Shortcuts and Palette"),
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
                // list items stay short: one thought each
                if let Some(item) = line.strip_prefix("- ") {
                    assert!(item.split_whitespace().count() <= 16, "{t}: {item}");
                }
            }
        }
    }
}
