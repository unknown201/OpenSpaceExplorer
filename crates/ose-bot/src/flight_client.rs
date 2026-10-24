//! Client calls for sessions, the craft library and powered flight (protocol 13).

use crate::client::{Client, Launched};
use anyhow::{bail, Result};
use ose_net::{AttitudeMode, ClientMessage, CraftSummary, ServerMessage, SessionMode};
use std::time::Duration;

/// The campaign as `CreateSession` / `JoinCampaign` left it.
#[derive(Clone, Debug, PartialEq)]
pub struct Session {
    pub name: String,
    pub mode: SessionMode,
    pub you_are_member: bool,
    pub created: bool,
    pub members: usize,
}

fn session_of(m: &ServerMessage) -> Option<Session> {
    match m {
        ServerMessage::CampaignState {
            name,
            mode,
            members,
            you_are_member,
            created,
            ..
        } => Some(Session {
            name: name.clone(),
            mode: *mode,
            you_are_member: *you_are_member,
            created: *created,
            members: members.len(),
        }),
        _ => None,
    }
}

fn launched_of(m: &ServerMessage) -> Option<Result<Launched>> {
    match m {
        ServerMessage::CraftLaunched {
            vessel,
            name,
            parts,
            wet_mass_t,
            dry_mass_t,
            delta_v_vacuum_mps,
            reason,
            ..
        } => Some(match vessel {
            Some(v) => Ok(Launched {
                vessel: *v,
                name: name.clone(),
                parts: *parts,
                wet_mass_t: *wet_mass_t,
                dry_mass_t: *dry_mass_t,
                delta_v_vacuum_mps: *delta_v_vacuum_mps,
            }),
            None => Err(anyhow::anyhow!(
                "server refused the craft {name:?}: {}",
                reason.clone().unwrap_or_else(|| "no reason given".into())
            )),
        }),
        _ => None,
    }
}

impl Client {
    /// Configure the shared campaign (only works while nobody has joined it) and return its state.
    pub async fn create_session(&mut self, name: &str, mode: SessionMode) -> Result<Session> {
        self.send(&ClientMessage::CreateSession {
            name: name.into(),
            mode,
            difficulty: Vec::new(),
        })
        .await?;
        self.wait_for(Duration::from_secs(10), "CampaignState", |_, m| {
            session_of(m)
        })
        .await
    }

    /// Join the shared campaign and return its state with the mode.
    pub async fn join_session(&mut self) -> Result<Session> {
        self.send(&ClientMessage::JoinCampaign).await?;
        let s = self
            .wait_for(Duration::from_secs(10), "CampaignState", |_, m| {
                session_of(m)
            })
            .await?;
        if !s.you_are_member {
            bail!("server answered JoinCampaign but did not list this player as a member");
        }
        Ok(s)
    }

    pub async fn craft_list(&mut self) -> Result<Vec<CraftSummary>> {
        self.send(&ClientMessage::CraftList).await?;
        self.wait_for(Duration::from_secs(10), "CraftList", |_, m| match m {
            ServerMessage::CraftList { crafts } => Some(crafts.clone()),
            _ => None,
        })
        .await
    }

    /// Put a listed craft on the KSC launch pad.
    pub async fn launch_saved_on_pad(&mut self, id: u64) -> Result<Launched> {
        self.send(&ClientMessage::LaunchSavedOnPad { id }).await?;
        self.wait_for(Duration::from_secs(20), "CraftLaunched", |_, m| {
            launched_of(m)
        })
        .await?
    }

    /// Put a listed craft straight into a circular orbit (the fallback when the ascent fails).
    pub async fn launch_saved_to_orbit(&mut self, id: u64, altitude_m: f64) -> Result<Launched> {
        self.send(&ClientMessage::LaunchSavedCraft { id, altitude_m })
            .await?;
        self.wait_for(Duration::from_secs(20), "CraftLaunched", |_, m| {
            launched_of(m)
        })
        .await?
    }

    /// Recover a vessel standing on the home world (it is removed). Returns whether the server recovered it.
    pub async fn recover_vessel(&mut self, vessel: u64) -> Result<bool> {
        self.send(&ClientMessage::RecoverVessel { vessel }).await?;
        let (recovered, reason) = self
            .wait_for(
                Duration::from_secs(10),
                "ScienceSubmitted",
                |_, m| match m {
                    ServerMessage::ScienceSubmitted {
                        vessel: v,
                        recovered,
                        reason,
                        ..
                    } if *v == vessel => Some((*recovered, reason.clone())),
                    _ => None,
                },
            )
            .await?;
        if !recovered {
            bail!(
                "server did not recover vessel {vessel}: {}",
                reason.unwrap_or_else(|| "no reason given".into())
            );
        }
        Ok(true)
    }

    /// Send throttle and attitude without waiting for the acknowledgement (the stream carries the result).
    pub async fn set_controls(
        &mut self,
        vessel: u64,
        throttle: f64,
        attitude: AttitudeMode,
    ) -> Result<()> {
        self.send(&ClientMessage::SetControls {
            vessel,
            throttle: throttle.clamp(0.0, 1.0),
            attitude,
        })
        .await
    }

    /// Fire the next stage without waiting for the acknowledgement.
    pub async fn stage(&mut self, vessel: u64) -> Result<()> {
        self.send(&ClientMessage::Stage { vessel }).await
    }
}
