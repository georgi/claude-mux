//! WebSocket protocol for `/v1/stream` (architecture 6.2).
//!
//! One socket per client. Text frames carry JSON ([`ClientMsg`] up,
//! [`ServerMsg`] down), tagged with `"type"`. Binary frames carry terminal
//! bytes ([`Frame`]).
//!
//! Handshake: the client sends `hello` first (with `token` unless a cookie
//! authenticated the upgrade). The server answers `welcome`, then `state` (when
//! `since_rev` is missing or too old) or `deltas`, then for each requested
//! terminal a binary frame: `Output` with the missed bytes if `since_seq` is
//! still in the ring, else `Snapshot`.
//!
//! Terminal input is a binary [`Frame::Input`], not a JSON message, so
//! keystrokes cost no escaping.
//!
//! Chat: `sub_chat` answers with one `chat` message holding the entries after
//! `after` (the whole kept conversation without it), then one `chat` message
//! per batch of new entries until `unsub_chat`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::api::{ErrorCode, Scope, ServerInfo};
use crate::view::{AgentId, ChatEntry, ClientId, DeltaView, StateView, TermId};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum ClientKind {
    Tui,
    Web,
    Cli,
}

/// A terminal the client already has bytes of, up to (not including) `seq`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TermPos {
    pub id: TermId,
    pub since_seq: u64,
}

/// Client to server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMsg {
    Hello {
        client: ClientKind,
        /// Last `rev` seen; absent means "send me the full state".
        #[serde(default)]
        since_rev: Option<u64>,
        /// Device token, when no cookie authenticated the upgrade. Never in the URL.
        #[serde(default)]
        token: Option<String>,
        /// Free-form client description, e.g. `cmux 0.1.0 (linux)`.
        #[serde(default)]
        info: Option<String>,
        /// Terminals to resume at once, as after a reconnect.
        #[serde(default)]
        terms: Vec<TermPos>,
    },
    /// Start streaming a terminal. Without `since_seq` the server sends a snapshot.
    Subscribe {
        term: TermId,
        #[serde(default)]
        since_seq: Option<u64>,
    },
    Unsubscribe {
        term: TermId,
    },
    /// This client's focus makes it the size owner of `term`.
    Focus {
        term: TermId,
    },
    Resize {
        term: TermId,
        rows: u16,
        cols: u16,
    },
    Ping {
        #[serde(default)]
        nonce: u64,
    },
    /// Start streaming an agent's conversation. `after` is the uuid of the
    /// last entry the client has.
    SubChat {
        agent: AgentId,
        #[serde(default)]
        after: Option<String>,
    },
    UnsubChat {
        agent: AgentId,
    },
}

/// Server to client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMsg {
    Welcome {
        rev: u64,
        server: ServerInfo,
        client_id: ClientId,
        scope: Scope,
    },
    /// A full snapshot, when `since_rev` is missing or too old.
    State {
        rev: u64,
        snapshot: StateView,
    },
    /// In order, with no gaps after the client's `since_rev`.
    Deltas {
        rev: u64,
        items: Vec<DeltaView>,
    },
    Size {
        term: TermId,
        rows: u16,
        cols: u16,
        owner: Option<ClientId>,
    },
    Presence {
        clients: Vec<PresenceEntry>,
    },
    /// The program in a subscribed terminal ended. Sent after its last output;
    /// no more `Output` follows for it.
    Exit {
        term: TermId,
        code: u32,
        #[serde(default)]
        signal: Option<String>,
    },
    Error {
        code: ErrorCode,
        message: String,
    },
    /// Conversation entries of a `sub_chat` agent, oldest first.
    Chat {
        agent: AgentId,
        entries: Vec<ChatEntry>,
    },
    Pong {
        #[serde(default)]
        nonce: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct PresenceEntry {
    pub id: ClientId,
    pub kind: ClientKind,
    /// The paired device's name, if known.
    pub device: Option<String>,
}

// ---------------------------------------------------------------------------
// Binary frames
// ---------------------------------------------------------------------------

/// Frame kinds (first byte).
pub const KIND_INPUT: u8 = 0x01;
pub const KIND_OUTPUT: u8 = 0x02;
pub const KIND_SNAPSHOT: u8 = 0x03;

/// `[kind u8][term u32 BE]` for input, plus `[seq u64 BE]` for output and
/// snapshot frames; the payload follows.
pub const INPUT_HEADER: usize = 1 + 4;
pub const OUTPUT_HEADER: usize = 1 + 4 + 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Frame {
    /// Client to server: keyboard (or paste, or mouse) bytes for a PTY.
    Input { term: TermId, data: Vec<u8> },
    /// Server to client: live PTY bytes. `seq` is the stream offset of
    /// `data[0]`; the next frame for this terminal starts at `seq + len`.
    Output {
        term: TermId,
        seq: u64,
        data: Vec<u8>,
    },
    /// Server to client: escape sequences that redraw the screen, cursor and
    /// modes from scratch (reset the local parser first). `seq` is the offset
    /// where live `Output` resumes.
    Snapshot {
        term: TermId,
        seq: u64,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FrameError {
    Empty,
    UnknownKind(u8),
    /// The header is cut short.
    Truncated {
        need: usize,
        got: usize,
    },
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FrameError::Empty => f.write_str("empty frame"),
            FrameError::UnknownKind(k) => write!(f, "unknown frame kind {k:#04x}"),
            FrameError::Truncated { need, got } => {
                write!(f, "truncated frame header: need {need} bytes, got {got}")
            }
        }
    }
}

impl std::error::Error for FrameError {}

impl Frame {
    pub fn term(&self) -> TermId {
        match self {
            Frame::Input { term, .. }
            | Frame::Output { term, .. }
            | Frame::Snapshot { term, .. } => *term,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let (kind, term, seq, data) = match self {
            Frame::Input { term, data } => (KIND_INPUT, term, None, data),
            Frame::Output { term, seq, data } => (KIND_OUTPUT, term, Some(*seq), data),
            Frame::Snapshot { term, seq, data } => (KIND_SNAPSHOT, term, Some(*seq), data),
        };
        let mut out = Vec::with_capacity(OUTPUT_HEADER + data.len());
        out.push(kind);
        out.extend_from_slice(&term.0.to_be_bytes());
        if let Some(seq) = seq {
            out.extend_from_slice(&seq.to_be_bytes());
        }
        out.extend_from_slice(data);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Frame, FrameError> {
        let kind = *buf.first().ok_or(FrameError::Empty)?;
        let need = match kind {
            KIND_INPUT => INPUT_HEADER,
            KIND_OUTPUT | KIND_SNAPSHOT => OUTPUT_HEADER,
            k => return Err(FrameError::UnknownKind(k)),
        };
        if buf.len() < need {
            return Err(FrameError::Truncated {
                need,
                got: buf.len(),
            });
        }
        let term = TermId(u32::from_be_bytes(buf[1..5].try_into().unwrap()));
        if kind == KIND_INPUT {
            return Ok(Frame::Input {
                term,
                data: buf[need..].to_vec(),
            });
        }
        let seq = u64::from_be_bytes(buf[5..13].try_into().unwrap());
        let data = buf[need..].to_vec();
        Ok(if kind == KIND_OUTPUT {
            Frame::Output { term, seq, data }
        } else {
            Frame::Snapshot { term, seq, data }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::view::StateView;

    fn rt<T: Serialize + for<'a> Deserialize<'a> + PartialEq + std::fmt::Debug>(v: T) -> String {
        let s = serde_json::to_string(&v).unwrap();
        assert_eq!(serde_json::from_str::<T>(&s).unwrap(), v, "{s}");
        s
    }

    #[test]
    fn client_msgs_round_trip() {
        let hello = ClientMsg::Hello {
            client: ClientKind::Tui,
            since_rev: Some(7),
            token: Some("t".into()),
            info: Some("cmux 0.1".into()),
            terms: vec![TermPos {
                id: TermId(1),
                since_seq: 99,
            }],
        };
        assert!(rt(hello).contains(r#""type":"hello""#));
        rt(ClientMsg::Subscribe {
            term: TermId(2),
            since_seq: None,
        });
        rt(ClientMsg::Subscribe {
            term: TermId(2),
            since_seq: Some(5),
        });
        rt(ClientMsg::Unsubscribe { term: TermId(2) });
        rt(ClientMsg::Focus { term: TermId(2) });
        rt(ClientMsg::Resize {
            term: TermId(2),
            rows: 24,
            cols: 80,
        });
        rt(ClientMsg::Ping { nonce: 3 });
        let agent: AgentId = "P-20261018-demo/T-0001".parse().unwrap();
        let sub = rt(ClientMsg::SubChat {
            agent: agent.clone(),
            after: Some("u-1".into()),
        });
        assert!(sub.contains(r#""type":"sub_chat""#), "{sub}");
        rt(ClientMsg::UnsubChat { agent });
        // `after` may be left out.
        let m: ClientMsg =
            serde_json::from_str(r#"{"type":"sub_chat","agent":"P-20261018-demo/executor"}"#)
                .unwrap();
        assert!(matches!(m, ClientMsg::SubChat { after: None, .. }));
    }

    #[test]
    fn minimal_hello_and_ping_parse() {
        let m: ClientMsg = serde_json::from_str(r#"{"type":"hello","client":"web"}"#).unwrap();
        assert_eq!(
            m,
            ClientMsg::Hello {
                client: ClientKind::Web,
                since_rev: None,
                token: None,
                info: None,
                terms: vec![]
            }
        );
        let p: ClientMsg = serde_json::from_str(r#"{"type":"ping"}"#).unwrap();
        assert_eq!(p, ClientMsg::Ping { nonce: 0 });
        assert!(serde_json::from_str::<ClientMsg>(r#"{"type":"nope"}"#).is_err());
        assert!(serde_json::from_str::<ClientMsg>(r#"{"client":"web"}"#).is_err());
    }

    #[test]
    fn server_msgs_round_trip() {
        rt(ServerMsg::Welcome {
            rev: 1,
            server: ServerInfo {
                version: "0.1.0".into(),
                name: "vps".into(),
            },
            client_id: ClientId("c-1".into()),
            scope: Scope::Read,
        });
        rt(ServerMsg::State {
            rev: 4,
            snapshot: StateView {
                rev: 4,
                ..Default::default()
            },
        });
        rt(ServerMsg::Deltas {
            rev: 5,
            items: vec![],
        });
        let chat = rt(ServerMsg::Chat {
            agent: "P-20261018-demo/executor".parse().unwrap(),
            entries: vec![crate::view::ChatEntry {
                uuid: "u-1".into(),
                ts: 1,
                role: crate::view::ChatRole::Assistant,
                kind: crate::view::ChatKind::ToolCall,
                text: "cargo test".into(),
                tool: Some(crate::view::ChatTool {
                    name: "Bash".into(),
                    summary: "cargo test".into(),
                    input: Some(serde_json::json!({"command": "cargo test"})),
                }),
                call_id: Some("toolu_1".into()),
                is_error: false,
                full_len: None,
                model: None,
            }],
        });
        assert!(
            chat.contains(r#""type":"chat""#) && chat.contains(r#""kind":"tool_call""#),
            "{chat}"
        );
        rt(ServerMsg::Exit {
            term: TermId(1),
            code: 0,
            signal: Some("SIGTERM".into()),
        });
        rt(ServerMsg::Size {
            term: TermId(1),
            rows: 30,
            cols: 100,
            owner: Some(ClientId("c-1".into())),
        });
        rt(ServerMsg::Size {
            term: TermId(1),
            rows: 30,
            cols: 100,
            owner: None,
        });
        rt(ServerMsg::Presence {
            clients: vec![PresenceEntry {
                id: ClientId("c-1".into()),
                kind: ClientKind::Cli,
                device: Some("phone".into()),
            }],
        });
        let e = rt(ServerMsg::Error {
            code: ErrorCode::Forbidden,
            message: "read-only".into(),
        });
        assert!(e.contains(r#""type":"error""#) && e.contains("forbidden"));
        rt(ServerMsg::Pong { nonce: 9 });
    }

    #[test]
    fn frames_round_trip() {
        let data = vec![0u8, 1, 0x1b, 0xff, b'a'];
        for f in [
            Frame::Input {
                term: TermId(7),
                data: data.clone(),
            },
            Frame::Output {
                term: TermId(u32::MAX),
                seq: u64::MAX,
                data: data.clone(),
            },
            Frame::Snapshot {
                term: TermId(0),
                seq: 1 << 40,
                data: data.clone(),
            },
            Frame::Input {
                term: TermId(1),
                data: vec![],
            },
            Frame::Output {
                term: TermId(1),
                seq: 0,
                data: vec![],
            },
        ] {
            assert_eq!(Frame::decode(&f.encode()).unwrap(), f);
        }
    }

    #[test]
    fn frame_layout_is_fixed() {
        let out = Frame::Output {
            term: TermId(0x0102_0304),
            seq: 0x0a0b_0c0d_0e0f_1011,
            data: b"hi".to_vec(),
        };
        assert_eq!(
            out.encode(),
            [2, 1, 2, 3, 4, 10, 11, 12, 13, 14, 15, 16, 17, b'h', b'i']
        );
        let snap = Frame::Snapshot {
            term: TermId(1),
            seq: 2,
            data: vec![],
        };
        assert_eq!(snap.encode()[0], KIND_SNAPSHOT);
        assert_ne!(snap.encode()[0], out.encode()[0]);
        let inp = Frame::Input {
            term: TermId(1),
            data: b"x".to_vec(),
        };
        assert_eq!(inp.encode(), [1, 0, 0, 0, 1, b'x']);
        assert_eq!(inp.term(), TermId(1));
    }

    #[test]
    fn malformed_frames_are_errors() {
        assert_eq!(Frame::decode(&[]), Err(FrameError::Empty));
        assert_eq!(Frame::decode(&[0x00]), Err(FrameError::UnknownKind(0)));
        assert_eq!(
            Frame::decode(&[0x7f, 0, 0, 0, 0]),
            Err(FrameError::UnknownKind(0x7f))
        );
        assert_eq!(
            Frame::decode(&[KIND_INPUT, 0, 0]),
            Err(FrameError::Truncated { need: 5, got: 3 })
        );
        // Output with a term but a cut seq.
        assert_eq!(
            Frame::decode(&[KIND_OUTPUT, 0, 0, 0, 1, 0, 0, 0]),
            Err(FrameError::Truncated { need: 13, got: 8 })
        );
        assert_eq!(
            Frame::decode(&[KIND_SNAPSHOT]),
            Err(FrameError::Truncated { need: 13, got: 1 })
        );
        assert!(FrameError::UnknownKind(9).to_string().contains("0x09"));
        // Every prefix of a valid frame shorter than its header fails cleanly.
        let f = Frame::Output {
            term: TermId(1),
            seq: 2,
            data: vec![],
        }
        .encode();
        for n in 0..f.len() {
            assert!(Frame::decode(&f[..n]).is_err(), "prefix {n}");
        }
    }
}
