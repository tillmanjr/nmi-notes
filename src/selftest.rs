//! `nmi-notes --selftest`: exercises the real RichEdit control against the
//! store to verify offsets, line endings, and caret preservation. Runs
//! headless (windows are never shown). Prints results; exit code 0 = pass.

use nmi_notes::model::diff::diff_utf16;
use nmi_notes::model::placement::Rect;
use nmi_notes::model::store::{Change, Op, Store};

use crate::win::{clipboard, editor};

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

pub fn run() -> i32 {
    let mut failures = 0;
    let mut check = |name: &str, ok: bool, detail: String| {
        if ok {
            println!("PASS {name}");
        } else {
            println!("FAIL {name}: {detail}");
            failures += 1;
        }
    };

    editor::register_class();
    let Some(h) = editor::create() else {
        println!("FAIL could not create editor");
        return 1;
    };

    // 1. Round-trip: \n in, \n out, surrogates intact, offsets 1:1.
    let initial = "alpha\nbeta\n😀 gamma\n";
    editor::set_text(h, initial);
    let got = editor::get_text16(h);
    check("roundtrip", got == u16s(initial), format!("{:?}", String::from_utf16_lossy(&got)));
    check("caret at end after set_text", editor::selection(h) == (got.len(), got.len()), format!("{:?}", editor::selection(h)));

    // 2. Local edit -> diff -> store edit stays identical to editor text.
    let mut store = Store::new(vec![], None);
    let ch = store.dispatch(Op::Create { text: initial.into() }, 1).unwrap();
    let Change::Created { id } = ch[0] else { unreachable!() };
    let mut synced = u16s(initial);
    let mut rev = 1;

    editor::type_text(h, 6, 10, "BETA\nextra line");
    let now = editor::get_text16(h);
    let sp = diff_utf16(&synced, &now).expect("a change");
    let text = String::from_utf16_lossy(&now[sp.start..sp.start + sp.new_len]);
    let res = store.dispatch(Op::Edit { id, base_rev: rev, start: sp.start, old_len: sp.old_len, text }, 2);
    check("store accepts editor diff", res.is_ok(), format!("{res:?}"));
    rev += 1;
    synced = now.clone();
    let store_text = store.note(id).unwrap().text.clone();
    check(
        "store == editor after local edit",
        u16s(&store_text) == now,
        format!("store={store_text:?} editor={:?}", String::from_utf16_lossy(&now)),
    );

    // 3. Remote append with caret mid-text: caret stays, text matches store.
    editor::set_selection(h, 3, 3);
    let ch = store.dispatch(Op::Append { id, text: "appended\nlines".into() }, 3).unwrap();
    if let Change::Text { splice, rev: r, .. } = &ch[0] {
        editor::apply_splice(h, splice.start, splice.old_len, &splice.text);
        rev = *r;
    }
    let now = editor::get_text16(h);
    let store_text = store.note(id).unwrap().text.clone();
    check("store == editor after remote append", u16s(&store_text) == now, format!("{:?}", String::from_utf16_lossy(&now)));
    check("caret preserved mid-text", editor::selection(h) == (3, 3), format!("{:?}", editor::selection(h)));

    // 4. Remote insert before a selection shifts it.
    editor::set_selection(h, 10, 14);
    let ch = store.dispatch(Op::Edit { id, base_rev: rev, start: 0, old_len: 0, text: "XY\n".into() }, 4).unwrap();
    if let Change::Text { splice, rev: r, .. } = &ch[0] {
        editor::apply_splice(h, splice.start, splice.old_len, &splice.text);
        rev = *r;
    }
    check("selection shifted by remote insert", editor::selection(h) == (13, 17), format!("{:?}", editor::selection(h)));

    // 5. Caret at end follows appended text (tail mode).
    let len = editor::get_text16(h).len();
    editor::set_selection(h, len, len);
    let ch = store.dispatch(Op::Append { id, text: "tail".into() }, 5).unwrap();
    if let Change::Text { splice, .. } = &ch[0] {
        editor::apply_splice(h, splice.start, splice.old_len, &splice.text);
    }
    let now = editor::get_text16(h);
    check("tail follows", editor::selection(h) == (now.len(), now.len()), format!("{:?} len {}", editor::selection(h), now.len()));
    let store_text = store.note(id).unwrap().text.clone();
    check("store == editor at end", u16s(&store_text) == now, format!("{:?}", String::from_utf16_lossy(&now)));
    let _ = (rev, synced);

    // 6. CRLF pasted by the user is normalized to single \n units.
    editor::set_text(h, "a");
    editor::type_text(h, 1, 1, "\r\nb");
    let now = editor::get_text16(h);
    check("pasted CRLF becomes one \\n", now == u16s("a\nb"), format!("{:?}", String::from_utf16_lossy(&now)));

    // 7. Clipboard round-trip (opt-in: `--clipboard`; restores prior text).
    if std::env::args().any(|a| a == "--clipboard") {
    let before = clipboard::read(h);
    let ok = clipboard::write_text(h, "clip\nline two");
    let after = clipboard::read(h);
    let roundtrip = matches!(&after, clipboard::ClipRead::Text(t) if t == "clip\nline two");
    check("clipboard round-trip", ok && roundtrip, "mismatch".into());
    if let clipboard::ClipRead::Text(t) = before {
        let _ = clipboard::write_text(h, &t);
    }
    }

    // 8. Placement helper with a sample work area.
    let r = nmi_notes::model::placement::place_window(
        nmi_notes::model::placement::Point { x: 100, y: 100 },
        Rect { left: 0, top: 0, right: 1920, bottom: 1040 },
        700,
        500,
        200,
    );
    check("placement fallback right", r.left == 300, format!("{r:?}"));

    println!("{} failure(s)", failures);
    i32::from(failures > 0)
}
