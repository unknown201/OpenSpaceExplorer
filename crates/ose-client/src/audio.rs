//! Sound: the original's music, interface clicks and flight sounds (`assets/web/audio`, exported from the
//! game's Wwise banks by `tools/export_audio.py`, listed in `audio/sounds.json`).
//!
//! Music streams through one `<audio>` element per track and crossfades when the scene changes, the way the
//! original's music switch follows the scene state (main menu, KSC, VAB, the body in flight). Short sounds and
//! the engine loop are decoded once into Web Audio buffers: one-shots play through a master gain, the engine
//! loop through its own gain and playback rate, driven by thrust and throttle.
//!
//! Browsers only start audio after a user gesture, so nothing plays until `unlock` (the first click or key
//! press). The wanted state is kept before that, so the scene's music starts as soon as audio is allowed.
//! Sound can be muted from the menu; the choice is kept in local storage (`ose.sound`).
//!
//! Test hooks on `#audio`: `data-ose-music` (the scene's music key), `data-ose-sfx` (the last one-shot) and
//! `data-ose-sfx-count`, `data-ose-engine` (the engine loop's gain, 0 when silent), `data-ose-muted`.

use serde::Deserialize;
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use wasm_bindgen::JsCast;
use web_sys::{
    AudioBuffer, AudioBufferSourceNode, AudioContext, Document, GainNode, HtmlAudioElement,
};

#[derive(Deserialize, Clone, Debug)]
pub struct Sound {
    pub file: String,
    #[serde(default)]
    pub r#loop: bool,
    #[serde(default = "one")]
    pub gain: f64,
}

fn one() -> f64 {
    1.0
}

/// Seconds a music crossfade takes.
const FADE_S: f64 = 2.5;
/// Master volumes.
const MUSIC_VOLUME: f64 = 0.6;
const SFX_VOLUME: f64 = 0.8;

struct Track {
    key: String,
    el: HtmlAudioElement,
    /// 0..1 fade level; the element's volume is fade x the sound's gain x MUSIC_VOLUME.
    fade: f64,
    gain: f64,
}

#[derive(Default)]
pub struct Audio {
    sounds: HashMap<String, Sound>,
    ctx: Option<AudioContext>,
    master: Option<GainNode>,
    buffers: Rc<RefCell<HashMap<String, AudioBuffer>>>,
    /// The scene's music key, wanted even before audio is unlocked.
    want_music: Option<String>,
    current: Option<Track>,
    fading: Vec<Track>,
    engine: Option<(AudioBufferSourceNode, GainNode)>,
    engine_level: f64,
    /// The scene's ambience bed (the KSC's launch pad emitter, the VAB hall) and which one it is.
    ambience: Option<(String, AudioBufferSourceNode, GainNode)>,
    want_ambience: Option<String>,
    sfx_count: u32,
    pub muted: bool,
    last_ms: f64,
}

impl Audio {
    pub fn new(muted: bool) -> Self {
        Audio {
            muted,
            ..Default::default()
        }
    }

    pub fn set_manifest(&mut self, sounds: HashMap<String, Sound>) {
        self.sounds = sounds;
        self.load_buffers();
        if self.current.is_none() && self.unlocked() {
            if let Some(k) = self.want_music.clone() {
                self.start_track(&k);
            }
        }
    }

    pub fn has(&self, key: &str) -> bool {
        self.sounds.contains_key(key)
    }

    pub fn unlocked(&self) -> bool {
        self.ctx.is_some()
    }

    /// Start audio after a user gesture: the context, the short sounds' buffers and the scene's music.
    pub fn unlock(&mut self) {
        if self.ctx.is_none() {
            let Ok(ctx) = AudioContext::new() else { return };
            if let Ok(g) = ctx.create_gain() {
                g.gain()
                    .set_value(if self.muted { 0.0 } else { SFX_VOLUME as f32 });
                g.connect_with_audio_node(&ctx.destination()).ok();
                self.master = Some(g);
            }
            self.ctx = Some(ctx);
            self.load_buffers();
        }
        if let Some(ctx) = &self.ctx {
            ctx.resume().ok();
        }
        if self.current.is_none() {
            if let Some(k) = self.want_music.clone() {
                self.start_track(&k);
            }
        }
    }

    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
        if let Some(g) = &self.master {
            g.gain()
                .set_value(if muted { 0.0 } else { SFX_VOLUME as f32 });
        }
        self.apply_volumes();
    }

    /// Decode every non-music sound once the context exists (music streams instead).
    fn load_buffers(&self) {
        let Some(ctx) = self.ctx.clone() else { return };
        for (key, s) in &self.sounds {
            if key.starts_with("music") || self.buffers.borrow().contains_key(key) {
                continue;
            }
            let (key, url, ctx, buffers) = (
                key.clone(),
                format!("assets/{}", s.file),
                ctx.clone(),
                self.buffers.clone(),
            );
            wasm_bindgen_futures::spawn_local(async move {
                let Some(window) = web_sys::window() else {
                    return;
                };
                let Ok(resp) =
                    wasm_bindgen_futures::JsFuture::from(window.fetch_with_str(&url)).await
                else {
                    return;
                };
                let Ok(resp) = resp.dyn_into::<web_sys::Response>() else {
                    return;
                };
                if !resp.ok() {
                    return;
                }
                let Ok(bytes) = resp.array_buffer() else {
                    return;
                };
                let Ok(bytes) = wasm_bindgen_futures::JsFuture::from(bytes).await else {
                    return;
                };
                let Ok(p) = ctx.decode_audio_data(&bytes.unchecked_into()) else {
                    return;
                };
                if let Ok(buf) = wasm_bindgen_futures::JsFuture::from(p).await {
                    if let Ok(buf) = buf.dyn_into::<AudioBuffer>() {
                        buffers.borrow_mut().insert(key, buf);
                    }
                }
            });
        }
    }

    /// The scene's music; a change crossfades from the playing track.
    pub fn music(&mut self, key: Option<&str>) {
        // The key is kept even when the sound list lacks it (no game assets): the hook still names the scene.
        if self.want_music.as_deref() == key {
            return;
        }
        self.want_music = key.map(str::to_string);
        if let Some(t) = self.current.take() {
            self.fading.push(t);
        }
        if let (Some(k), true) = (key, self.unlocked()) {
            self.start_track(k);
        }
    }

    fn start_track(&mut self, key: &str) {
        let Some(s) = self.sounds.get(key) else {
            return;
        };
        let Ok(el) = HtmlAudioElement::new_with_src(&format!("assets/{}", s.file)) else {
            return;
        };
        el.set_loop(s.r#loop);
        el.set_volume(0.0);
        let _ = el.play();
        self.current = Some(Track {
            key: key.to_string(),
            el,
            fade: 0.0,
            gain: s.gain,
        });
    }

    fn apply_volumes(&self) {
        let m = if self.muted { 0.0 } else { MUSIC_VOLUME };
        for t in self.current.iter().chain(self.fading.iter()) {
            t.el.set_volume((t.fade * t.gain * m).clamp(0.0, 1.0));
        }
    }

    /// A one-shot sound.
    pub fn play(&mut self, key: &str) {
        self.play_rate(key, 1.0);
    }

    pub fn play_rate(&mut self, key: &str, rate: f64) {
        if !self.sounds.contains_key(key) {
            return;
        }
        self.sfx_count += 1;
        self.mark("data-ose-sfx", key);
        let (Some(ctx), Some(master)) = (&self.ctx, &self.master) else {
            return;
        };
        let Some(buf) = self.buffers.borrow().get(key).cloned() else {
            return;
        };
        let gain = self.sounds[key].gain;
        let Ok(src) = ctx.create_buffer_source() else {
            return;
        };
        src.set_buffer(Some(&buf));
        src.playback_rate().set_value(rate as f32);
        let Ok(g) = ctx.create_gain() else { return };
        g.gain().set_value(gain as f32);
        src.connect_with_audio_node(&g).ok();
        g.connect_with_audio_node(master).ok();
        src.start().ok();
    }

    /// The engine loop: `level` 0 (silent) to 1 (full thrust), `rate` its playback rate (lower for big engines).
    pub fn engine(&mut self, key: &str, level: f64, rate: f64) {
        let level = level.clamp(0.0, 1.0);
        let was = self.engine_level;
        self.engine_level = level;
        if level <= 0.0 {
            if let Some((src, _)) = self.engine.take() {
                AsRef::<web_sys::AudioScheduledSourceNode>::as_ref(&src)
                    .stop()
                    .ok();
                if was > 0.0 {
                    self.play("engine_liquid_stop");
                }
            }
            return;
        }
        let (Some(ctx), Some(master)) = (&self.ctx, &self.master) else {
            return;
        };
        if self.engine.is_none() {
            let Some(buf) = self.buffers.borrow().get(key).cloned() else {
                return;
            };
            let (Ok(src), Ok(g)) = (ctx.create_buffer_source(), ctx.create_gain()) else {
                return;
            };
            src.set_buffer(Some(&buf));
            src.set_loop(true);
            g.gain().set_value(0.0);
            src.connect_with_audio_node(&g).ok();
            g.connect_with_audio_node(master).ok();
            src.start().ok();
            self.engine = Some((src, g));
        }
        if let Some((src, g)) = &self.engine {
            let gain = self.sounds.get(key).map(|s| s.gain).unwrap_or(1.0);
            // Ramp toward the new level so throttle steps don't click.
            g.gain()
                .set_target_at_time(
                    (gain * (0.25 + 0.75 * level)) as f32,
                    ctx.current_time(),
                    0.08,
                )
                .ok();
            src.playback_rate()
                .set_value((rate * (0.85 + 0.25 * level)) as f32);
        }
    }

    /// The scene's ambience loop, quietly under the music; `None` stops it.
    pub fn ambience(&mut self, key: Option<&str>) {
        self.want_ambience = key.map(str::to_string);
        if self.ambience.as_ref().map(|a| a.0.as_str()) == key {
            return;
        }
        if let Some((_, src, _)) = self.ambience.take() {
            AsRef::<web_sys::AudioScheduledSourceNode>::as_ref(&src)
                .stop()
                .ok();
        }
        let Some(key) = key else { return };
        let (Some(ctx), Some(master)) = (&self.ctx, &self.master) else {
            return;
        };
        let Some(buf) = self.buffers.borrow().get(key).cloned() else {
            return;
        };
        let (Ok(src), Ok(g)) = (ctx.create_buffer_source(), ctx.create_gain()) else {
            return;
        };
        src.set_buffer(Some(&buf));
        src.set_loop(true);
        let gain = self.sounds.get(key).map(|s| s.gain).unwrap_or(0.5);
        g.gain().set_value(0.0);
        g.gain()
            .set_target_at_time(gain as f32, ctx.current_time(), 1.0)
            .ok();
        src.connect_with_audio_node(&g).ok();
        g.connect_with_audio_node(master).ok();
        src.start().ok();
        self.ambience = Some((key.to_string(), src, g));
    }

    /// Advance crossfades and refresh the test hooks; call once per frame.
    pub fn tick(&mut self, now_ms: f64, doc: &Document) {
        let dt = if self.last_ms > 0.0 {
            ((now_ms - self.last_ms) / 1000.0).clamp(0.0, 0.5)
        } else {
            0.0
        };
        self.last_ms = now_ms;
        let step = dt / FADE_S;
        if let Some(t) = &mut self.current {
            t.fade = (t.fade + step).min(1.0);
        }
        for t in &mut self.fading {
            t.fade -= step;
        }
        self.fading.retain(|t| {
            if t.fade <= 0.0 {
                let _ = t.el.pause();
                false
            } else {
                true
            }
        });
        self.apply_volumes();
        if let Some(el) = doc.get_element_by_id("audio") {
            el.set_attribute("data-ose-music", self.want_music.as_deref().unwrap_or(""))
                .ok();
            el.set_attribute("data-ose-sfx-count", &self.sfx_count.to_string())
                .ok();
            el.set_attribute("data-ose-engine", &format!("{:.2}", self.engine_level))
                .ok();
            el.set_attribute("data-ose-muted", if self.muted { "1" } else { "0" })
                .ok();
            el.set_attribute(
                "data-ose-ambience",
                self.want_ambience.as_deref().unwrap_or(""),
            )
            .ok();
            el.set_attribute(
                "data-ose-playing",
                self.current.as_ref().map(|t| t.key.as_str()).unwrap_or(""),
            )
            .ok();
        }
    }

    fn mark(&self, attr: &str, value: &str) {
        if let Some(el) = web_sys::window()
            .and_then(|w| w.document())
            .and_then(|d| d.get_element_by_id("audio"))
        {
            el.set_attribute(attr, value).ok();
        }
    }
}

/// Parse `audio/sounds.json`.
pub fn parse_manifest(text: &str) -> HashMap<String, Sound> {
    serde_json::from_str(text).unwrap_or_default()
}

/// The music key for a scene: the main menu, the KSC and its buildings, the VAB, or the body in flight (the
/// original keys its flight music on the sphere of influence; the Mun has its own, other bodies use Kerbin's).
pub fn scene_music(scene: Scene<'_>) -> &'static str {
    match scene {
        Scene::MainMenu => "music_menu",
        Scene::Ksc => "music_ksc",
        Scene::Vab => "music_vab",
        Scene::Tracking => "music_tracking",
        Scene::Rnd => "music_rnd",
        Scene::Mission => "music_mission",
        Scene::Flight(body) if body.eq_ignore_ascii_case("mun") => "music_mun",
        Scene::Flight(_) => "music_kerbin",
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Scene<'a> {
    MainMenu,
    Ksc,
    Vab,
    Tracking,
    Rnd,
    Mission,
    Flight(&'a str),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_parses_and_scenes_pick_their_music() {
        let m = parse_manifest(
            r#"{"music_menu": {"file": "audio/music_menu.ogg", "event": "Play_amb_main_menu", "wem": 1,
                "loop": true, "gain": 0.5}, "stage": {"file": "audio/stage.ogg", "loop": false}}"#,
        );
        assert!(m["music_menu"].r#loop);
        assert_eq!(m["stage"].gain, 1.0);
        assert_eq!(scene_music(Scene::MainMenu), "music_menu");
        assert_eq!(scene_music(Scene::Flight("Mun")), "music_mun");
        assert_eq!(scene_music(Scene::Flight("Kerbin")), "music_kerbin");
    }
}
