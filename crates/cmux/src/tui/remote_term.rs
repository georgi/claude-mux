//! A terminal of a cmuxd server, as a `Pane` sees it.
//!
//! It stands in for the local PTY: `parser` is a `vt100::Parser` that the
//! connection thread feeds with `Output` and `Snapshot` frames (the parser is
//! rebuilt on a snapshot), `dirty` is set when it changed, `write` sends
//! keyboard bytes as `Input` frames and `resize` sends a `Resize` message.
//! Drawing reads `parser` exactly as it does for a local pane.
//!
//! Notes for users of this type:
//! - Resizing only asks the server. The PTY follows the size owner (the client
//!   that focused the terminal last); the server's `Size` message resizes the
//!   parser of every client, and wins over the local guess.
//! - Terminal queries (DSR, DA) are not answered here: with several clients
//!   attached, each would reply and the program would see duplicates.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use cmux_proto::view::TermId;

use crate::conn::stream::{ConnHandle, TermShared};
use crate::pane::Callbacks;

pub struct RemoteTerm {
    pub term: TermId,
    /// The screen. Locked briefly by the connection thread on every frame.
    pub parser: Arc<Mutex<vt100::Parser<Callbacks>>>,
    /// Set by the connection thread when `parser` changed; the UI clears it
    /// before it draws (the same protocol as a local pane).
    pub dirty: Arc<AtomicBool>,
    handle: ConnHandle,
    size: (u16, u16),
    exited: bool,
    attached: bool,
}

impl RemoteTerm {
    /// Subscribes to `term` on the connection. Until the server's snapshot
    /// arrives the screen is blank at `rows` x `cols`.
    pub fn new(handle: ConnHandle, term: TermId, rows: u16, cols: u16) -> RemoteTerm {
        let TermShared { parser, dirty } = handle.open_term(term, rows, cols);
        RemoteTerm {
            term,
            parser,
            dirty,
            handle,
            size: (rows, cols),
            exited: false,
            attached: true,
        }
    }

    /// Keyboard, paste or mouse bytes for the program. Dropped while the
    /// connection is down.
    pub fn write(&self, bytes: &[u8]) {
        self.handle.input(self.term, bytes);
    }

    /// Asks for a new size and resizes the local screen to match at once.
    pub fn resize(&mut self, rows: u16, cols: u16) {
        if self.size == (rows, cols) {
            return;
        }
        self.size = (rows, cols);
        self.handle.resize(self.term, rows, cols);
        self.parser
            .lock()
            .unwrap()
            .screen_mut()
            .set_size(rows.max(1), cols.max(1));
    }

    /// Makes this client the size owner of the terminal (call when the pane
    /// gets focus).
    pub fn focus(&self) {
        self.handle.focus(self.term);
    }

    /// (rows, cols) as last requested by this client.
    pub fn size(&self) -> (u16, u16) {
        self.size
    }

    /// Whether the program has exited; the integrator sets it from the state
    /// (`AgentView::life`, or the terminal disappearing).
    pub fn exited(&self) -> bool {
        self.exited
    }

    pub fn set_exited(&mut self, exited: bool) {
        self.exited = exited;
    }

    /// Stops streaming this terminal. The program keeps running.
    pub fn detach(&mut self) {
        if self.attached {
            self.attached = false;
            self.handle.close_term(self.term);
        }
    }
}

impl Drop for RemoteTerm {
    fn drop(&mut self) {
        self.detach();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use cmux_proto::stream::{ClientMsg, Frame};

    use super::*;
    use crate::conn::stream::{Cmd, ConnHandle, Session, FrameAction, feed};

    /// Bytes a Claude-like program might send: colors, cursor moves, a clear,
    /// wide chars, an alt-screen switch and enough lines to scroll.
    fn recorded() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"\x1b[2J\x1b[H\x1b[1;32mcmux\x1b[0m ready\r\n");
        for i in 0..40 {
            v.extend_from_slice(format!("line {i} \x1b[38;5;{}mcolor\x1b[0m\r\n", i + 20).as_bytes());
        }
        v.extend_from_slice("日本語 text \x1b[5;10Hat 5,10\x1b[K".as_bytes());
        v.extend_from_slice(b"\x1b[?1049h\x1b[2J\x1b[Halt screen\x1b[2;3Hxy");
        v.extend_from_slice(b"\x1b[?1049l\x1b[?25l back");
        v
    }

    const ROWS: u16 = 12;
    const COLS: u16 = 50;

    fn local(bytes: &[u8]) -> vt100::Parser {
        let mut p = vt100::Parser::new(ROWS, COLS, 10_000);
        p.process(bytes);
        p
    }

    /// Plays frames into a RemoteTerm through the same functions the
    /// connection thread uses.
    fn play(rt: &RemoteTerm, handle: &ConnHandle, session: &mut Session, frames: Vec<Frame>) {
        for f in frames {
            if let FrameAction::Feed { term, reset, data } = session.on_frame(f) {
                feed(handle.terms(), term, reset, &data, &|_| {});
            }
        }
        assert!(rt.dirty.load(Ordering::Acquire));
    }

    fn screens_equal(rt: &RemoteTerm, want: &vt100::Parser) {
        let got = rt.parser.lock().unwrap();
        assert_eq!(got.screen().contents_formatted(), want.screen().contents_formatted());
        assert_eq!(got.screen().cursor_position(), want.screen().cursor_position());
        assert_eq!(got.screen().alternate_screen(), want.screen().alternate_screen());
    }

    fn setup() -> (RemoteTerm, ConnHandle, std::sync::mpsc::Receiver<Cmd>, Session) {
        let (handle, rx) = ConnHandle::detached();
        let rt = RemoteTerm::new(handle.clone(), TermId(7), ROWS, COLS);
        let mut session = Session::new();
        // The connection thread applies the Want the constructor queued.
        session.on_cmd(&rx.try_recv().unwrap());
        (rt, handle, rx, session)
    }

    fn chunks(bytes: &[u8], term: TermId, mut seq: u64, size: usize) -> Vec<Frame> {
        bytes
            .chunks(size)
            .map(|c| {
                let f = Frame::Output {
                    term,
                    seq,
                    data: c.to_vec(),
                };
                seq += c.len() as u64;
                f
            })
            .collect()
    }

    #[test]
    fn output_frames_match_a_local_parser() {
        let (rt, handle, _rx, mut session) = setup();
        let bytes = recorded();
        // Odd chunk sizes split escape sequences and UTF-8 across frames.
        play(&rt, &handle, &mut session, chunks(&bytes, TermId(7), 0, 13));
        screens_equal(&rt, &local(&bytes));
        assert_eq!(session.next_seq(TermId(7)), Some(bytes.len() as u64));
    }

    #[test]
    fn snapshot_mid_stream_resets_and_matches() {
        let (rt, handle, _rx, mut session) = setup();
        let bytes = recorded();
        let (head, tail) = bytes.split_at(bytes.len() / 2);
        play(&rt, &handle, &mut session, chunks(head, TermId(7), 0, 29));
        // The server (or a reconnect) sends a redraw of the state at the split.
        // Garbage first, to prove the parser is reset rather than patched.
        play(
            &rt,
            &handle,
            &mut session,
            chunks(b"\x1b[31mJUNK\x1b[?1049h", TermId(7), head.len() as u64, 64),
        );
        let at = head.len() as u64;
        let snap = {
            let mut p = local(head);
            let mut out = p.screen().state_formatted();
            out.extend_from_slice(&p.screen().attributes_formatted());
            let _ = &mut p;
            out
        };
        // The snapshot is positioned at the split, so live output continues there.
        play(
            &rt,
            &handle,
            &mut session,
            std::iter::once(Frame::Snapshot {
                term: TermId(7),
                seq: at,
                data: snap,
            })
            .chain(chunks(tail, TermId(7), at, 31))
            .collect(),
        );
        screens_equal(&rt, &local(&bytes));
        assert_eq!(session.next_seq(TermId(7)), Some(bytes.len() as u64));
    }

    #[test]
    fn write_resize_and_detach_become_commands() {
        let (mut rt, _handle, rx, _session) = setup();
        rt.write(b"hi\r");
        rt.focus();
        rt.resize(30, 100);
        rt.resize(30, 100); // no change: nothing sent
        assert_eq!(rt.parser.lock().unwrap().screen().size(), (30, 100));
        rt.detach();
        let cmds: Vec<Cmd> = rx.try_iter().collect();
        assert_eq!(
            cmds,
            vec![
                Cmd::Input {
                    term: TermId(7),
                    data: b"hi\r".to_vec()
                },
                Cmd::Focus(TermId(7)),
                Cmd::Resize {
                    term: TermId(7),
                    rows: 30,
                    cols: 100
                },
                Cmd::Release(TermId(7)),
            ]
        );
        // Dropping after detach does not release twice.
        drop(rt);
        assert!(rx.try_recv().is_err());
        // The messages the thread would put on the wire.
        let mut s = Session::new();
        s.on_cmd(&Cmd::Want(TermId(7)));
        assert!(s.on_cmd(&cmds[2]).is_some());
        let _ = ClientMsg::Ping { nonce: 0 };
    }
}
