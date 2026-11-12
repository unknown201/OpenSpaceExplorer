//! Browser wiring for the advisory director. No ScheduleBurn, Stage, Translate or throttle intents.
use super::*;
use crate::rendezvous::{self as rdv, Level, Phase};
use ose_math::{rendezvous as mechanics, DVec3};
use web_sys::{HtmlInputElement, HtmlSelectElement};

fn select(doc: &Document, id: &str) -> String {
    doc.get_element_by_id(id)
        .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
        .map(|e| e.value())
        .unwrap_or_default()
}
fn value(doc: &Document, id: &str, value: &str) {
    if let Some(e) = doc
        .get_element_by_id(id)
        .and_then(|e| e.dyn_into::<HtmlSelectElement>().ok())
    {
        e.set_value(value);
    }
}
fn disabled(doc: &Document, id: &str, disable: bool) {
    if let Some(e) = doc.get_element_by_id(id) {
        if disable {
            e.set_attribute("disabled", "").ok();
        } else {
            e.remove_attribute("disabled").ok();
        }
    }
}
fn hidden(doc: &Document, id: &str, hide: bool) {
    if let Some(e) = doc.get_element_by_id(id) {
        if hide {
            e.set_attribute("hidden", "").ok();
        } else {
            e.remove_attribute("hidden").ok();
        }
    }
}
fn options(doc: &Document, id: &str, html: &str, selected: &str) {
    if let Some(e) = doc.get_element_by_id(id) {
        let key = format!("{}:{}", html.len(), hud::fnv(html));
        if e.get_attribute("data-options").as_deref() != Some(&key) {
            e.set_inner_html(html);
            e.set_attribute("data-options", &key).ok();
        }
    }
    value(doc, id, selected);
}

pub fn install(app: &Shared) -> Result<(), JsValue> {
    let doc = app.borrow().doc.clone();
    for id in ["rdv-level", "settings-rdv-level"] {
        value(&doc, id, app.borrow().rendezvous.level.key());
    }
    for (id, on) in [
        ("settings-rdv-world", app.borrow().rendezvous.world_cues),
        ("settings-rdv-audio", app.borrow().rendezvous.audible),
    ] {
        if let Some(e) = doc
            .get_element_by_id(id)
            .and_then(|e| e.dyn_into::<HtmlInputElement>().ok())
        {
            e.set_checked(on);
        }
    }
    for id in [
        "rdv-level",
        "settings-rdv-level",
        "rdv-destination",
        "lp-rdv-destination",
        "lp-rdv-enable",
        "rdv-own-port",
        "rdv-target-port",
        "settings-rdv-world",
        "settings-rdv-audio",
    ] {
        let Some(el) = doc.get_element_by_id(id) else {
            continue;
        };
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut a = a.borrow_mut();
            match id {
                "rdv-level" | "settings-rdv-level" => {
                    a.rdv_release();
                    let level = Level::parse(&select(&a.doc, id));
                    a.rendezvous.level = level;
                    a.rendezvous.enabled =
                        level != Level::Off && a.rendezvous.destination.is_some();
                    store("ose.rdv.level", level.key());
                    for key in ["rdv-level", "settings-rdv-level"] {
                        value(&a.doc, key, level.key());
                    }
                    if level == Level::Off {
                        a.rendezvous.cue = None;
                        a.rendezvous.point = None;
                        a.rendezvous.axis_point = None;
                    }
                }
                "rdv-destination" => {
                    a.rdv_release();
                    a.rendezvous.invalidate();
                    a.rendezvous.destination = select(&a.doc, id).parse().ok();
                    a.rendezvous.target_port = None;
                    a.rendezvous.own_port = None;
                    a.rendezvous.enabled = false;
                    a.rendezvous.phase = Phase::Prepare;
                    a.target = a.rendezvous.destination.map(ose_net::TargetId::Vessel);
                    a.send_target();
                }
                "lp-rdv-destination" => {
                    a.rendezvous.launch_destination = select(&a.doc, id).parse().ok();
                }
                "lp-rdv-enable" => {
                    a.rendezvous.launch_enabled = checkbox(&a.doc, id).unwrap_or(false);
                }
                "rdv-own-port" | "rdv-target-port" => {
                    a.rdv_release();
                    a.rendezvous.dock_latched = false;
                    let chosen = select(&a.doc, id);
                    let port = (!chosen.is_empty()).then_some(chosen);
                    if id == "rdv-own-port" {
                        a.rendezvous.own_port = port;
                    } else {
                        a.rendezvous.target_port = port;
                    }
                    if a.rendezvous.phase == Phase::Unavailable {
                        a.rendezvous.phase = Phase::Prepare;
                    }
                }
                "settings-rdv-world" => {
                    a.rendezvous.world_cues = checkbox(&a.doc, id).unwrap_or(true);
                    store(
                        "ose.rdv.world",
                        if a.rendezvous.world_cues { "on" } else { "off" },
                    );
                }
                "settings-rdv-audio" => {
                    a.rendezvous.audible = checkbox(&a.doc, id).unwrap_or(false);
                    store(
                        "ose.rdv.audio",
                        if a.rendezvous.audible { "on" } else { "off" },
                    );
                }
                _ => {}
            }
            a.rdv_save();
            a.rendezvous.last_render_ms = 0.0;
        });
        el.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    for id in ["rdv-plan", "rdv-hold", "rdv-warp", "rdv-cancel"] {
        let Some(el) = doc.get_element_by_id(id) else {
            continue;
        };
        let a = app.clone();
        let cb = Closure::<dyn FnMut()>::new(move || {
            let mut a = a.borrow_mut();
            match id {
                "rdv-plan" => {
                    a.rdv_release();
                    let arrival = a.rendezvous.transfer.map(|p| p.arrival_ut);
                    a.rendezvous.invalidate();
                    a.rendezvous.correction_arrival = arrival;
                    if a.rendezvous.destination.is_none() {
                        if let Some(ose_net::TargetId::Vessel(id)) = a.target {
                            a.rendezvous.destination = Some(id);
                        }
                    }
                    if a.rendezvous.destination.is_some() {
                        if a.rendezvous.level == Level::Off {
                            a.rendezvous.level = Level::Guided;
                            store("ose.rdv.level", "guided");
                        }
                        a.rendezvous.enabled = true;
                        a.rendezvous.phase = Phase::Prepare;
                        a.rendezvous.note.clear();
                        a.target = a.rendezvous.destination.map(ose_net::TargetId::Vessel);
                        a.send_target();
                    } else {
                        a.rendezvous.note = "Select an orbiting destination first.".into();
                    }
                    a.rdv_save();
                }
                "rdv-hold" => {
                    if a.rendezvous.hold {
                        a.rdv_release();
                    } else if let Some(cue) = a.rendezvous.cue {
                        a.rendezvous.hold = true;
                        a.rendezvous.hold_sent_ms = 0.0;
                        a.attitude = ose_net::AttitudeMode::Fixed(cue);
                        a.send_controls();
                    }
                }
                "rdv-warp" => {
                    let now = a.time.ut_at(now_ms());
                    let at = if a.rendezvous.phase == Phase::Launch {
                        a.rdv_plane().map(|p| now + p.wait_s - 20.0)
                    } else {
                        a.rendezvous.transfer.map(|p| {
                            if a.rendezvous.phase == Phase::Coast {
                                p.arrival_ut
                                    - (p.arrival_delta_v.length()
                                        / a.rendezvous.acceleration.max(0.01)
                                        * 0.5
                                        + 120.0)
                                        .max(180.0)
                            } else {
                                p.departure_ut
                                    - a.rendezvous.remaining.length()
                                        / a.rendezvous.acceleration.max(0.01)
                                        * 0.5
                                    - 120.0
                            }
                        })
                    };
                    if let Some(at) = at {
                        a.start_warp_to(at, "rendezvous opportunity");
                    }
                }
                "rdv-cancel" => {
                    a.rdv_release();
                    a.rendezvous.enabled = false;
                    a.rendezvous.invalidate();
                    a.rendezvous.phase = Phase::Prepare;
                    a.rendezvous.note = "Guidance ended. Station target retained.".into();
                    a.rdv_save();
                }
                _ => {}
            }
            a.rendezvous.last_render_ms = 0.0;
        });
        el.add_event_listener_with_callback("click", cb.as_ref().unchecked_ref())?;
        cb.forget();
    }
    Ok(())
}

impl App {
    pub(super) fn rdv_target_changed(&mut self) {
        self.rdv_release();
        self.rendezvous.invalidate();
        self.rendezvous.enabled = false;
        self.rendezvous.phase = Phase::Prepare;
        self.rendezvous.target_port = None;
        self.rendezvous.own_port = None;
        self.rendezvous.destination = match self.target {
            Some(ose_net::TargetId::Vessel(id)) => Some(id),
            _ => None,
        };
        self.rdv_save();
    }
    pub(super) fn rdv_save(&self) {
        let Some(id) = self.flying else { return };
        store(
            &format!("ose.rdv.{}.{}", self.campaign_id, id),
            &serde_json::json!({
            "destination":self.rendezvous.destination,"own_port":self.rendezvous.own_port,
            "target_port":self.rendezvous.target_port,"enabled":self.rendezvous.enabled})
            .to_string(),
        );
    }
    pub(super) fn rdv_start(&mut self, id: u64) {
        self.rendezvous.hold = false;
        self.rendezvous.invalidate();
        self.rendezvous.phase = Phase::Prepare;
        self.rendezvous.note.clear();
        self.rendezvous.own_port = None;
        self.rendezvous.target_port = None;
        self.rendezvous.destination = None;
        self.rendezvous.enabled = false;
        if let Some(s) = stored(&format!("ose.rdv.{}.{}", self.campaign_id, id))
            .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        {
            self.rendezvous.destination = s["destination"].as_u64().filter(|d| *d != id);
            self.rendezvous.own_port = s["own_port"].as_str().map(str::to_string);
            self.rendezvous.target_port = s["target_port"].as_str().map(str::to_string);
            self.rendezvous.enabled =
                s["enabled"].as_bool().unwrap_or(false) && self.rendezvous.level != Level::Off;
        } else if let Some(destination) = self
            .rendezvous
            .launch_destination
            .take()
            .filter(|d| *d != id)
        {
            self.rendezvous.destination = Some(destination);
            self.rendezvous.enabled = self.rendezvous.launch_enabled;
            if self.rendezvous.enabled && self.rendezvous.level == Level::Off {
                self.rendezvous.level = Level::Guided;
                store("ose.rdv.level", "guided");
            }
        }
        if let Some(destination) = self.rendezvous.destination {
            self.target = Some(ose_net::TargetId::Vessel(destination));
        }
        if self.rendezvous.enabled {
            if let Some(e) = self.doc.get_element_by_id("rdv-panel") {
                e.set_attribute("open", "").ok();
            }
        }
        self.rendezvous.last_render_ms = 0.0;
    }
    pub(super) fn rdv_release(&mut self) {
        if self.rendezvous.hold {
            self.rendezvous.hold = false;
            self.sas_hold();
        }
        if let Some(e) = self.doc.get_element_by_id("rdv-hold") {
            e.set_attribute("aria-pressed", "false").ok();
        }
    }
    fn rdv_phase(&mut self, phase: Phase) {
        if self.rendezvous.phase != phase {
            self.rdv_release();
            self.rendezvous.phase = phase;
        }
    }
    fn rdv_plane(&self) -> Option<mechanics::PlaneWindow> {
        let scene = self.scene.as_ref()?;
        let me = scene.vessel(self.flying?)?;
        let target = scene.vessel(self.rendezvous.destination?)?;
        if me.reference_body != target.reference_body {
            return None;
        }
        let body = scene.bodies.get(me.reference_body as usize)?;
        let now = self.time.ut_at(now_ms());
        let r = scene.vessel_position(me, now) - scene.position(me.reference_body as usize, now);
        let target = scene::orbit_from(body.mu, &target.orbit);
        mechanics::plane_window(
            r,
            *scene.spin_omega.get(me.reference_body as usize)?,
            target.frame.z,
        )
    }
    fn rdv_options(&self) {
        let Some(scene) = &self.scene else { return };
        let mut html = "<option value=\"\">Choose an orbiting vessel</option>".to_string();
        for v in &scene.vessels {
            if Some(v.id) == self.flying
                || v.flight
                    .as_ref()
                    .is_none_or(|f| f.eva || f.situation != ose_net::FlightSituation::Orbit)
            {
                continue;
            }
            html.push_str(&format!(
                "<option value=\"{}\">{}</option>",
                v.id,
                esc(&v.name)
            ));
        }
        let chosen = self
            .rendezvous
            .destination
            .map(|id| id.to_string())
            .unwrap_or_default();
        options(&self.doc, "rdv-destination", &html, &chosen);
        let chosen = self
            .rendezvous
            .launch_destination
            .map(|id| id.to_string())
            .unwrap_or_default();
        options(&self.doc, "lp-rdv-destination", &html, &chosen);
    }
    fn rdv_ask(&mut self, id: u64) {
        let Some(v) = self.scene.as_ref().and_then(|s| s.vessel(id)) else {
            return;
        };
        let rev = v.craft_rev;
        if self.vessel_crafts.get(&id).map(|c| c.0) != Some(rev)
            && self.vessel_craft_asked.get(&id) != Some(&rev)
        {
            if let Some(ws) = &self.ws {
                send(ws, &ClientMessage::GetVesselCraft { vessel: id });
                self.vessel_craft_asked.insert(id, rev);
            }
        }
    }
    fn rdv_craft(&self, id: u64) -> Option<&ose_data::craft::Craft> {
        let v = self.scene.as_ref()?.vessel(id)?;
        let (rev, c) = self.vessel_crafts.get(&id)?;
        (*rev == v.craft_rev).then_some(c.as_ref()).flatten()
    }
    fn rdv_port_options(&mut self, own: &[rdv::Port], target: &[rdv::Port]) {
        if self.rendezvous.own_port.is_none() {
            self.rendezvous.own_port = own
                .iter()
                .find(|p| p.free && target.iter().any(|t| t.free && rdv::compatible(p, t)))
                .map(|p| p.guid.clone());
        }
        let chosen = own
            .iter()
            .find(|p| Some(&p.guid) == self.rendezvous.own_port.as_ref());
        if self.rendezvous.target_port.is_none() {
            self.rendezvous.target_port = target
                .iter()
                .find(|p| p.free && chosen.is_some_and(|o| rdv::compatible(o, p)))
                .map(|p| p.guid.clone());
        }
        for (id, list, selected) in [
            ("rdv-own-port", own, &self.rendezvous.own_port),
            ("rdv-target-port", target, &self.rendezvous.target_port),
        ] {
            let mut html = "<option value=\"\">Select port</option>".to_string();
            for p in list {
                let fits = id == "rdv-own-port" || chosen.is_some_and(|o| rdv::compatible(o, p));
                let status = if !p.free {
                    "occupied"
                } else if !fits {
                    "incompatible"
                } else {
                    "available"
                };
                html.push_str(&format!(
                    "<option value=\"{}\"{}>{} · {status}</option>",
                    esc(&p.guid),
                    if !p.free || !fits { " disabled" } else { "" },
                    esc(&p.name)
                ));
            }
            options(&self.doc, id, &html, selected.as_deref().unwrap_or(""));
        }
    }

    pub(super) fn rendezvous_tick(&mut self, now_ms: f64) {
        if now_ms - self.rendezvous.last_render_ms < 100.0 {
            return;
        }
        self.rendezvous.last_render_ms = now_ms;
        self.rdv_options();
        let Some(id) = self.flying else { return };
        let enabled = self.rendezvous.enabled && self.rendezvous.level != Level::Off;
        if enabled && now_ms - self.rendezvous.snapshot_ms > 5_000.0 {
            self.rdv_phase(Phase::Unavailable);
            self.rendezvous.cue = None;
            self.rendezvous.point = None;
            self.rendezvous.axis_point = None;
            self.rdv_render(
                "Waiting for live flight data",
                vec![],
                "Guidance hold released. Replan after the connection recovers.",
            );
            return;
        }
        if let Some(panel) = self.doc.get_element_by_id("rdv-panel") {
            panel
                .set_attribute("data-level", self.rendezvous.level.key())
                .ok();
        }
        for key in ["rdv-level", "settings-rdv-level"] {
            value(&self.doc, key, self.rendezvous.level.key());
        }
        disabled(&self.doc, "rdv-hold", true);
        disabled(&self.doc, "rdv-warp", true);
        hidden(&self.doc, "rdv-alignment", true);
        hidden(&self.doc, "rdv-world", true);
        hidden(&self.doc, "rdv-axis", true);
        if self.rendezvous.phase == Phase::Complete {
            self.rdv_render(
                "Module attached to station",
                vec![],
                "Select the station and a free port for your next delivery.",
            );
            return;
        }
        let Some(destination) = self.rendezvous.destination else {
            set_text(&self.doc, "rdv-summary", "optional guidance");
            set_text(
                &self.doc,
                "rdv-instruction",
                "Select a destination for this delivery",
            );
            set_text(&self.doc, "rdv-values", "");
            set_text(&self.doc, "rdv-note", &self.rendezvous.note);
            return;
        };
        self.rdv_ask(id);
        self.rdv_ask(destination);
        let pair = self
            .scene
            .as_ref()
            .and_then(|s| Some((s.vessel(id)?.clone(), s.vessel(destination)?.clone())));
        let Some((me, target)) = pair else {
            if enabled {
                self.rdv_phase(Phase::Unavailable);
                self.rendezvous.cue = None;
                self.rendezvous.point = None;
            }
            self.rdv_render(
                "Target unavailable. Select another destination.",
                vec![],
                "The station may have docked, split, or been removed.",
            );
            return;
        };
        let own = self.rdv_craft(id).cloned();
        let station = self.rdv_craft(destination).cloned();
        let own_ports = own
            .as_ref()
            .map(|c| rdv::ports(c, &self.catalog))
            .unwrap_or_default();
        let target_ports = station
            .as_ref()
            .map(|c| rdv::ports(c, &self.catalog))
            .unwrap_or_default();
        self.rdv_port_options(&own_ports, &target_ports);
        if !enabled {
            self.rendezvous.cue = None;
            self.rendezvous.point = None;
            self.rdv_release();
            self.rdv_render(
                "Choose ports, then plan this delivery",
                vec![],
                "Attitude holds never apply thrust. Off keeps ordinary vessel targeting.",
            );
            return;
        }
        if own.is_none() || station.is_none() {
            self.rdv_phase(Phase::Prepare);
            self.rendezvous.cue = None;
            self.rdv_render(
                "Loading vessel and port data",
                vec![],
                "Planning begins when authoritative craft documents arrive.",
            );
            return;
        }
        let (Some(f), Some(tf)) = (&me.flight, &target.flight) else {
            self.rdv_phase(Phase::Unavailable);
            self.rdv_render("Flight instruments unavailable", vec![], "");
            return;
        };
        if me.reference_body != target.reference_body {
            self.rdv_phase(Phase::Unavailable);
            self.rendezvous.cue = None;
            self.rdv_render(
                "Destination orbits a different body",
                vec![],
                "Transfer to its reference body before planning local rendezvous.",
            );
            return;
        }
        let scene = self.scene.as_ref().unwrap();
        let ut = self.time.ut_at(now_ms);
        let body = scene.bodies[me.reference_body as usize].clone();
        let mu = body.mu;
        let minimum_radius = body.radius_m + body.atmosphere_depth_m + 1_000.0;
        let bp = scene.position(me.reference_body as usize, ut);
        let r = scene.vessel_position(&me, ut) - bp;
        let tr = scene.vessel_position(&target, ut) - bp;
        let me_orbit = scene::orbit_from(mu, &me.orbit);
        let target_orbit = scene::orbit_from(mu, &target.orbit);
        let own_v = if me.on_rails {
            me_orbit.state_at(ut).1
        } else {
            me.velocity_mps - scene.body_velocity(me.reference_body as usize, scene.vessels_ut)
        };
        let target_v = if target.on_rails {
            target_orbit.state_at(ut).1
        } else {
            target.velocity_mps
                - scene.body_velocity(target.reference_body as usize, scene.vessels_ut)
        };
        let acceleration = if f.throttle > 0.01 {
            f.thrust_kn / f.throttle / f.mass_t.max(0.001)
        } else {
            own.as_ref()
                .map(|c| rdv::active_thrust(c, &self.catalog, f.stage, f.stages))
                .unwrap_or(0.0)
                / f.mass_t.max(0.001)
        };
        self.rendezvous.acceleration = acceleration;
        let relative = mechanics::relative(
            tr - r,
            target_v - own_v,
            acceleration.max(f.rcs_thrust_kn as f64 / (6.0 * f.mass_t.max(0.001))),
        )
        .unwrap();
        let old_phase = self.rendezvous.phase;
        let budget = (f.delta_v_now_mps * 0.9).max(
            own.as_ref()
                .map(|c| rdv::active_budget(f.mass_t, f.propellant_t, c, &self.catalog))
                .unwrap_or(0.0),
        );
        self.rendezvous.cue = None;
        self.rendezvous.point = None;
        self.rendezvous.axis_point = None;
        if !f.has_control
            || matches!(
                f.situation,
                ose_net::FlightSituation::Crashed | ose_net::FlightSituation::Splashed
            )
        {
            self.rdv_phase(Phase::Unavailable);
            self.rdv_render(
                "Vessel cannot follow guidance",
                vec![],
                "Restore control authority or choose another craft.",
            );
            return;
        }
        if f.situation != ose_net::FlightSituation::Orbit || me_orbit.periapsis() < minimum_radius {
            self.rdv_phase(Phase::Launch);
            let window = self.rdv_plane();
            let mut rows = vec![
                (
                    "Target inclination",
                    format!("{:.1}°", target.orbit.inclination_deg),
                ),
                (
                    "Station altitude",
                    format!("{:.1} km", (tr.length() - body.radius_m) / 1000.0),
                ),
            ];
            let instruction = if let Some(p) = window {
                rows.push(("Launch heading", format!("{:.1}°", p.heading_deg)));
                rows.push(("Plane crossing in", tracking::duration(p.wait_s)));
                rows.push(("Plane error", format!("{:.2}°", p.plane_error_deg)));
                disabled(&self.doc, "rdv-warp", p.wait_s < 40.0);
                if p.plane_error_deg < 0.5 {
                    "Plane aligned · fly your ascent"
                } else {
                    "Wait for the station’s orbital plane"
                }
            } else {
                "Reach a safe orbit, then plan the intercept"
            };
            self.rdv_render(instruction,rows,"Plane opportunity only; ascent time and target phase are not yet predicted. Fly ascent and circularization manually.");
            return;
        }
        let signature = (id, me.reference_body, me.craft_rev, target.craft_rev);
        if self.rendezvous.signature.is_some_and(|s| s != signature) {
            self.rdv_phase(Phase::Unavailable);
            self.rendezvous.transfer = None;
            self.rdv_render(
                "Craft changed · replan rendezvous",
                vec![],
                "Port IDs are retained. Confirm the ports and recalculate before continuing.",
            );
            return;
        }
        self.rendezvous.signature = Some(signature);
        let arrival_braking = old_phase == Phase::Coast
            && self.rendezvous.transfer.is_some_and(|p| {
                p.arrival_ut - ut
                    <= (p.arrival_delta_v.length() / acceleration.max(0.01) * 0.5 + 120.0)
                        .max(180.0)
                    || relative
                        .stopping_distance_m
                        .is_some_and(|d| relative.distance_m < d + 8_000.0)
            });
        let near = relative.distance_m < 8_000.0
            || arrival_braking
            || matches!(old_phase, Phase::Approach | Phase::Dock);
        if !near && self.rendezvous.transfer.is_none() && old_phase != Phase::Unavailable {
            let correction = self
                .rendezvous
                .correction_arrival
                .take()
                .and_then(|arrival| {
                    mechanics::correction(
                        me_orbit,
                        target_orbit,
                        ut + 180.0,
                        arrival,
                        minimum_radius,
                        budget,
                    )
                });
            self.rendezvous.transfer = correction.or_else(|| {
                mechanics::plan(
                    me_orbit,
                    target_orbit,
                    ut + (budget / (2.0 * acceleration.max(0.01))).min(120.0),
                    minimum_radius,
                    budget,
                )
            });
            if let Some(p) = self.rendezvous.transfer {
                self.rendezvous.remaining = p.delta_v;
            }
        }
        if !near {
            let Some(plan) = self.rendezvous.transfer else {
                self.rdv_phase(Phase::Unavailable);
                self.rdv_render("No feasible rendezvous plan",vec![("Available Δv",format!("{budget:.0} m/s"))],"No safe two-body transfer fits the current fuel budget. Improve the orbit or fuel supply, then replan.");
                return;
            };
            let sample_ut = scene.vessels_ut;
            let sample_r = me.position_m - scene.position(me.reference_body as usize, sample_ut);
            let sample_v =
                me.velocity_mps - scene.body_velocity(me.reference_body as usize, sample_ut);
            let firing = f.thrust_kn > 0.0;
            if let Some((at, r0, v0, was_firing)) = self.rendezvous.burn_sample {
                if sample_ut > at && (firing || was_firing) {
                    self.rendezvous.remaining = self.rendezvous.remaining
                        - mechanics::delivered_delta_v(
                            mu,
                            r0,
                            v0,
                            sample_r,
                            sample_v,
                            sample_ut - at,
                        );
                }
            }
            if self.rendezvous.burn_sample.is_none_or(|s| sample_ut > s.0) {
                self.rendezvous.burn_sample = Some((sample_ut, sample_r, sample_v, firing));
            }
            let left = self.rendezvous.remaining.length();
            let burn_seconds = left / acceleration.max(1e-8);
            let starts_in = plan.departure_ut - ut - burn_seconds * 0.5;
            if left < 0.25 && f.throttle < 0.001 {
                self.rdv_phase(Phase::Coast);
            } else if old_phase != Phase::Coast {
                self.rdv_phase(Phase::Intercept);
            }
            let coasting = self.rendezvous.phase == Phase::Coast;
            if !coasting && ut > plan.departure_ut + 120.0 && f.throttle < 0.001 && left > 1.0 {
                self.rdv_phase(Phase::Unavailable);
                self.rdv_render(
                    "Burn missed · replan intercept",
                    vec![],
                    "The destination is retained. Recalculate from your current orbit.",
                );
                return;
            }
            self.rendezvous.cue = if coasting {
                None
            } else {
                Some(self.rendezvous.remaining.normalized())
            };
            self.rendezvous.point = Some(bp + target_orbit.state_at(plan.arrival_ut).0);
            let mut rows = vec![(
                "Encounter in",
                tracking::duration((plan.arrival_ut - ut).max(0.0)),
            )];
            let instruction = if coasting {
                let miss = (me_orbit.state_at(plan.arrival_ut).0
                    - target_orbit.state_at(plan.arrival_ut).0)
                    .length();
                rows.push(("Predicted pass", format!("{miss:.0} m")));
                if miss > 1_500.0 {
                    "Encounter needs a correction · replan"
                } else {
                    "Coast · prepare to match velocity"
                }
            } else {
                rows.push((
                    "Start burn in",
                    if starts_in > 0.0 {
                        tracking::duration(starts_in)
                    } else {
                        "NOW".into()
                    },
                ));
                rows.push(("Δv remaining", format!("{left:.2} m/s")));
                let forward = self
                    .current_nav()
                    .map(|n| DVec3::new(n.forward.x, n.forward.y, n.forward.z))
                    .unwrap_or(f.facing);
                let angle = forward
                    .dot(self.rendezvous.remaining.normalized())
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                rows.push(("Cue error", format!("{angle:.1}°")));
                rows.push((
                    "Arrival braking Δv",
                    format!("{:.1} m/s", plan.arrival_delta_v.length()),
                ));
                if left < 0.25 {
                    "Cut throttle · burn complete"
                } else if f.throttle > 0.001 {
                    "Fly the burn · watch Δv remaining"
                } else if starts_in <= 0.0 {
                    "Begin your burn toward the cue"
                } else {
                    "Align with the burn cue"
                }
            };
            disabled(
                &self.doc,
                "rdv-warp",
                (if coasting {
                    plan.arrival_ut - ut
                        < (plan.arrival_delta_v.length() / acceleration.max(0.01) * 0.5 + 130.0)
                            .max(190.0)
                } else {
                    starts_in < 130.0
                }) || f.throttle > 0.001,
            );
            set_text(
                &self.doc,
                "rdv-warp",
                if coasting {
                    "Warp to encounter"
                } else {
                    "Warp to burn"
                },
            );
            self.rdv_render(instruction,rows,"Two-body estimate. You start and stop every burn. Replan after a significant burn error; no maneuver is scheduled.");
        } else {
            let Some((own_craft, target_craft, op, tp)) =
                own.as_ref().zip(station.as_ref()).and_then(|(oc, tc)| {
                    Some((
                        oc,
                        tc,
                        own_ports
                            .iter()
                            .find(|p| Some(&p.guid) == self.rendezvous.own_port.as_ref())?,
                        target_ports
                            .iter()
                            .find(|p| Some(&p.guid) == self.rendezvous.target_port.as_ref())?,
                    ))
                })
            else {
                self.rdv_phase(Phase::Unavailable);
                self.rdv_render(
                    "Choose a compatible pair of docking ports",
                    vec![],
                    "The craft documents may still be loading. A free compatible port is required.",
                );
                return;
            };
            if !op.free || !tp.free || !rdv::compatible(op, tp) {
                self.rdv_phase(Phase::Unavailable);
                self.rdv_render(
                    "Port occupied or incompatible",
                    vec![],
                    "Back away and explicitly select another free compatible port.",
                );
                return;
            }
            let own_offset =
                rdv::world_offset(op.position, f.centre_of_mass_m, f.facing, f.roll_ref);
            let target_offset =
                rdv::world_offset(tp.position, tf.centre_of_mass_m, tf.facing, tf.roll_ref);
            let own_axis = rdv::world_axis(op.axis, f.facing, f.roll_ref);
            let target_axis = rdv::world_axis(tp.axis, tf.facing, tf.roll_ref);
            let own_face = r + own_offset;
            let target_face = tr + target_offset;
            let offset = target_face - own_face;
            let port_velocity =
                (target_v + tf.angvel.cross(target_offset)) - (own_v + f.angvel.cross(own_offset));
            let ports = mechanics::relative(
                offset,
                port_velocity,
                // The streamed RCS rating sums opposing nozzles across all axes. Use a
                // conservative one-axis share for final translation/braking advice.
                if f.rcs_thrust_kn > 0.0 {
                    f.rcs_thrust_kn as f64 / (6.0 * f.mass_t.max(0.001))
                } else {
                    acceleration
                },
            )
            .unwrap();
            let stand = rdv::stand_off(
                rdv::radius(target_craft, &self.catalog, tf.centre_of_mass_m),
                rdv::radius(own_craft, &self.catalog, f.centre_of_mass_m),
                target_offset,
                target_axis,
            );
            let staging = target_face + target_axis * stand;
            let to_stage = staging - own_face;
            let axial = (own_face - target_face).dot(target_axis);
            let lateral = (own_face - target_face - target_axis * axial).length();
            if !self.rendezvous.dock_latched
                && ports.speed_mps < 1.0
                && (to_stage.length() < 15.0
                    || (axial > 0.0 && axial < stand + 20.0 && lateral < 2.0))
            {
                self.rendezvous.dock_latched = true;
            }
            let dock = self.rendezvous.dock_latched && ports.distance_m < stand * 3.0 + 100.0;
            self.rdv_phase(if dock { Phase::Dock } else { Phase::Approach });
            if rdv::obstructed(target_craft, &self.catalog, tp, stand) {
                self.rdv_phase(Phase::Unavailable);
                self.rdv_render("Port approach axis intersects station geometry",vec![],"Choose another port or reposition the station. Guidance cannot certify a safe path.");
                return;
            }
            let control = self
                .current_nav()
                .map(|n| DVec3::new(n.forward.x, n.forward.y, n.forward.z))
                .unwrap_or(f.facing);
            self.rendezvous.cue = if !dock && relative.speed_mps > 0.3 {
                Some((target_v - own_v).normalized())
            } else {
                Some(rdv::alignment_hold(control, own_axis, target_axis))
            };
            self.rendezvous.point = Some(bp + if dock { target_face } else { staging });
            self.rendezvous.axis_point = Some(bp + target_face);
            let angle = (-own_axis.dot(target_axis))
                .clamp(-1.0, 1.0)
                .acos()
                .to_degrees();
            let too_fast = ports.closing_mps > 0.0
                && ports
                    .stopping_distance_m
                    .is_none_or(|d| d + 10.0 >= ports.distance_m);
            let mut rows = vec![
                ("Port range", format!("{:.1} m", ports.distance_m)),
                ("Closing speed", format!("{:.2} m/s", ports.closing_mps)),
                ("Relative speed", format!("{:.2} m/s", ports.speed_mps)),
            ];
            let instruction = if dock {
                rows.push(("Lateral drift", format!("{:.2} m/s", ports.lateral_mps)));
                rows.push(("Port axis error", format!("{angle:.1}°")));
                rows.push(("Axis offset", format!("{lateral:.2} m")));
                hidden(&self.doc, "rdv-alignment", false);
                if let Some(dot) = self
                    .doc
                    .get_element_by_id("rdv-dot")
                    .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                {
                    let right = target_axis.cross(tf.roll_ref).normalized();
                    let up = right.cross(target_axis).normalized();
                    let across = own_face - target_face;
                    dot.style()
                        .set_property(
                            "left",
                            &format!("{}%", 50.0 + (across.dot(right) * 10.0).clamp(-40.0, 40.0)),
                        )
                        .ok();
                    dot.style()
                        .set_property(
                            "top",
                            &format!("{}%", 50.0 - (across.dot(up) * 10.0).clamp(-40.0, 40.0)),
                        )
                        .ok();
                }
                if too_fast && ports.speed_mps > 0.5 {
                    "Closing too fast · brake now"
                } else if angle > 3.0 {
                    "Align your port with the station port"
                } else if lateral > 0.3 {
                    "Use RCS to centre on the port axis"
                } else {
                    "Close gently · capture is automatic"
                }
            } else {
                rows.push(("Staging point", format!("{:.0} m", to_stage.length())));
                if let Some(s) = relative.brake_in_s {
                    rows.push((
                        "Begin braking in",
                        if s > 0.0 {
                            tracking::duration(s)
                        } else {
                            "NOW".into()
                        },
                    ));
                }
                if too_fast && ports.speed_mps > 0.5 {
                    "Closing too fast · brake now"
                } else if relative.speed_mps > 0.3 {
                    "Match velocity before closing in"
                } else {
                    "Use RCS toward the staging point"
                }
            };
            self.rdv_render(instruction,rows,"Staging uses catalog bounds. Check deployed arrays and hull clearance visually; RCS translation and docking mode remain manual.");
            let resource_note = if f.rcs_thrust_kn <= 0.0 {
                "No RCS thrust available. Fit thrusters and propellant before final approach."
            } else if !f.rcs {
                "RCS is off · enable RCS before translating."
            } else {
                ""
            };
            if !resource_note.is_empty() {
                set_text(&self.doc, "rdv-note", resource_note);
            }
        }
        if old_phase != self.rendezvous.phase {
            self.rdv_save();
        }
        let hold_name = if self.rendezvous.phase == Phase::Dock
            || (self.rendezvous.phase == Phase::Approach && relative.speed_mps <= 0.3)
        {
            "Hold port alignment"
        } else if self.rendezvous.phase == Phase::Approach {
            "Hold braking direction"
        } else {
            "Hold burn direction"
        };
        set_text(
            &self.doc,
            "rdv-hold",
            if self.rendezvous.hold {
                "Release guidance hold"
            } else {
                hold_name
            },
        );
        disabled(&self.doc, "rdv-hold", self.rendezvous.cue.is_none());
        if let Some(e) = self.doc.get_element_by_id("rdv-hold") {
            e.set_attribute(
                "aria-pressed",
                if self.rendezvous.hold {
                    "true"
                } else {
                    "false"
                },
            )
            .ok();
        }
        if self.rendezvous.hold && now_ms - self.rendezvous.hold_sent_ms > 300.0 {
            if let Some(cue) = self.rendezvous.cue {
                self.attitude = ose_net::AttitudeMode::Fixed(cue);
                self.send_controls();
                self.rendezvous.hold_sent_ms = now_ms;
            } else {
                self.rdv_release();
            }
        }
        if self.rendezvous.audible && self.rendezvous.phase == Phase::Intercept {
            if let Some(p) = self.rendezvous.transfer {
                let countdown = (p.departure_ut
                    - ut
                    - self.rendezvous.remaining.length() / self.rendezvous.acceleration.max(0.01)
                        * 0.5)
                    .ceil() as i64;
                if [30, 10, 5, 4, 3, 2, 1].contains(&countdown)
                    && self.rendezvous.last_prompt != Some(countdown)
                {
                    self.audio.play("ui_esc_open");
                    self.rendezvous.last_prompt = Some(countdown);
                }
            }
        }
        self.rdv_world();
        if self.warp_to.is_some()
            && self.requested_warp > self.time.warp
            && self.warp_holder.is_some_and(|p| Some(p) != self.player_id)
        {
            set_text(
                &self.doc,
                "rdv-note",
                "Shared time is limited by another pilot’s warp request. Countdown remains live.",
            );
        }
    }

    fn rdv_render(&self, instruction: &str, rows: Vec<(&str, String)>, help: &str) {
        set_text(&self.doc, "rdv-phase", self.rendezvous.phase.label());
        set_text(
            &self.doc,
            "rdv-summary",
            if self.rendezvous.enabled {
                self.rendezvous.phase.label()
            } else {
                "optional guidance"
            },
        );
        set_text(&self.doc, "rdv-instruction", instruction);
        set_text(&self.doc, "rdv-help", help);
        set_text(&self.doc, "rdv-note", &self.rendezvous.note);
        if let Some(e) = self.doc.get_element_by_id("rdv-panel") {
            e.set_attribute("data-phase", self.rendezvous.phase.label())
                .ok();
        }
        if let Some(e) = self.doc.get_element_by_id("rdv-values") {
            e.set_inner_html(
                &rows
                    .iter()
                    .map(|(k, v)| format!("<dt>{}</dt><dd>{}</dd>", esc(k), esc(v)))
                    .collect::<String>(),
            );
        }
        set_text(
            &self.doc,
            "rdv-plan",
            if self.rendezvous.enabled {
                "Replan rendezvous"
            } else {
                "Plan rendezvous"
            },
        );
    }
    fn rdv_world(&self) {
        #[cfg(target_arch = "wasm32")]
        if self.rendezvous.enabled && self.rendezvous.world_cues && !self.map_mode {
            for (id, at) in [
                ("rdv-world", self.flight3d.guidance_screen),
                ("rdv-axis", self.flight3d.port_screen),
            ] {
                if let Some((x, y)) = at {
                    if let Some(e) = self
                        .doc
                        .get_element_by_id(id)
                        .and_then(|e| e.dyn_into::<HtmlElement>().ok())
                    {
                        let rect = self.gpu_canvas.get_bounding_client_rect();
                        let x = x / self.dpr;
                        let y = y / self.dpr;
                        if x >= 0.0 && y >= 0.0 && x < rect.width() && y < rect.height() {
                            e.remove_attribute("hidden").ok();
                            e.style().set_property("left", &format!("{x}px")).ok();
                            e.style().set_property("top", &format!("{y}px")).ok();
                        }
                    }
                }
            }
            set_text(
                &self.doc,
                "rdv-world-label",
                match self.rendezvous.phase {
                    Phase::Dock => "Docking port axis",
                    Phase::Approach => "Port staging point",
                    _ => "Planned meeting point",
                },
            );
        }
    }
}
