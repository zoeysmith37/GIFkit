//! GIFKIT_SMOKE=<report file>: the app drives itself through the main
//! parts (every tool panel, a text layer, playback, the timeline, an
//! export) and writes what happened to the report, then quits. CI runs it
//! on each platform so a crash or a hang in the real window shows up there.

use super::*;

pub(super) struct Smoke {
    report: PathBuf,
    step: usize,
    at: f64,
    log: Vec<String>,
    out: PathBuf,
}

impl Smoke {
    pub fn from_env() -> Option<Self> {
        let report = PathBuf::from(std::env::var_os("GIFKIT_SMOKE")?);
        let out = std::env::temp_dir().join(format!("gifkit-smoke-{}.gif", std::process::id()));
        Some(Self { report, step: 0, at: 0.0, log: Vec::new(), out })
    }
}

impl GifKitApp {
    pub(super) fn smoke_step(&mut self, ctx: &egui::Context) {
        let Some(mut s) = self.smoke.take() else { return };
        let now = ctx.input(|i| i.time);
        ctx.request_repaint();
        let frame = ctx.cumulative_frame_nr();
        // past two minutes something hangs: say so and stop
        if now > 120.0 {
            s.log.push(format!("timed out at step {}", s.step));
            self.smoke_finish(ctx, &s, false);
            return;
        }
        let idle = !self.busy();
        let next = |s: &mut Smoke, msg: String| {
            s.log.push(msg);
            s.step += 1;
            s.at = now;
        };
        match s.step {
            // the file opened and the first picture is up
            0 if self.base.is_some() && idle && frame > 10 => {
                let renderer = if crate::gpu::enabled() { "graphics card" } else { "processor" };
                next(&mut s, format!("opened {} frames at {:?}, effects on the {renderer}", self.plan.len(), self.dims()));
            }
            // every tool panel, one after another
            1 => {
                let i = ((now - s.at) * 10.0) as usize;
                match Tool::ALL.get(i) {
                    Some(t) => self.tool = *t,
                    None => next(&mut s, format!("showed {} tool panels", Tool::ALL.len())),
                }
            }
            2 => {
                let t = text::Annotation { text: "smoke test".into(), ..Default::default() };
                self.stack.push(LayerKind::Text(t));
                self.tool = Tool::Text;
                self.refresh_plan();
                next(&mut s, format!("added a text layer ({} layers)", self.stack.layers.len()));
            }
            3 => {
                self.playing = true;
                next(&mut s, "playing".into());
            }
            4 if now - s.at > 1.5 => {
                self.playing = false;
                let at = self.cur;
                next(&mut s, format!("played to frame {}", at + 1));
            }
            // the timeline: split the GIF at the playhead, then undo it
            5 if idle => {
                let n = self.gif_runs().len();
                self.cur = self.plan.len() / 2;
                self.tl_split_at(0, self.cur, now);
                let m = self.gif_runs().len();
                self.do_undo();
                next(&mut s, format!("split the GIF into {m} clips (was {n}) and undid it"));
            }
            6 if idle => {
                let path = s.out.clone();
                let opts = self.gif_opts.clone();
                self.run_export(ctx, "Save GIF", move |a, p| io::save_gif(a, &opts, &path, Some(p)).map(|n| format!("smoke export {n} bytes")));
                next(&mut s, "exporting".into());
            }
            7 if idle && now - s.at > 0.3 => {
                let size = std::fs::metadata(&s.out).map(|m| m.len()).unwrap_or(0);
                let back = io::load_animation(&s.out).map(|a| a.len()).unwrap_or(0);
                let _ = std::fs::remove_file(&s.out);
                let ok = size > 0 && back > 0;
                s.log.push(format!("exported {size} bytes, {back} frames read back"));
                self.smoke_finish(ctx, &s, ok);
                return;
            }
            _ => {}
        }
        self.smoke = Some(s);
    }

    fn smoke_finish(&mut self, ctx: &egui::Context, s: &Smoke, ok: bool) {
        let text = format!("{}\n{}\n", if ok { "ok" } else { "FAILED" }, s.log.join("\n"));
        let _ = std::fs::write(&s.report, text);
        // no "save changes?" on the way out
        self.modified = false;
        self.smoke = None;
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
}
