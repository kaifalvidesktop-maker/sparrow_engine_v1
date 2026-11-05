use sparrow_engine::sparrow_engine::SparrowEngine;
fn px(buf:&[u8],w:u32,x:u32,y:u32)->[u8;3]{let i=((y*w+x)*4) as usize;[buf[i],buf[i+1],buf[i+2]]}
#[test]
fn all() {
    let mut e = SparrowEngine::new();
    e.set_viewport(400, 300);
    // blocks must stack vertically; rgba(0,0,0,0) must be transparent; @media must not leak
    let html = r#"<style>.a{background:rgb(255, 0, 0);height:50px} .b{background:#00f;height:50px} @media print{.x{color:red} .b{background:#000}} .c.d{background:#0f0} .e{background:rgba(0,0,0,0);height:20px}</style>
    <div class=a>A</div><div class=b>B</div><div class=c>only c</div><div class='c d'>cd</div><div class=e>E</div>
    "#.to_string() + &"<p>long</p>".repeat(100) + r#"<h1 id=sec>Sec</h1><a href='#sec'>go</a>"#;
    e.load_html(&html, "https://x.test/p").unwrap();
    let buf = e.render().unwrap();
    println!("a={:?} b={:?}", px(&buf,400,300,10), px(&buf,400,300,40));
    assert_eq!(px(&buf,400,300,10),[255,0,0], "block A red at top");
    assert_eq!(px(&buf,400,300,40),[0,0,255], "block B below A, @media not leaked");
    // fragment: no refetch, scroll moves
    e.load_url("https://x.test/p#sec").unwrap();
    let (u, _) = (e.url().to_string(), 0);
    println!("url={u}");
    assert!(u.ends_with("#sec"));
    let s = e.scroll_by(f32::NAN).unwrap();
    println!("scroll after jump={s}");
    assert!(s > 100.0 && s.is_finite());
    e.set_viewport(400, 5000);
    e.render().unwrap();
    assert_eq!(e.scroll_by(0.0).unwrap(), 0.0, "clamped after resize");
    // crash recovery / js loop
    assert!(e.eval_js("while(true){}").is_err());
    assert!(e.load_url("about:blank ").is_ok());
    // deep nesting must not abort
    let deep = "<div>".repeat(20000) + "x";
    std::thread::Builder::new().stack_size(2*1024*1024).spawn(move||{
        let mut e = SparrowEngine::new(); e.load_html(&deep,"https://x.test/").unwrap(); let _=e.render();
    }).unwrap().join().unwrap();
}
