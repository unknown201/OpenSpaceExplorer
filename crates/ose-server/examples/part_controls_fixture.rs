//! Seed a disposable local browser session: cargo run -p ose-server --example part_controls_fixture -- 127.0.0.1:7797
use futures_util::{SinkExt, StreamExt};
use ose_net::{ClientMessage as C, ServerMessage as S};
use tokio_tungstenite::tungstenite::Message;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let addr = std::env::args().nth(1).unwrap_or("127.0.0.1:7797".into());
    anyhow::ensure!(
        addr.starts_with("127.0.0.1:") || addr.starts_with("localhost:"),
        "local test servers only"
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await?;
    ws.send(Message::Binary(
        ose_net::encode(&C::Hello {
            protocol: ose_net::PROTOCOL_VERSION,
            token: None,
            display_name: "Part control audit".into(),
        })?
        .into(),
    ))
    .await?;
    let token = loop {
        if let Some(Ok(Message::Binary(b))) =
            tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await?
        {
            if let S::Welcome { token, .. } = ose_net::decode::<S>(&b)? {
                break token;
            }
        } else {
            anyhow::bail!("server closed before Welcome");
        }
    };
    ws.send(Message::Binary(
        ose_net::encode(&C::CreateSession {
            name: "Part controls audit".into(),
            mode: ose_net::SessionMode::Sandbox,
            difficulty: vec![],
        })?
        .into(),
    ))
    .await?;
    let campaign = loop {
        if let Some(Ok(Message::Binary(b))) =
            tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await?
        {
            if let S::CampaignState { id, .. } = ose_net::decode::<S>(&b)? {
                break id;
            }
        } else {
            anyhow::bail!("server closed before CampaignState");
        }
    };
    let defs: Vec<ose_data::PartDef> =
        serde_json::from_str(include_str!("../../../assets/own/parts-catalog.json"))?;
    let ids = [
        "pod_1v_conical_crew",
        "fueltank_1v_inline_methalox_1x2",
        "landingleg_0v",
        "antenna_0v_16",
        "solararray_1v_shielded_1x6",
        "solararray_1v_1x6",
        "light_0v_spot",
        "light_0v_bulb",
        "engine_1v_methalox_swivel",
    ];
    let parts = ids
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let def = defs.iter().find(|d| d.id == *id).unwrap();
            let pos = match i {
                0 => [0., 1.5, 0.],
                1 => [0., 0., 0.],
                2 => [0.65, -0.3, 0.],
                3 => [-0.65, 0.3, 0.],
                4 => [0., 0.2, 0.65],
                5 => [0., 0.2, -0.65],
                6 => [0.7, 0.5, 0.5],
                7 => [-0.7, 0.5, 0.5],
                _ => [0., -1.5, 0.],
            };
            ose_data::craft::CraftPart {
                guid: format!("control-{i}"),
                part_id: (*id).into(),
                parent: (i > 0).then_some(if i == 1 { 0 } else { 1 }),
                own_node: None,
                parent_node: None,
                local_position: ose_math::DVec3::new(pos[0], pos[1], pos[2]),
                local_rotation: ose_data::craft::Quat {
                    w: 1.,
                    ..Default::default()
                },
                stage_index: 0,
                fuel_crossfeed: true,
                resources: def
                    .resource_containers
                    .iter()
                    .map(|r| ose_data::craft::StoredResource {
                        resource: r.resource.clone(),
                        stored_units: r.capacity_units,
                        capacity_units: r.capacity_units,
                    })
                    .collect(),
                fuel_line_target: None,
            }
        })
        .collect();
    let craft = ose_data::craft::Craft {
        name: "Part controls audit".into(),
        description: "Light, leg, antenna, reversible and one-shot solar panel".into(),
        parts,
        stages: vec![vec![8]],
    };
    ws.send(Message::Binary(
        ose_net::encode(&C::SaveCraft { craft })?.into(),
    ))
    .await?;
    loop {
        if let Some(Ok(Message::Binary(b))) =
            tokio::time::timeout(std::time::Duration::from_secs(10), ws.next()).await?
        {
            if let S::CraftSaved { id, reason, .. } = ose_net::decode::<S>(&b)? {
                anyhow::ensure!(id.is_some(), "{reason:?}");
                std::fs::create_dir_all(".cache/part-controls")?;
                std::fs::write(
                    ".cache/part-controls/fixture.json",
                    serde_json::to_vec_pretty(
                        &serde_json::json!({"token":token,"craft":id,"campaign":campaign}),
                    )?,
                )?;
                break;
            }
        } else {
            anyhow::bail!("server closed before CraftSaved");
        }
    }
    Ok(())
}
