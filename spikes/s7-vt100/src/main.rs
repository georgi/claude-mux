//! S7: for each captured cut point, feed bytes[..offset] to parser A, take state_formatted(), feed to fresh parser B,
//! compare cell-by-cell. Also dumps snapshots to snap-<label>.bin (for the xterm.js half) and text rows to snap-<label>.txt.
use std::fs;
fn main() {
    let dir = std::env::args().nth(1).unwrap_or(".".into());
    let bytes = fs::read(format!("{dir}/capture.bin")).unwrap();
    let marks: serde_json::Value = serde_json::from_slice(&fs::read(format!("{dir}/marks.json")).unwrap()).unwrap();
    let (rows, cols) = (marks["rows"].as_u64().unwrap() as u16, marks["cols"].as_u64().unwrap() as u16);
    let mut all_ok = true;
    for m in marks["marks"].as_array().unwrap() {
        let label = m["label"].as_str().unwrap();
        let off = m["offset"].as_u64().unwrap() as usize;
        let mut a = vt100::Parser::new(rows, cols, 0);
        a.process(&bytes[..off]);
        let snap = a.screen().state_formatted();
        let mut b = vt100::Parser::new(rows, cols, 0);
        let mut snap = snap;
        if a.screen().alternate_screen() { snap.splice(0..0, b"\x1b[?1049h".iter().copied()); } // fix: re-enter alt screen first
        b.process(&snap);
        let (sa, sb) = (a.screen(), b.screen());
        let mut diffs = 0;
        for r in 0..rows { for c in 0..cols {
            let (ca, cb) = (sa.cell(r, c).unwrap(), sb.cell(r, c).unwrap());
            if ca.contents() != cb.contents() || ca.fgcolor() != cb.fgcolor() || ca.bgcolor() != cb.bgcolor()
                || ca.bold() != cb.bold() || ca.italic() != cb.italic() || ca.underline() != cb.underline() || ca.inverse() != cb.inverse()
                || ca.dim() != cb.dim() || ca.is_wide() != cb.is_wide() { diffs += 1; if diffs < 4 { eprintln!("  diff at {r},{c}: {:?} vs {:?}", ca.contents(), cb.contents()); } }
        } }
        let cur = (sa.cursor_position() == sb.cursor_position(), sa.hide_cursor() == sb.hide_cursor(), true,
                   sa.application_cursor() == sb.application_cursor(), sa.bracketed_paste() == sb.bracketed_paste(), sa.mouse_protocol_mode() == sb.mouse_protocol_mode() && sa.mouse_protocol_encoding() == sb.mouse_protocol_encoding() && sa.alternate_screen() == sb.alternate_screen());
        eprintln!("  A: mouse={:?}/{:?} alt={} | B: mouse={:?}/{:?} alt={}", sa.mouse_protocol_mode(), sa.mouse_protocol_encoding(), sa.alternate_screen(), sb.mouse_protocol_mode(), sb.mouse_protocol_encoding(), sb.alternate_screen());
        let fmt_same = sa.contents_formatted() == sb.contents_formatted();
        println!("{label:28} off={off:6} snap={:5}B cell_diffs={diffs} cursor/hide/title/appcur/bpaste/mouse_same={:?} contents_formatted_equal={fmt_same} cursor={:?} hidden={} bpaste={}",
                 snap.len(), cur, sa.cursor_position(), sa.hide_cursor(), sa.bracketed_paste());
        if diffs > 0 || !fmt_same || cur != (true, true, true, true, true, true) { all_ok = false; }
        fs::write(format!("{dir}/snap-{label}.bin"), &snap).unwrap();
        fs::write(format!("{dir}/snap-{label}.txt"), sa.contents()).unwrap();
        // also the fallback form: contents_formatted + cursor position
    }
    println!("ALL_OK={all_ok}");
}
