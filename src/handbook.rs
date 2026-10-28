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
        "Effects, Text, Shapes, Emoji, Markup, Censor, Image, Watermark and AI shield each add a layer.

Layers stack like in an image editor. Higher ones draw on top. Hide or lock them from the timeline.

## Text
- Drag text on the preview to place it
- Typing makes it appear letter by letter or word by word
- Fonts include the ones on your system

## Effects
- Colour, blur, glitch, distortions and more
- Transparency removes a colour. From Corner picks the background for you
- Soft edge and Remove colour spill clean up a green screen

## Animation
Moves a layer over time with ready-made motions like slide, hop and spin. Keyframes on the timeline (K) give finer control.",
    ),
    (
        "Timeline",
        "The timeline sits under the preview. The GIF has its own row and each layer gets one too.

## Clips
- Drag a clip to move it and its edges to trim it
- Trimmed frames are kept. Pull the edge back out to get them back
- S splits at the playhead and Delete removes a clip
- Those two keys work with the pointer on the timeline
- Right-click a GIF clip for Clip Speed and transitions into the next clip

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

## Speed and frames
- Speed changes the delay of every frame
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

#[cfg(test)]
mod tests {
    use super::*;

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
