//! Motion of what a material's own `vertex` function does: wind in grass,
//! waves on water, particles moved by a GPU buffer.
//!
//! The motion pass (temporal antialiasing, temporal AO) runs a model's
//! `vertex` twice: as of this frame, and as of the last one. For the second
//! run, the game's code is rewritten (lines kept, so errors point where they
//! did) to read last frame's values through two prelude helpers:
//!
//! - `frame.time` becomes `motion_frame_time()`: the frame's time, or the
//!   last drawn frame's (`frame.time.zw`) while `motion_past_time` is set;
//! - `items[i]` becomes `items[motion_item(u32(i))]`: the element, or its
//!   copy from the last frame (past the buffer's live elements, see
//!   `Graphics::set_gpu_buffer_motion`) while `motion_past_items` is set.
//!
//! Both flags are private variables only the motion pass's vertex entry
//! points set, so every other pass computes exactly what it did. Which
//! shaders depend on them ([`VertexPast`]) is read from the compiled
//! module: their models are drawn into the motion image even when their
//! matrix holds still (the still motion draw), and nothing else is.

use std::borrow::Cow;

/// What a material shader's `vertex` function reads that has a last-frame
/// value: models drawn with it can move while their matrix holds still.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) struct VertexPast {
    /// The time (`frame.time`), or `drawing_last_frame()`.
    pub time: bool,
    /// A GPU buffer's elements (`items`).
    pub items: bool,
}

impl VertexPast {
    /// Whether anything does: the shader gets a still motion pipeline.
    pub fn any(self) -> bool {
        self.time || self.items
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: shaders composed on this thread are not rewritten and have no
    /// still motion draws (the behaviour before), for negative controls.
    pub(crate) static NO_VERTEX_PAST: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Off for the whole process: the rendering baseline's control
/// (`PONKO_RENDER_VERTEX_MOTION=0`, see `scene_bench.rs`), set before any
/// shader is made.
static OFF: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Turns the rewrite and the still motion draws off (`true`) for shaders
/// made from now on and every frame: what the renderer did before them.
#[cfg(all(not(target_arch = "wasm32"), any(test, feature = "bench-internals")))]
pub(super) fn set_off(off: bool) {
    OFF.store(off, std::sync::atomic::Ordering::Relaxed);
}

/// Whether the rewrite and the still motion draws are off: for the
/// rendering baseline's control, or on this thread (test controls).
pub(super) fn disabled() -> bool {
    #[cfg(test)]
    if NO_VERTEX_PAST.get() {
        return true;
    }
    OFF.load(std::sync::atomic::Ordering::Relaxed)
}

/// What the engine adds after the GPU buffer bindings of a material shader:
/// the element a read of `items` takes, as of this frame or the last.
pub(super) const ITEM_HELPER: &str = "
// The element of `items` the game's shader reads: as of the last frame
// while the motion pass runs `vertex` for it (a buffer that keeps its last
// frame has that copy past its live elements, at `static_indices[0]`).
fn motion_item(i: u32) -> u32 {
    if motion_past_items {
        return i + static_indices[0];
    }
    return i;
}
";

/// The game's shader code reading `frame.time` and `items[..]` through the
/// helpers that can give last frame's values (see the module's docs).
/// Lines don't move. Code in comments, members of other values (`x.items`)
/// and pointers (`&frame.time`) are left alone.
pub(super) fn with_past(user: &str) -> Cow<'_, str> {
    if disabled() || !(user.contains("time") || user.contains("items")) {
        return Cow::Borrowed(user);
    }
    let rewritten = rewrite(user);
    if rewritten == user {
        Cow::Borrowed(user)
    } else {
        Cow::Owned(rewritten)
    }
}

fn ident_char(c: u8) -> bool {
    c.is_ascii_alphanumeric() || c == b'_'
}

/// The end of the whitespace and comments from `at`.
fn skip_blank(s: &[u8], mut at: usize) -> usize {
    loop {
        while at < s.len() && s[at].is_ascii_whitespace() {
            at += 1;
        }
        match comment_end(s, at) {
            Some(end) => at = end,
            None => return at,
        }
    }
}

/// Where the comment starting at `at` ends, `None` if none starts there.
/// Block comments nest, as WGSL's do.
fn comment_end(s: &[u8], at: usize) -> Option<usize> {
    if s.get(at) != Some(&b'/') {
        return None;
    }
    match s.get(at + 1) {
        Some(b'/') => Some(s[at..].iter().position(|&c| c == b'\n').map_or(s.len(), |n| at + n)),
        Some(b'*') => {
            let (mut depth, mut i) = (1, at + 2);
            while i < s.len() && depth > 0 {
                if s[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if s[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            Some(i)
        }
        _ => None,
    }
}

/// The last character before `at` that is not whitespace (comments before
/// a token are rare enough to ignore here).
fn before(s: &[u8], at: usize) -> Option<u8> {
    s[..at].iter().rev().find(|c| !c.is_ascii_whitespace()).copied()
}

/// The newlines in `s`, to keep a rewritten span's lines.
fn newlines(s: &[u8]) -> String {
    s.iter().filter(|&&c| c == b'\n').map(|_| '\n').collect()
}

fn rewrite(user: &str) -> String {
    let s = user.as_bytes();
    let mut out = String::with_capacity(user.len() + 64);
    // `copied`: where the text not yet copied to `out` starts.
    let (mut at, mut copied) = (0, 0);
    while at < s.len() {
        if let Some(end) = comment_end(s, at) {
            at = end;
            continue;
        }
        if !ident_char(s[at]) || (at > 0 && ident_char(s[at - 1])) {
            at += 1;
            continue;
        }
        let end = at + s[at..].iter().position(|&c| !ident_char(c)).unwrap_or(s.len() - at);
        let word = &user[at..end];
        let member = before(s, at) == Some(b'.');
        if word == "frame" && !member && before(s, at) != Some(b'&') {
            let dot = skip_blank(s, end);
            if s.get(dot) == Some(&b'.') {
                let name = skip_blank(s, dot + 1);
                let name_end = name + s[name..].iter().position(|&c| !ident_char(c)).unwrap_or(s.len() - name);
                if &user[name..name_end] == "time" {
                    out.push_str(&user[copied..at]);
                    out.push_str("motion_frame_time()");
                    out.push_str(&newlines(&s[at..name_end]));
                    (at, copied) = (name_end, name_end);
                    continue;
                }
            }
        }
        if word == "items" && !member {
            let open = skip_blank(s, end);
            if s.get(open) == Some(&b'[')
                && let Some(close) = matching_bracket(s, open)
            {
                out.push_str(&user[copied..open]);
                out.push_str("[motion_item(u32(");
                // An index can read `items` too.
                out.push_str(&rewrite(&user[open + 1..close]));
                out.push_str("))]");
                (at, copied) = (close + 1, close + 1);
                continue;
            }
        }
        at = end;
    }
    out.push_str(&user[copied..]);
    out
}

/// The `]` closing the `[` at `open`, skipping nested brackets and
/// comments; `None` if it doesn't close (the compiler reports that).
fn matching_bracket(s: &[u8], open: usize) -> Option<usize> {
    let (mut depth, mut i) = (0usize, open);
    while i < s.len() {
        if let Some(end) = comment_end(s, i) {
            i = end;
            continue;
        }
        match s[i] {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// What `vertex` reads that has a last-frame value, from the composed
/// shader `source` of the game's (already rewritten) code `user`. Code
/// that reads neither is not compiled again to find out.
pub(super) fn vertex_past(user: &str, source: &str) -> VertexPast {
    use wgpu::naga;
    if disabled()
        || !(user.contains("motion_frame_time") || user.contains("motion_item") || user.contains("drawing_last_frame"))
    {
        return VertexPast::default();
    }
    let Ok(module) = naga::front::wgsl::parse_str(source) else {
        return VertexPast::default();
    };
    let Ok(info) = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::all())
        .validate(&module)
    else {
        return VertexPast::default();
    };
    let Some((vertex, _)) = module
        .functions
        .iter()
        .find(|(_, f)| f.name.as_deref() == Some("vertex"))
    else {
        return VertexPast::default();
    };
    // `info[f][g]` is how function `f` (with what it calls) uses global `g`.
    let reads = |name: &str| {
        module
            .global_variables
            .iter()
            .any(|(g, global)| global.name.as_deref() == Some(name) && !info[vertex][g].is_empty())
    };
    VertexPast {
        time: reads("motion_past_time"),
        items: reads("motion_past_items"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_time_and_item_reads_are_rewritten_and_lines_kept() {
        let user = "fn vertex(v: Vertex) -> Vertex {\n\
            var o = v; // frame.time in a comment\n\
            o.position.x += sin(frame.time.x) + items[v.instance_index].x;\n\
            o.position.y += frame\n  . time.y + items[items[0].w].y; /* items[1] */\n\
            return o;\n}";
        let out = with_past(user);
        assert_eq!(out.lines().count(), user.lines().count(), "{out}");
        assert!(out.contains("// frame.time in a comment"), "{out}");
        assert!(out.contains("/* items[1] */"), "{out}");
        assert!(
            out.contains("sin(motion_frame_time().x) + items[motion_item(u32(v.instance_index))].x"),
            "{out}"
        );
        assert!(
            out.contains("items[motion_item(u32(items[motion_item(u32(0))].w))].y"),
            "nested reads: {out}"
        );
        assert!(out.contains("motion_frame_time()\n"), "{out}");
    }

    #[test]
    fn members_pointers_and_other_names_are_left_alone() {
        for user in [
            "fn f(s: S) -> f32 { return s.frame.time + s.items[0]; }",
            "fn f() { let p = &frame.time; }",
            "fn f() { let x = myframe.time + frame.timer + some_items[0]; }",
            "fn f() -> f32 { return 1.0; }",
        ] {
            assert_eq!(with_past(user), user);
        }
    }

    #[test]
    fn only_shaders_whose_vertex_reads_the_past_get_a_still_motion_draw() {
        let past = |user: &str| {
            let user = with_past(user);
            vertex_past(&user, &super::super::shaders::compose_material(&user, true, true))
        };
        assert_eq!(past(""), VertexPast::default());
        assert_eq!(
            past("fn vertex(v: Vertex) -> Vertex { var o = v; o.position.y += 1.0; return o; }"),
            VertexPast::default()
        );
        // Time in `surface` only: the corners don't move.
        assert_eq!(
            past("fn surface(s: Surface) -> Surface { var o = s; o.alpha = sin(frame.time.x); return o; }"),
            VertexPast::default()
        );
        // Through a helper.
        let wind = "fn sway() -> f32 { return sin(frame.time.x); }\n\
            fn vertex(v: Vertex) -> Vertex { var o = v; o.position.x += sway(); return o; }";
        assert_eq!(
            past(wind),
            VertexPast {
                time: true,
                items: false
            }
        );
        let buffer =
            "fn vertex(v: Vertex) -> Vertex { var o = v; o.position += items[v.instance_index].xyz; return o; }";
        assert_eq!(
            past(buffer),
            VertexPast {
                time: false,
                items: true
            }
        );
        let own = "fn vertex(v: Vertex) -> Vertex { var o = v; o.position.x += select(material.custom[0].x, material.custom[1].x, drawing_last_frame()); return o; }";
        assert!(past(own).time);
    }
}
