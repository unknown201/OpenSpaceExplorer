//! The tracking station's Colonies tab (`docs/colonies-design.md`, step 2): the campaign's colonies, and for
//! the one picked its stock (fuel and Metal), residents, vessels and launch pad. Its founder can abandon it.
use ose_net::ColonyInfo;

fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Units in a short form: 12 345 -> "12.3k".
fn units(u: f64) -> String {
    if u >= 10_000.0 {
        format!("{:.1}k", u / 1000.0)
    } else if u >= 100.0 {
        format!("{u:.0}")
    } else {
        format!("{u:.1}")
    }
}

/// The colony list, as the station's vessel list draws its rows.
pub fn render_list(colonies: &[ColonyInfo], selected: Option<u64>) -> String {
    if colonies.is_empty() {
        return "<li class=\"ts-hint\" data-ose=\"colony-none\">No colonies yet. Land a crewed vessel with a \
                colony trunk away from Kerbin's space center and press Found colony here in the trunk's \
                part window.</li>"
            .into();
    }
    colonies
        .iter()
        .map(|c| {
            format!(
                "<li class=\"ts-vessel colony{sel}\" data-ose=\"ts-colony\" data-ose-colony=\"{id}\">\
                 <span class=\"name\">{name}</span><span class=\"meta\">{body} \u{b7} {n} vessels \u{b7} \
                 {k} Kerbals</span></li>",
                sel = if Some(c.id) == selected { " selected" } else { "" },
                id = c.id,
                name = esc(&c.name),
                body = esc(&c.body),
                n = c.vessels.len(),
                k = c.residents.len(),
            )
        })
        .collect()
}

/// One colony's card. `me` is the viewing player, who may abandon a colony they founded.
pub fn render_info(c: &ColonyInfo, me: Option<u64>) -> String {
    let mut stock = String::new();
    let mut shown: Vec<&(String, f64, f64)> = c.stock.iter().collect();
    // Metal first: it pays for every part
    shown.sort_by_key(|s| (s.0 != ose_data::colony::METAL, s.0.clone()));
    for (res, stored, cap) in shown {
        let tonnes = if res == ose_data::colony::METAL {
            format!(" ({:.1} t)", stored * ose_data::colony::METAL_UNIT_T)
        } else {
            String::new()
        };
        stock.push_str(&format!(
            "<dt>{}</dt><dd>{} / {}{tonnes}</dd>",
            esc(res),
            units(*stored),
            units(*cap)
        ));
    }
    if stock.is_empty() {
        stock = "<dt>Stock</dt><dd>empty: land tankers and trunks within 300 m</dd>".into();
    }
    let residents = if c.residents.is_empty() {
        "none".to_string()
    } else {
        c.residents
            .iter()
            .map(|r| esc(r))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // a Kerbin day is 6 h
    let newcomers = match (c.vacancies, c.next_arrival_in_s) {
        (0, _) => "no room: every seat is taken".to_string(),
        (_, None) => "nobody lives here to welcome them".to_string(),
        (n, Some(t)) if t >= 21_600.0 => {
            format!("next in {:.1} days ({n} empty seats)", t / 21_600.0)
        }
        (n, Some(t)) => format!("next in {:.1} h ({n} empty seats)", t / 3600.0),
    };
    // the same field the server's drills dig from, so the card can say what a drill here would make
    let share = ose_data::abundance::ore(&c.body, c.lat_deg, c.lon_deg);
    let ore = if share <= 0.0 {
        "none here".to_string()
    } else {
        let rate = ose_data::colony::MINING
            .iter()
            .find(|m| m.id == ose_data::colony::DRILL)
            .and_then(|m| (m.converter)().outputs.first().map(|o| o.1))
            .unwrap_or(0.0)
            * share;
        format!("{:.0}% (a drill digs {rate:.2} Ore/s)", share * 100.0)
    };
    let abandon = if me == Some(c.owner) {
        format!(
            "<button data-ose=\"colony-abandon\" data-ose-colony-abandon=\"{}\">Abandon colony</button>",
            c.id
        )
    } else {
        String::new()
    };
    format!(
        "<h3>{name}</h3><div class=\"ts-owner\">Founded by {owner}</div><dl>\
         <dt>Body</dt><dd>{body}</dd><dt>Site</dt><dd>{lat:.3}\u{b0}, {lon:.3}\u{b0}</dd>\
         <dt>Vessels</dt><dd>{n}</dd><dt>Residents</dt><dd>{residents}</dd>\
         <dt>Newcomers</dt><dd>{arrivals} moved in; {newcomers}</dd>\
         <dt>Ore in the ground</dt><dd>{ore}</dd>\
         <dt>Launches</dt><dd>{launches}</dd>{stock}</dl>\
         <div class=\"ts-hint\">Its launch pad is in the VAB's launch sites. A craft launched there takes its \
         fuel and its dry mass in Metal from this stock.</div>\
         <div class=\"ts-actions\">{abandon}</div>",
        name = esc(&c.name),
        owner = esc(&c.owner_name),
        body = esc(&c.body),
        lat = c.lat_deg,
        lon = c.lon_deg,
        n = c.vessels.len(),
        launches = c.launches,
        arrivals = c.arrivals,
    )
}

/// A colony's map marker tooltip: its name, whose it is, and who lives there.
pub fn map_tooltip(c: &ColonyInfo, mine: bool) -> String {
    let whose = if mine {
        "yours".to_string()
    } else {
        c.owner_name.clone()
    };
    format!(
        "{} \u{b7} colony on the {}\n{whose}\n{} Kerbals \u{b7} {} vessels",
        c.name,
        c.body,
        c.residents.len(),
        c.vessels.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn colony() -> ColonyInfo {
        ColonyInfo {
            id: 3,
            name: "Armstrong <base>".into(),
            owner: 7,
            owner_name: "Josh".into(),
            body: "Mun".into(),
            lat_deg: 1.0,
            lon_deg: 20.0,
            founded_ut: 0.0,
            launches: 2,
            vessels: vec![10, 11],
            residents: vec!["Bill Kerman".into()],
            stock: vec![
                ("MonoPropellant".into(), 4.5, 9.0),
                ("Metal".into(), 7_500.0, 10_000.0),
            ],
            vacancies: 1,
            arrivals: 0,
            next_arrival_in_s: Some(43_200.0),
        }
    }

    #[test]
    fn the_card_lists_metal_first_and_only_the_founder_may_abandon() {
        let c = colony();
        let html = render_info(&c, Some(7));
        assert!(html.contains("Armstrong &lt;base&gt;"));
        assert!(html.find("Metal").unwrap() < html.find("MonoPropellant").unwrap());
        assert!(html.contains("7500 / 10.0k (7.5 t)"), "{html}");
        assert!(html.contains("next in 2.0 days (1 empty seats)"), "{html}");
        assert!(html.contains("data-ose-colony-abandon=\"3\""));
        let share = ose_data::abundance::ore("Mun", 1.0, 20.0);
        assert!(
            html.contains(&format!("{:.0}% (a drill", share * 100.0)) || share == 0.0,
            "{html}"
        );
        let jool = ColonyInfo {
            body: "Jool".into(),
            ..c.clone()
        };
        assert!(render_info(&jool, None).contains("Ore in the ground</dt><dd>none here"));
        assert!(!render_info(&c, Some(8)).contains("colony-abandon"));
        let list = render_list(&[c], Some(3));
        assert!(
            list.contains("selected") && list.contains("2 vessels") && list.contains("1 Kerbals")
        );
        assert!(render_list(&[], None).contains("No colonies yet"));
        let tip = map_tooltip(&colony(), false);
        assert_eq!(
            tip,
            "Armstrong <base> \u{b7} colony on the Mun\nJosh\n1 Kerbals \u{b7} 2 vessels"
        );
    }
}
