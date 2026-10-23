//! Wire protocol between client and server. Encoded with `postcard`.

#![forbid(unsafe_code)]

use ose_data::Tick;
use serde::{Deserialize, Serialize};

pub const PROTOCOL_VERSION: u16 = 1;

/// Client to server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMessage {
    Hello {
        protocol: u16,
        token: Option<String>,
        display_name: String,
    },
    RequestWarp {
        factor: u32,
    },
    Ping {
        sent_tick: Tick,
    },
}

/// Server to client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMessage {
    Welcome {
        player_id: u64,
        token: String,
        tick: Tick,
    },
    Rejected {
        reason: String,
    },
    TimeState {
        tick: Tick,
        warp: u32,
        warp_limited_by: Option<u64>,
    },
    Pong {
        sent_tick: Tick,
    },
}

pub fn encode<T: Serialize>(msg: &T) -> Result<Vec<u8>, postcard::Error> {
    postcard::to_allocvec(msg)
}

pub fn decode<'a, T: Deserialize<'a>>(bytes: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn messages_round_trip() {
        let m = ServerMessage::TimeState {
            tick: Tick(42),
            warp: 1000,
            warp_limited_by: Some(7),
        };
        let bytes = encode(&m).unwrap();
        let back: ServerMessage = decode(&bytes).unwrap();
        assert_eq!(m, back);
    }
}
