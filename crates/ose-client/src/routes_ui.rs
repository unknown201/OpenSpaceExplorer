//! Delivery routes in the tracking station (`docs/delivery-routes-design.md`, step 3): the Routes tab lists
//! the campaign's routes (from KSC, to colonies, and depot-to-depot shuttles), the proving flights, and the
//! depots; a picked route's card shows its status, cargo and run log with Pause / Resume / Delete, what its
//! runs do after delivering, and Send home / Let go / Keep for the run it left docked. A finished recording's
//! card makes a route from it. The vessel card gains Mark as depot and Record route.
use ose_net::{DepotInfo, RecordingInfo, RouteEnd, RouteInfo};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// What is picked in the Routes tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pick {
    Route(u64),
    Recording(u64),
}

/// A duration in game time, short: "3 h 20 m", "12 m", "45 s" (a Kerbin day is 6 h).
pub fn span(s: f64) -> String {
    let s = s.max(0.0);
    if s >= 6.0 * 3600.0 {
        format!(
            "{:.0} d {:.0} h",
            (s / 21_600.0).floor(),
            (s % 21_600.0 / 3600.0).floor()
        )
    } else if s >= 3600.0 {
        format!(
            "{:.0} h {:.0} m",
            (s / 3600.0).floor(),
            (s % 3600.0 / 60.0).floor()
        )
    } else if s >= 60.0 {
        format!("{:.0} m", (s / 60.0).floor())
    } else {
        format!("{s:.0} s")
    }
}

fn amounts(list: &[(String, f64)]) -> String {
    if list.is_empty() {
        return "nothing".into();
    }
    list.iter()
        .map(|(k, v)| format!("{v:.1} {}", esc(k)))
        .collect::<Vec<_>>()
        .join(", ")
}

fn depot_name(depots: &[DepotInfo], id: u64) -> String {
    depots
        .iter()
        .find(|d| d.id == id)
        .map(|d| esc(&d.name))
        .unwrap_or_else(|| "a former depot".into())
}

/// Where a route's runs go, for its row and card.
fn destination(r: &RouteInfo, depots: &[DepotInfo]) -> String {
    match (r.origin, r.colony) {
        (Some(o), _) => format!(
            "{} \u{2192} {}",
            depot_name(depots, o),
            depot_name(depots, r.depot)
        ),
        (None, Some(_)) => "KSC \u{2192} colony".into(),
        (None, None) => format!("KSC \u{2192} {}", depot_name(depots, r.depot)),
    }
}

/// The list: routes, then proving flights, then depots.
pub fn render_list(
    routes: &[RouteInfo],
    recordings: &[RecordingInfo],
    depots: &[DepotInfo],
    pick: Option<Pick>,
) -> String {
    let mut html = String::new();
    html.push_str("<li class=\"ts-group\">Routes</li>");
    if routes.is_empty() {
        html.push_str(
            "<li class=\"ts-hint\" data-ose=\"route-none\">No routes yet. Mark a vessel as a depot, then \
             record a route: pick a craft on the pad (or one next to a depot, for a shuttle), press Record \
             route, and fly it there and dock.</li>",
        );
    }
    for r in routes {
        html.push_str(&format!(
            "<li class=\"ts-vessel route{sel}{paused}\" data-ose=\"ts-route\" data-ose-route=\"{id}\">\
             <span class=\"name\">{name}</span><span class=\"meta\">{dest} \u{b7} {status}</span></li>",
            sel = if pick == Some(Pick::Route(r.id)) { " selected" } else { "" },
            paused = if r.paused.is_some() { " paused" } else { "" },
            id = r.id,
            name = esc(&r.name),
            dest = destination(r, depots),
            status = esc(&r.status),
        ));
    }
    if !recordings.is_empty() {
        html.push_str("<li class=\"ts-group\">Proving flights</li>");
    }
    for p in recordings {
        let state = match p.state.as_str() {
            "Recording" => format!("recording, {}", p.phase.to_lowercase()),
            "OneWay" => "one way, ready".into(),
            "Landed" => "landed at a colony, ready".into(),
            "Dropped" => "dropped".into(),
            other => other.to_lowercase(),
        };
        html.push_str(&format!(
            "<li class=\"ts-vessel recording{sel}\" data-ose=\"ts-recording\" data-ose-recording=\"{id}\">\
             <span class=\"name\">{craft}</span><span class=\"meta\">{state}</span></li>",
            sel = if pick == Some(Pick::Recording(p.id)) {
                " selected"
            } else {
                ""
            },
            id = p.id,
            craft = esc(&p.craft),
        ));
    }
    if !depots.is_empty() {
        html.push_str("<li class=\"ts-group\">Depots</li>");
    }
    for d in depots {
        html.push_str(&format!(
            "<li class=\"ts-vessel depot\"{focus}><span class=\"name\">{name}</span>\
             <span class=\"meta\">{where_}</span></li>",
            focus = d
                .vessel
                .map(|v| format!(" data-ose-ts-focus=\"{v}\""))
                .unwrap_or_default(),
            name = esc(&d.name),
            where_ = if d.vessel.is_some() {
                "depot"
            } else {
                "out of sight"
            },
        ));
    }
    html
}

/// A route's card. `now` is the universe time, for the next launch.
pub fn render_route(r: &RouteInfo, depots: &[DepotInfo], now: f64) -> String {
    let mut rows = format!(
        "<dt>Route</dt><dd>{}</dd><dt>Status</dt><dd>{}</dd>",
        destination(r, depots),
        esc(&r.status)
    );
    if let Some(t) = r.next_launch_ut.filter(|t| *t > now) {
        rows.push_str(&format!(
            "<dt>Next launch</dt><dd>in {}</dd>",
            span(t - now)
        ));
    }
    if let Some(leg) = &r.shuttle_leg {
        rows.push_str(&format!(
            "<dt>Shuttle</dt><dd>{}</dd>",
            esc(&leg.to_lowercase())
        ));
    }
    rows.push_str(&format!("<dt>Cargo</dt><dd>{}</dd>", amounts(&r.cargo)));
    if r.manual.is_some() {
        rows.push_str("<dt>Run</dt><dd>flown by hand</dd>");
    }
    let delivered: f64 = r
        .runs
        .iter()
        .flat_map(|x| x.delivered.iter())
        .map(|(_, v)| v)
        .sum();
    rows.push_str(&format!(
        "<dt>Runs</dt><dd>{} ({} delivered, {:.1} units in all)</dd>",
        r.runs.len(),
        r.runs.iter().filter(|x| x.outcome == "Delivered").count(),
        delivered
    ));

    let mut log = String::new();
    for x in r.runs.iter().rev().take(8) {
        let why = x
            .reason
            .as_deref()
            .map(|w| format!(": {}", esc(w)))
            .unwrap_or_default();
        let after = x
            .after
            .as_deref()
            .map(|a| format!(", then {}", esc(a)))
            .unwrap_or_default();
        let by = if x.flown_by.is_empty() {
            String::new()
        } else {
            " (flown by hand)".into()
        };
        log.push_str(&format!(
            "<li class=\"run {cls}\">#{n} {outcome}{why} \u{b7} {got} \u{b7} {took}{after}{by}</li>",
            cls = x.outcome.to_lowercase(),
            n = x.n,
            outcome = esc(&x.outcome),
            got = amounts(&x.delivered),
            took = span(x.ended_ut - x.launched_ut),
        ));
    }
    if !log.is_empty() {
        log = format!("<ul class=\"route-log\">{log}</ul>");
    }

    // what its runs do after delivering: not for shuttles (they go round) or colonies (they stay)
    let mut end = String::new();
    if r.origin.is_none() && r.colony.is_none() {
        let opts = [
            (
                RouteEnd::Rotate,
                "Rotate",
                "stay until the next run is in orbit, then fly home",
            ),
            (RouteEnd::Stay, "Stay", "stay docked; each run takes a port"),
            (RouteEnd::Return, "Return", "undock and fly home"),
            (RouteEnd::LetGo, "Let go", "undock and drift"),
        ];
        end.push_str("<div class=\"route-end\">After delivering: ");
        for (when, label, title) in opts {
            end.push_str(&format!(
                "<button data-ose=\"route-end\" data-ose-route-end=\"{id}\" data-ose-end=\"{when:?}\" \
                 title=\"{title}\"{on}>{label}</button>",
                id = r.id,
                on = if r.when_delivered == when {
                    " class=\"on\""
                } else {
                    ""
                },
            ));
        }
        end.push_str("</div>");
        if let Some(n) = r.stayed {
            end.push_str(&format!(
                "<div class=\"route-stayed\">Run #{n} is docked at the depot: \
                 <button data-ose=\"route-stayed\" data-ose-route-stayed=\"{id}\" data-ose-what=\"SendHome\">\
                 Send home</button><button data-ose=\"route-stayed\" data-ose-route-stayed=\"{id}\" \
                 data-ose-what=\"LetGo\">Let go</button><button data-ose=\"route-stayed\" \
                 data-ose-route-stayed=\"{id}\" data-ose-what=\"Keep\">Keep</button></div>",
                id = r.id
            ));
        }
    }
    let take = r
        .run_vessel
        .map(|v| {
            format!(
                "<button data-ose=\"ts-fly\" data-ose-ts-fly=\"{v}\" title=\"Take control of the run; \
                 leave the flight to hand it back\">Take control</button>"
            )
        })
        .unwrap_or_default();
    format!(
        "<h3>{name}</h3><div class=\"ts-owner\">{kind}</div><dl>{rows}</dl>{log}{end}\
         <div class=\"ts-actions\"><button data-ose=\"route-pause\" data-ose-route-pause=\"{id}\" \
         data-ose-paused=\"{paused}\">{pause}</button>{take}<button data-ose=\"route-delete\" \
         data-ose-route-delete=\"{id}\">Delete</button></div>",
        name = esc(&r.name),
        kind = if r.origin.is_some() {
            "Shuttle route"
        } else {
            "Delivery route"
        },
        id = r.id,
        paused = r.paused.is_some(),
        pause = if r.paused.is_some() {
            "Resume"
        } else {
            "Pause"
        },
    )
}

/// A proving flight's card. `me` may stop their own recording.
pub fn render_recording(p: &RecordingInfo, me: Option<u64>, has_route: bool) -> String {
    let mut rows = format!(
        "<dt>Craft</dt><dd>{}</dd><dt>State</dt><dd>{}{}</dd>",
        esc(&p.craft),
        esc(&p.state),
        p.dropped
            .as_deref()
            .map(|w| format!(": {}", esc(w)))
            .unwrap_or_default()
    );
    if let Some(o) = &p.origin {
        rows.push_str(&format!("<dt>From</dt><dd>{} (a shuttle)</dd>", esc(o)));
    }
    if let Some(d) = &p.depot {
        rows.push_str(&format!("<dt>To</dt><dd>{}</dd>", esc(d)));
    }
    rows.push_str(&format!(
        "<dt>Flight</dt><dd>{}, {} burns</dd><dt>Used</dt><dd>{}</dd><dt>Delivered</dt><dd>{}</dd>",
        span(p.duration_s),
        p.burns,
        amounts(&p.used),
        amounts(&p.delivered)
    ));
    let mine = me == Some(p.player);
    let actions = match p.state.as_str() {
        "Recording" if mine => format!(
            "<button data-ose=\"rec-stop\" data-ose-rec-stop=\"{}\" title=\"Docked at the depot it ends as a \
             one-way route; anywhere else it is dropped\">Stop recording</button>",
            p.id
        ),
        "Recording" => String::new(),
        "OneWay" | "Landed" if !has_route => format!(
            "<input id=\"rt-name\" data-ose=\"rt-name\" placeholder=\"Route name\" maxlength=\"64\">\
             <button data-ose=\"rec-route\" data-ose-rec-route=\"{id}\">Create route</button>\
             <button data-ose=\"rec-forget\" data-ose-rec-forget=\"{id}\">Forget</button>",
            id = p.id
        ),
        _ if has_route => "<span class=\"ts-hint\">A route is made from it</span>".into(),
        _ => format!(
            "<button data-ose=\"rec-forget\" data-ose-rec-forget=\"{}\">Forget</button>",
            p.id
        ),
    };
    format!(
        "<h3>Proving flight</h3><div class=\"ts-owner\">{who}</div><dl>{rows}</dl>\
         <div class=\"ts-actions\">{actions}</div>",
        who = if mine {
            "Your recording"
        } else {
            "Another player's recording"
        },
    )
}

/// The routes buttons for a vessel's card: mark it as a depot (or not), and record a route with it.
pub fn vessel_actions(vessel: u64, is_depot: bool, can_fly: bool, recording: bool) -> String {
    if !can_fly {
        return String::new();
    }
    let record = if recording {
        String::new()
    } else {
        format!(
            "<button data-ose=\"rec-start\" data-ose-rec-start=\"{vessel}\" title=\"Record its flight to a \
             depot as a delivery route: from the pad, or from next to a depot for a shuttle\">Record route\
             </button>"
        )
    };
    format!(
        "<div class=\"ts-actions routes\"><button data-ose=\"depot-toggle\" data-ose-depot=\"{vessel}\" \
         data-ose-depot-on=\"{on}\">{label}</button>{record}</div>",
        on = !is_depot,
        label = if is_depot {
            "Unmark depot"
        } else {
            "Mark as depot"
        },
    )
}

/// A toast for a route's new run outcome.
pub fn run_toast(r: &RouteInfo) -> Option<(String, &'static str)> {
    let x = r.runs.last()?;
    let tier = match x.outcome.as_str() {
        "Delivered" => "passive",
        "Short" => "passive",
        _ => "warning",
    };
    let text = match x.outcome.as_str() {
        "Delivered" | "Short" => format!(
            "{} #{} delivered {}{}",
            r.name,
            x.n,
            amounts(&x.delivered).replace("&amp;", "&"),
            if x.outcome == "Short" { " (short)" } else { "" }
        ),
        other => format!(
            "{} #{} {}: {}",
            r.name,
            x.n,
            other.to_lowercase(),
            x.reason.clone().unwrap_or_default()
        ),
    };
    Some((text, tier))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ose_net::RouteRunInfo;

    fn route() -> RouteInfo {
        RouteInfo {
            id: 7,
            name: "Fuel <run>".into(),
            owner: 1,
            recording: 3,
            depot: 2,
            colony: None,
            status: "Next launch in 300 s".into(),
            paused: None,
            next_launch_ut: Some(1300.0),
            run_vessel: None,
            cargo: vec![("MonoPropellant".into(), 3.85)],
            when_delivered: RouteEnd::Rotate,
            stayed: Some(1),
            manual: None,
            origin: None,
            shuttle_leg: None,
            runs: vec![RouteRunInfo {
                n: 1,
                vessel_name: "Fuel run 1".into(),
                launched_ut: 0.0,
                ended_ut: 3700.0,
                outcome: "Delivered".into(),
                reason: None,
                delivered: vec![("MonoPropellant".into(), 3.85)],
                after: Some("docked".into()),
                flown_by: vec![],
            }],
        }
    }

    fn depots() -> Vec<DepotInfo> {
        vec![DepotInfo {
            id: 2,
            name: "Station".into(),
            owner: 1,
            vessel: Some(40),
        }]
    }

    #[test]
    fn a_route_card_has_its_controls() {
        let html = render_route(&route(), &depots(), 1000.0);
        assert!(html.contains("Fuel &lt;run&gt;"), "escaped");
        assert!(html.contains("KSC \u{2192} Station"));
        assert!(html.contains("in 5 m"), "{html}");
        assert!(html.contains("data-ose-route-pause=\"7\""));
        assert!(html.contains("data-ose-end=\"Rotate\" title=\"stay until the next run is in orbit, then fly home\" class=\"on\""));
        assert!(
            html.contains("data-ose-what=\"SendHome\""),
            "the run left docked"
        );
        assert!(html.contains("#1 Delivered"));
        assert!(!html.contains("Take control"), "no run in flight");
    }

    #[test]
    fn a_shuttle_card_has_no_end_modes() {
        let mut r = route();
        r.origin = Some(2);
        r.depot = 9;
        r.shuttle_leg = Some("Outbound".into());
        r.run_vessel = Some(55);
        let html = render_route(&r, &depots(), 0.0);
        assert!(html.contains("Shuttle route"));
        assert!(html.contains("Station \u{2192} a former depot"));
        assert!(!html.contains("route-end"));
        assert!(
            html.contains("data-ose-ts-fly=\"55\""),
            "take control of the shuttle"
        );
    }

    #[test]
    fn a_finished_recording_makes_a_route_once() {
        let p = RecordingInfo {
            id: 3,
            player: 1,
            craft: "Tanker".into(),
            vessel: None,
            phase: "Docked".into(),
            state: "OneWay".into(),
            dropped: None,
            depot: Some("Station".into()),
            duration_s: 7300.0,
            used: vec![],
            delivered: vec![("MonoPropellant".into(), 3.85)],
            payload_parts: 0,
            burns: 4,
            origin: None,
        };
        assert!(render_recording(&p, Some(1), false).contains("data-ose-rec-route=\"3\""));
        assert!(!render_recording(&p, Some(1), true).contains("data-ose-rec-route"));
        let list = render_list(&[route()], &[p], &depots(), Some(Pick::Recording(3)));
        assert!(list.contains("recording selected"));
        assert!(
            list.contains("data-ose-ts-focus=\"40\""),
            "a depot focuses its vessel"
        );
    }

    #[test]
    fn spans_read_in_kerbin_days() {
        assert_eq!(span(45.0), "45 s");
        assert_eq!(span(3700.0), "1 h 1 m");
        assert_eq!(span(30_000.0), "1 d 2 h");
    }

    #[test]
    fn toasts_say_what_a_run_did() {
        let (t, tier) = run_toast(&route()).unwrap();
        assert_eq!(t, "Fuel <run> #1 delivered 3.9 MonoPropellant");
        assert_eq!(tier, "passive");
    }
}
