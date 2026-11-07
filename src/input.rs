//! Text editing model for <input>/<textarea>: caret, selection, typing, delete, word/line movement,
//! clipboard, undo/redo, mouse mapping. DOM-independent: the engine owns one `TextField` per focused control.
//! Caret moves by grapheme-ish clusters (Bengali/Devanagari vowel signs + virama conjuncts stay together);
//! Backspace/Delete remove one code point (like most editors).

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Key { Char(char), Backspace, Delete, Left, Right, Up, Down, Home, End, Enter, Tab, Escape }

#[derive(Clone, Copy, Debug, Default)]
pub struct Mods { pub shift: bool, pub ctrl: bool, pub alt: bool }

pub trait Clipboard { fn get(&mut self) -> String; fn set(&mut self, text: &str); }

#[derive(Default)]
pub struct MemoryClipboard(pub String);
impl Clipboard for MemoryClipboard {
    fn get(&mut self) -> String { self.0.clone() }
    fn set(&mut self, t: &str) { self.0 = t.to_string(); }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Outcome { None, CaretMoved, Changed, Submit, Tab, Escape }

#[derive(Clone)]
struct Snap { chars: Vec<char>, caret: usize, anchor: Option<usize> }

#[derive(Clone, Copy, PartialEq)]
enum Edit { None, Typing, Deleting, Other }

pub struct TextField {
    chars: Vec<char>,
    caret: usize,
    anchor: Option<usize>,
    pub multiline: bool,
    pub password: bool,
    pub max_len: usize,
    undo: Vec<Snap>,
    redo: Vec<Snap>,
    last: Edit,
    want_col: Option<usize>,
}

const VIRAMA: [char; 9] = ['\u{094D}', '\u{09CD}', '\u{0A4D}', '\u{0ACD}', '\u{0B4D}', '\u{0BCD}', '\u{0C4D}', '\u{0CCD}', '\u{0D4D}'];

/// Combining marks that must stay attached to the previous character.
fn extends(c: char) -> bool {
    let u = c as u32;
    matches!(u, 0x0300..=0x036F | 0x200C | 0x200D | 0xFE00..=0xFE0F
        | 0x0900..=0x0903 | 0x093A..=0x094F | 0x0951..=0x0957 | 0x0962..=0x0963
        | 0x0981..=0x0983 | 0x09BC | 0x09BE..=0x09CD | 0x09D7 | 0x09E2..=0x09E3)
        && !matches!(u, 0x093D | 0x09BD) // avagraha is a letter
}

fn class(c: char) -> u8 {
    if c.is_whitespace() { 0 } else if c.is_ascii_punctuation() && c != '_' { 2 } else { 1 }
}

impl TextField {
    pub fn new(multiline: bool) -> Self {
        TextField { chars: vec![], caret: 0, anchor: None, multiline, password: false, max_len: 1 << 20, undo: vec![], redo: vec![], last: Edit::None, want_col: None }
    }
    pub fn with_text(multiline: bool, t: &str) -> Self { let mut f = Self::new(multiline); f.set_text(t); f }

    /// Replace the whole value (programmatic; clears undo history). Caret goes to the end.
    pub fn set_text(&mut self, t: &str) {
        self.chars = t.chars().filter(|c| self.multiline || (*c != '\n' && *c != '\r')).take(self.max_len).collect();
        self.caret = self.chars.len();
        self.anchor = None; self.undo.clear(); self.redo.clear(); self.last = Edit::None; self.want_col = None;
    }
    pub fn text(&self) -> String { self.chars.iter().collect() }
    /// What to draw (password fields show '*').
    pub fn display_text(&self) -> String { if self.password { "*".repeat(self.chars.len()) } else { self.text() } }
    pub fn len(&self) -> usize { self.chars.len() }
    pub fn is_empty(&self) -> bool { self.chars.is_empty() }
    pub fn caret(&self) -> usize { self.caret }
    pub fn selection(&self) -> Option<(usize, usize)> {
        self.anchor.filter(|a| *a != self.caret).map(|a| (a.min(self.caret), a.max(self.caret)))
    }
    pub fn selected_text(&self) -> String { self.selection().map(|(a, b)| self.chars[a..b].iter().collect()).unwrap_or_default() }

    // ---------------------------------------------------------------- boundaries
    /// Start index of every cluster, plus `len` at the end.
    fn bounds(&self) -> Vec<usize> {
        let mut b = vec![];
        let mut i = 0;
        while i < self.chars.len() {
            b.push(i);
            i += 1;
            loop {
                while i < self.chars.len() && extends(self.chars[i]) { i += 1; }
                // virama / ZWJ glues the following base character (conjunct)
                if i < self.chars.len() && self.chars[i] != '\n' && (VIRAMA.contains(&self.chars[i - 1]) || self.chars[i - 1] == '\u{200D}') { i += 1; } else { break; }
            }
        }
        b.push(self.chars.len());
        b
    }
    fn prev_cluster(&self, from: usize) -> usize { self.bounds().into_iter().rev().find(|&b| b < from).unwrap_or(0) }
    fn next_cluster(&self, from: usize) -> usize { self.bounds().into_iter().find(|&b| b > from).unwrap_or(self.chars.len()) }
    fn word_left(&self, mut i: usize) -> usize {
        while i > 0 && class(self.chars[i - 1]) == 0 { i -= 1; }
        if i > 0 { let c = class(self.chars[i - 1]); while i > 0 && class(self.chars[i - 1]) == c { i -= 1; } }
        i
    }
    fn word_right(&self, mut i: usize) -> usize {
        let n = self.chars.len();
        while i < n && class(self.chars[i]) == 0 { i += 1; }
        if i < n { let c = class(self.chars[i]); while i < n && class(self.chars[i]) == c { i += 1; } }
        i
    }
    /// (start, end) of line containing `i` (end excludes '\n').
    fn line_range(&self, i: usize) -> (usize, usize) {
        let s = self.chars[..i].iter().rposition(|&c| c == '\n').map(|p| p + 1).unwrap_or(0);
        let e = self.chars[i..].iter().position(|&c| c == '\n').map(|p| i + p).unwrap_or(self.chars.len());
        (s, e)
    }
    pub fn lines(&self) -> Vec<(usize, usize)> {
        let mut v = vec![];
        let mut s = 0;
        for (i, &c) in self.chars.iter().enumerate() { if c == '\n' { v.push((s, i)); s = i + 1; } }
        v.push((s, self.chars.len()));
        v
    }

    // ---------------------------------------------------------------- history
    fn snap(&self) -> Snap { Snap { chars: self.chars.clone(), caret: self.caret, anchor: self.anchor } }
    fn begin_edit(&mut self, kind: Edit) {
        if self.last != kind || kind == Edit::Other {
            self.undo.push(self.snap());
            if self.undo.len() > 100 { self.undo.remove(0); }
        }
        self.last = kind;
        self.redo.clear();
        self.want_col = None;
    }
    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.redo.push(self.snap());
        self.chars = s.chars; self.caret = s.caret; self.anchor = s.anchor; self.last = Edit::None;
        true
    }
    pub fn redo(&mut self) -> bool {
        let Some(s) = self.redo.pop() else { return false };
        self.undo.push(self.snap());
        self.chars = s.chars; self.caret = s.caret; self.anchor = s.anchor; self.last = Edit::None;
        true
    }

    // ---------------------------------------------------------------- editing
    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else { self.anchor = None; return false };
        self.chars.drain(a..b);
        self.caret = a; self.anchor = None;
        true
    }

    /// Insert text at the caret (typing, paste, IME commit). Respects `max_len` and single-line rules.
    pub fn insert_str(&mut self, s: &str) -> bool {
        let ins: Vec<char> = s.chars().filter(|&c| if c == '\n' { self.multiline } else if c == '\r' { false } else { !c.is_control() || c == '\t' }).collect();
        let sel_len = self.selection().map(|(a, b)| b - a).unwrap_or(0);
        let room = self.max_len.saturating_sub(self.chars.len() - sel_len);
        let ins: Vec<char> = ins.into_iter().take(room).collect();
        if ins.is_empty() && sel_len == 0 { return false; }
        self.begin_edit(if ins.len() == 1 && ins[0] != '\n' { Edit::Typing } else { Edit::Other });
        self.delete_selection();
        let n = ins.len();
        self.chars.splice(self.caret..self.caret, ins);
        self.caret += n;
        true
    }

    fn move_to(&mut self, to: usize, extend: bool) -> Outcome {
        let to = to.min(self.chars.len());
        if extend { if self.anchor.is_none() { self.anchor = Some(self.caret); } } else { self.anchor = None; }
        let changed = to != self.caret;
        self.caret = to;
        self.last = Edit::None;
        if changed { Outcome::CaretMoved } else { Outcome::None }
    }

    pub fn select_all(&mut self) { self.anchor = Some(0); self.caret = self.chars.len(); self.last = Edit::None; }
    /// Set caret from the mouse (`extend` = shift/drag).
    pub fn set_caret(&mut self, idx: usize, extend: bool) { self.want_col = None; self.move_to(idx, extend); }
    /// Double-click: select the word under `idx`.
    pub fn select_word(&mut self, idx: usize) {
        let i = idx.min(self.chars.len());
        let (mut a, mut b) = (i, i);
        if i < self.chars.len() { let c = class(self.chars[i]); while a > 0 && class(self.chars[a - 1]) == c { a -= 1; } while b < self.chars.len() && class(self.chars[b]) == c { b += 1; } }
        self.anchor = Some(a); self.caret = b; self.last = Edit::None;
    }

    pub fn handle_key(&mut self, key: Key, m: Mods, clip: &mut dyn Clipboard) -> Outcome {
        let sel = m.shift;
        match key {
            Key::Char(c) if m.ctrl && !m.alt => match c.to_ascii_lowercase() {
                'a' => { self.select_all(); Outcome::CaretMoved }
                'c' => { if self.selection().is_some() { clip.set(&self.selected_text()); } Outcome::None }
                'x' => { if self.selection().is_some() { clip.set(&self.selected_text()); self.begin_edit(Edit::Other); self.delete_selection(); Outcome::Changed } else { Outcome::None } }
                'v' => { let t = clip.get(); if self.insert_str(&t) { Outcome::Changed } else { Outcome::None } }
                'z' => { let ok = if m.shift { self.redo() } else { self.undo() }; if ok { Outcome::Changed } else { Outcome::None } }
                'y' => { if self.redo() { Outcome::Changed } else { Outcome::None } }
                _ => Outcome::None,
            },
            Key::Char(c) => { if self.insert_str(&c.to_string()) { Outcome::Changed } else { Outcome::None } }
            Key::Backspace => {
                if self.selection().is_some() { self.begin_edit(Edit::Other); self.delete_selection(); return Outcome::Changed; }
                if self.caret == 0 { return Outcome::None; }
                let to = if m.ctrl { self.word_left(self.caret) } else { self.caret - 1 };
                self.begin_edit(if m.ctrl { Edit::Other } else { Edit::Deleting });
                self.chars.drain(to..self.caret);
                self.caret = to;
                Outcome::Changed
            }
            Key::Delete => {
                if self.selection().is_some() { self.begin_edit(Edit::Other); self.delete_selection(); return Outcome::Changed; }
                if self.caret >= self.chars.len() { return Outcome::None; }
                let to = if m.ctrl { self.word_right(self.caret) } else { self.caret + 1 };
                self.begin_edit(if m.ctrl { Edit::Other } else { Edit::Deleting });
                self.chars.drain(self.caret..to);
                Outcome::Changed
            }
            Key::Left => {
                if let (Some((a, _)), false) = (self.selection(), sel) { self.anchor = None; self.caret = a; return Outcome::CaretMoved; }
                let to = if m.ctrl { self.word_left(self.caret) } else { self.prev_cluster(self.caret) };
                self.want_col = None; self.move_to(to, sel)
            }
            Key::Right => {
                if let (Some((_, b)), false) = (self.selection(), sel) { self.anchor = None; self.caret = b; return Outcome::CaretMoved; }
                let to = if m.ctrl { self.word_right(self.caret) } else { self.next_cluster(self.caret) };
                self.want_col = None; self.move_to(to, sel)
            }
            Key::Home => { let to = if m.ctrl { 0 } else { self.line_range(self.caret).0 }; self.want_col = None; self.move_to(to, sel) }
            Key::End => { let to = if m.ctrl { self.chars.len() } else { self.line_range(self.caret).1 }; self.want_col = None; self.move_to(to, sel) }
            Key::Up | Key::Down => {
                if !self.multiline { let to = if key == Key::Up { 0 } else { self.chars.len() }; return self.move_to(to, sel); }
                let (s, e) = self.line_range(self.caret);
                let col = self.want_col.unwrap_or(self.caret - s);
                let target = if key == Key::Up {
                    if s == 0 { 0 } else { let (ps, pe) = self.line_range(s - 1); ps + col.min(pe - ps) }
                } else if e >= self.chars.len() { self.chars.len() } else { let (ns, ne) = self.line_range(e + 1); ns + col.min(ne - ns) };
                let r = self.move_to(target, sel);
                self.want_col = Some(col);
                r
            }
            Key::Enter => { if self.multiline { if self.insert_str("\n") { Outcome::Changed } else { Outcome::None } } else { Outcome::Submit } }
            Key::Tab => Outcome::Tab,
            Key::Escape => Outcome::Escape,
        }
    }

    // ---------------------------------------------------------------- geometry helpers (single line or per line)
    /// X offset of caret index `idx` inside its line, using the shaper's `measure`.
    pub fn x_of(&self, idx: usize, measure: &dyn Fn(&str) -> f32) -> f32 {
        let idx = idx.min(self.chars.len());
        let (s, _) = self.line_range(idx);
        let t = self.display_text_chars(s, idx);
        measure(&t)
    }
    fn display_text_chars(&self, a: usize, b: usize) -> String {
        if self.password { "*".repeat(b - a) } else { self.chars[a..b].iter().collect() }
    }
    /// Nearest caret index for a click at `x` on line `line` (0-based).
    pub fn index_at(&self, x: f32, line: usize, measure: &dyn Fn(&str) -> f32) -> usize {
        let lines = self.lines();
        let (s, e) = lines[line.min(lines.len() - 1)];
        let mut best = (s, f32::MAX);
        for b in self.bounds().into_iter().filter(|&b| b >= s && b <= e) {
            let d = (measure(&self.display_text_chars(s, b)) - x).abs();
            if d < best.1 { best = (b, d); }
        }
        best.0
    }
    pub fn line_of(&self, idx: usize) -> usize { self.chars[..idx.min(self.chars.len())].iter().filter(|&&c| c == '\n').count() }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn k(f: &mut TextField, c: &mut MemoryClipboard, key: Key, m: Mods) -> Outcome { f.handle_key(key, m, c) }
    fn typ(f: &mut TextField, c: &mut MemoryClipboard, s: &str) { for ch in s.chars() { k(f, c, Key::Char(ch), Mods::default()); } }
    const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false };
    const CTRL: Mods = Mods { shift: false, ctrl: true, alt: false };
    const NONE: Mods = Mods { shift: false, ctrl: false, alt: false };

    #[test]
    fn typing_backspace_delete() {
        let (mut f, mut c) = (TextField::new(false), MemoryClipboard::default());
        typ(&mut f, &mut c, "hello");
        assert_eq!(f.text(), "hello");
        k(&mut f, &mut c, Key::Backspace, NONE);
        k(&mut f, &mut c, Key::Left, NONE); k(&mut f, &mut c, Key::Left, NONE);
        k(&mut f, &mut c, Key::Delete, NONE);
        assert_eq!(f.text(), "hel"); // "hell" minus the char at the caret
        assert_eq!(f.caret(), 2);
    }
    #[test]
    fn selection_replace_and_shift_arrows() {
        let (mut f, mut c) = (TextField::with_text(false, "abcdef"), MemoryClipboard::default());
        k(&mut f, &mut c, Key::Home, NONE);
        for _ in 0..3 { k(&mut f, &mut c, Key::Right, SHIFT); }
        assert_eq!(f.selected_text(), "abc");
        typ(&mut f, &mut c, "X");
        assert_eq!(f.text(), "Xdef");
        k(&mut f, &mut c, Key::Char('a'), CTRL);
        assert_eq!(f.selected_text(), "Xdef");
    }
    #[test]
    fn word_movement_and_delete() {
        let (mut f, mut c) = (TextField::with_text(false, "foo bar  baz"), MemoryClipboard::default());
        k(&mut f, &mut c, Key::Left, CTRL);
        assert_eq!(f.caret(), 9);
        k(&mut f, &mut c, Key::Left, CTRL);
        assert_eq!(f.caret(), 4);
        k(&mut f, &mut c, Key::Backspace, CTRL);
        assert_eq!(f.text(), "bar  baz");
    }
    #[test]
    fn clipboard_cut_copy_paste_and_single_line_filter() {
        let (mut f, mut c) = (TextField::with_text(false, "hello world"), MemoryClipboard::default());
        f.select_word(1);
        k(&mut f, &mut c, Key::Char('x'), CTRL);
        assert_eq!(c.0, "hello");
        assert_eq!(f.text(), " world");
        c.0 = "a\nb\r".into();
        k(&mut f, &mut c, Key::Char('v'), CTRL);
        assert_eq!(f.text(), "ab world"); // newlines dropped in single-line fields
    }
    #[test]
    fn undo_redo_coalesces_typing() {
        let (mut f, mut c) = (TextField::new(false), MemoryClipboard::default());
        typ(&mut f, &mut c, "abc");
        k(&mut f, &mut c, Key::Left, NONE); // breaks the typing run
        typ(&mut f, &mut c, "X");
        assert_eq!(f.text(), "abXc");
        k(&mut f, &mut c, Key::Char('z'), CTRL);
        assert_eq!(f.text(), "abc");
        k(&mut f, &mut c, Key::Char('z'), CTRL);
        assert_eq!(f.text(), "");
        k(&mut f, &mut c, Key::Char('y'), CTRL);
        assert_eq!(f.text(), "abc");
    }
    #[test]
    fn multiline_up_down_keeps_column_and_enter() {
        let (mut f, mut c) = (TextField::new(true), MemoryClipboard::default());
        typ(&mut f, &mut c, "abcd");
        k(&mut f, &mut c, Key::Enter, NONE);
        typ(&mut f, &mut c, "ef");
        k(&mut f, &mut c, Key::Enter, NONE);
        typ(&mut f, &mut c, "ghijk");
        assert_eq!(f.lines().len(), 3);
        k(&mut f, &mut c, Key::Up, NONE); // col 5 -> clamps to end of "ef" (col 2)
        assert_eq!(f.caret(), 7);
        k(&mut f, &mut c, Key::Up, NONE); // wants col 5 -> "abcd" end (col 4)
        assert_eq!(f.caret(), 4);
        k(&mut f, &mut c, Key::Down, NONE); k(&mut f, &mut c, Key::Down, NONE);
        assert_eq!(f.caret(), f.len()); // back to col 5 on last line
        assert_eq!(f.line_of(f.caret()), 2);
    }
    #[test]
    fn single_line_enter_submits_and_max_len() {
        let (mut f, mut c) = (TextField::new(false), MemoryClipboard::default());
        f.max_len = 3;
        typ(&mut f, &mut c, "abcdef");
        assert_eq!(f.text(), "abc");
        assert_eq!(k(&mut f, &mut c, Key::Enter, NONE), Outcome::Submit);
        assert_eq!(k(&mut f, &mut c, Key::Tab, NONE), Outcome::Tab);
    }
    #[test]
    fn bangla_clusters_move_together_backspace_by_codepoint() {
        // ক্ষ = ক + virama + ষ ; "বাংলা" = ব া ং ল া
        let (mut f, mut c) = (TextField::with_text(false, "ক্ষ"), MemoryClipboard::default());
        assert_eq!(f.len(), 3);
        k(&mut f, &mut c, Key::Left, NONE);
        assert_eq!(f.caret(), 0); // whole conjunct in one step
        k(&mut f, &mut c, Key::End, NONE);
        k(&mut f, &mut c, Key::Backspace, NONE);
        assert_eq!(f.text(), "ক্"); // one code point removed
        let g = TextField::with_text(false, "বাংলা");
        assert_eq!(g.bounds(), vec![0, 3, 5]); // [বাং][লা]
    }
    #[test]
    fn mouse_mapping_and_password() {
        let measure = |s: &str| s.chars().count() as f32 * 10.0;
        let mut f = TextField::with_text(false, "abcd");
        assert_eq!(f.index_at(24.0, 0, &measure), 2);
        assert_eq!(f.index_at(1000.0, 0, &measure), 4);
        assert_eq!(f.x_of(3, &measure), 30.0);
        f.password = true;
        assert_eq!(f.display_text(), "****");
        f.set_caret(1, false); f.set_caret(3, true);
        assert_eq!(f.selection(), Some((1, 3)));
    }
}
