//! Smoke tests: run `cargo test --test smoke` after the crate builds.
use sparrow_engine::{create_engine, safety};

const PAGE: &str = "<html><head><title>T</title><style>p{color:red}</style></head>\
<body><h1>Hello</h1><p>World <a href=\"/x\">link</a></p><script>var a=1+1;</script></body></html>";

#[test] fn loads_and_renders_pixels() {
    let mut e = create_engine();
    e.set_viewport(320, 200);
    e.load_html(PAGE, "https://example.com/").unwrap();
    assert_eq!(e.title(), "T");
    let px = e.render().unwrap();
    assert_eq!(px.len(), 320 * 200 * 4);
    assert!(px.iter().any(|&b| b != px[0]), "frame is one flat colour");
}
#[test] fn js_eval_works() {
    let mut e = create_engine();
    e.load_html(PAGE, "https://example.com/").unwrap();
    assert_eq!(e.eval_js("1+2").unwrap(), "3");
}
#[test] fn scroll_and_hit_test_never_panic() {
    let mut e = create_engine();
    e.load_html(PAGE, "https://example.com/").unwrap();
    let _ = e.scroll_by(f32::NAN);
    let _ = e.scroll_by(1e9);
    let _ = e.hit_test(-5.0, f32::INFINITY);
    assert!(!e.is_crashed());
}
#[test] fn garbage_html_survives() {
    let mut e = create_engine();
    for h in ["", "<", "<<<>>>", "<div><div><div>", "\u{0}\u{1}<p style=\"width:99999999px\">x"] {
        let _ = e.load_html(h, "https://example.com/");
        let _ = e.render();
    }
    assert!(!e.is_crashed());
}
#[test] fn deep_nesting_does_not_kill_process() {
    let html = "<div>".repeat(3000) + "x";
    let r = safety::run_big_stack(256, move || {
        let mut e = create_engine();
        let _ = e.load_html(&html, "https://example.com/");
        e.render().map(|p| p.len())
    });
    assert!(r.is_ok());
}
#[test] fn recover_after_crash_flag() {
    let mut e = create_engine();
    e.recover();
    assert!(!e.is_crashed());
}
#[test] fn history_is_ram_only() {
    let mut h = sparrow_engine::browser::history::History::open("must_not_exist.db").unwrap();
    h.add("https://a.com", "A").unwrap();
    assert!(!std::path::Path::new("must_not_exist.db").exists());
}
