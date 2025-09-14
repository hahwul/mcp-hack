/*!
format.rs

Fancy formatting utilities for `mcp-hack` CLI (human output paths).

Goals:
  - Provide consistent colorful / boxed / tabular formatting primitives.
  - Centralize style decision logic (e.g., NO_COLOR env, future --plain / --no-emoji).
  - Keep zero non-std dependencies (no terminal crates) for simplicity.
  - Degrade gracefully when ANSI disabled (NO_COLOR set).

Current Design (Baseline):
  - Color + box styling ENABLED by default (per user decision).
  - Emoji usage ENABLED by default (NO_EMOJI env = disable).
  - Wrap / truncate logic kept conservative; width detection is best-effort via:
        env COLUMNS -> parse -> clamp (40..=220) else default 100.

Future Extension Points:
  - Integrate global CLI flags: --plain, --no-emoji, --wide, --no-border.
  - Adaptive wrapping based on actual terminal (ioctl/TIOCGWINSZ) if needed.
  - Multi‑column layout / automatic column priority reduction.
  - Markdown / HTML export backend.

Public API Summary:
  - StyleOptions::detect() -> StyleOptions
  - color(role, text, &StyleOptions) -> String
  - emoji(tag, &StyleOptions) -> &'static str
  - box_header(title, subtitle_opt, &StyleOptions) -> String
  - table(headers, rows, TableOpts, &StyleOptions) -> String
  - wrap_text(s, max_width) -> Vec<String>
  - truncate_ellipsis(s, max_chars) -> String

Usage Example (inside a command module):
  let style = StyleOptions::detect();
  println!("{}", box_header("Tools (5)", Some("target=... • 12 ms"), &style));
  let tbl = table(
      &["#", "NAME", "PARAMS", "DESCRIPTION"],
      &rows,
      TableOpts::default(),
      &style
  );
  println!("{tbl}");

NOTE:
  - This module avoids logging or printing directly (returns formatted strings).
  - JSON output paths SHOULD NOT use these helpers to keep machine output clean.

License: MIT (inherits project license)
*/

use std::borrow::Cow;

/* -------------------------------------------------------------------------- */
/* Style Options                                                              */
/* -------------------------------------------------------------------------- */

#[derive(Debug, Clone)]
pub struct StyleOptions {
    pub use_color: bool,
    pub use_emoji: bool,
    pub term_width: usize,
    pub box_style: BoxStyle,
    pub padding: usize,
}

#[derive(Debug, Clone, Copy)]
pub enum BoxStyle {
    Light,   // ─ │ ┌ ┐ └ ┘
    Rounded, // ╭ ╮ ╰ ╯
}

impl Default for StyleOptions {
    fn default() -> Self {
        Self::detect()
    }
}

impl StyleOptions {
    pub fn detect() -> Self {
        let no_color = std::env::var_os("NO_COLOR").is_some();
        let no_emoji = std::env::var_os("NO_EMOJI").is_some();
        let use_color = !no_color;
        let use_emoji = !no_emoji;

        let width = std::env::var("COLUMNS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .map(|w| w.clamp(40, 220))
            .unwrap_or(100);

        StyleOptions {
            use_color,
            use_emoji,
            term_width: width,
            box_style: BoxStyle::Light,
            padding: 1,
        }
    }
}

/* -------------------------------------------------------------------------- */
/* Color / Emoji                                                              */
/* -------------------------------------------------------------------------- */

#[derive(Debug, Clone, Copy)]
pub enum Role {
    Primary,
    Secondary,
    Accent,
    Success,
    Warning,
    Error,
    Dim,
    Invert,
    Bold,
}

pub fn color(role: Role, text: impl AsRef<str>, style: &StyleOptions) -> String {
    if !style.use_color {
        return text.as_ref().to_string();
    }
    let code = match role {
        Role::Primary => "38;5;45",    // cyan-ish
        Role::Secondary => "38;5;250", // gray
        Role::Accent => "38;5;213",    // magenta/pink
        Role::Success => "38;5;82",    // green
        Role::Warning => "38;5;214",   // orange
        Role::Error => "38;5;196",     // red
        Role::Dim => "2",              // faint
        Role::Invert => "7",
        Role::Bold => "1",
    };
    format!("\x1b[{code}m{}\x1b[0m", text.as_ref())
}

pub fn emoji(tag: &str, style: &StyleOptions) -> &'static str {
    if !style.use_emoji {
        return "";
    }
    match tag {
        "success" => "✔",
        "error" => "✖",
        "warn" => "⚠",
        "info" => "ℹ",
        "rocket" => "🚀",
        "tool" => "🛠",
        "spark" => "✨",
        "list" => "📜",
        "clock" => "⏱",
        _ => "",
    }
}

/* -------------------------------------------------------------------------- */
/* Box Header                                                                 */
/* -------------------------------------------------------------------------- */

pub fn box_header(
    title: impl AsRef<str>,
    subtitle: Option<impl AsRef<str>>,
    style: &StyleOptions,
) -> String {
    let title = title.as_ref();
    let sub = subtitle.as_ref().map(|s| s.as_ref());

    let (h, v, tl, tr, bl, br) = match style.box_style {
        BoxStyle::Light => ('─', '│', '┌', '┐', '└', '┘'),
        BoxStyle::Rounded => ('─', '│', '╭', '╮', '╰', '╯'),
    };

    let content_width = style.term_width.min(200).max(20);
    let padding = style.padding;
    let mut lines: Vec<String> = Vec::new();

    // Title formatted
    let title_styled = color(Role::Primary, title, style);

    // Compact subtitle formatting: collapse " ms" -> "ms" to reduce wrap chance
    let sub_compact = sub.map(|s| {
        let mut owned = s.to_string();
        owned = owned.replace(" ms", "ms");
        owned
    });
    let subtitle_line = sub_compact.map(|s| color(Role::Secondary, s, style));

    let inner_title = match &subtitle_line {
        Some(sline) => format!("{title_styled}  {}", sline),
        None => title_styled,
    };

    let inner_len = strip_ansi(&inner_title).chars().count();
    // Box width = min(requested, inner_len + borders + padding)
    let total_inner = (inner_len + padding * 2).min(content_width - 2);
    let mut total_width = total_inner + 2; // plus vertical borders

    // Top border
    lines.push(format!(
        "{tl}{hline}{tr}",
        tl = tl,
        hline = h.to_string().repeat(total_width - 2),
        tr = tr
    ));

    // Content (wrap if needed) with simple widow prevention
    let mut wrap_width = total_width - 2 - padding * 2;
    let mut wrapped = wrap_text(&inner_title, wrap_width);

    if wrapped.len() > 1 {
        let last_len = display_width(&wrapped[wrapped.len() - 1]);
        // If last line is a very short widow (<=3 chars), try to expand width or merge
        if last_len > 0 && last_len <= 3 {
            // Try width expansion first (if terminal space remains)
            let max_inner_allowed = content_width - 2;
            if total_width < content_width {
                let extra_possible = max_inner_allowed + 2 - total_width;
                // Add just enough to comfortably fit the widow onto previous line
                // Heuristic: expand by at most 6 chars
                let expand_by = extra_possible.min(6);
                if expand_by > 0 {
                    total_width += expand_by;
                    wrap_width += expand_by;
                    wrapped = wrap_text(&inner_title, wrap_width);
                }
            }
            // If still widow after expansion, merge it manually
            if wrapped.len() > 1 {
                let last_len2 = display_width(&wrapped[wrapped.len() - 1]);
                if last_len2 > 0 && last_len2 <= 3 {
                    let last = wrapped.pop().unwrap();
                    if let Some(prev) = wrapped.last_mut() {
                        prev.push(' ');
                        prev.push_str(&last);
                    } else {
                        wrapped.push(last);
                    }
                }
            }
        }
    }

    for w in wrapped {
        let raw_len = strip_ansi(&w).chars().count();
        let space_pad = total_width - 2 - padding * 2 - raw_len;
        let pad_str = " ".repeat(padding);
        let spaces_str = if space_pad > 0 {
            " ".repeat(space_pad)
        } else {
            String::new()
        };
        lines.push(format!(
            "{v}{pad}{w}{spaces}{pad}{v}",
            v = v,
            pad = pad_str,
            w = w,
            spaces = spaces_str,
        ));
    }

    // Bottom border
    lines.push(format!(
        "{bl}{hline}{br}",
        bl = bl,
        hline = h.to_string().repeat(total_width - 2),
        br = br
    ));

    lines.join("\n")
}

/* -------------------------------------------------------------------------- */
/* Table Rendering                                                             */
/* -------------------------------------------------------------------------- */

#[derive(Debug, Clone)]
pub struct TableOpts {
    pub max_width: usize,
    pub truncate: bool,
    pub header_sep: bool,
    pub zebra: bool,
    pub min_col_width: usize,
}

impl Default for TableOpts {
    fn default() -> Self {
        Self {
            max_width: 0, // 0 -> auto style.term_width
            truncate: true,
            header_sep: true,
            zebra: false,
            min_col_width: 2,
        }
    }
}

pub fn table(
    headers: &[&str],
    rows: &[Vec<String>],
    opts: TableOpts,
    style: &StyleOptions,
) -> String {
    if headers.is_empty() {
        return String::new();
    }
    let col_count = headers.len();
    let width_limit = if opts.max_width == 0 {
        style.term_width
    } else {
        opts.max_width.min(style.term_width)
    };

    // Compute max content width per column
    let mut widths: Vec<usize> = headers.iter().map(|h| h.len()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate().take(col_count) {
            let w = strip_ansi(cell).chars().count();
            if w > widths[i] {
                widths[i] = w;
            }
        }
    }

    // Adjust if total exceeds width_limit (primitive greedy shrink)
    let total_raw: usize = widths.iter().sum::<usize>() + (col_count - 1) * 2;
    if total_raw > width_limit {
        // compute overflow
        let mut overflow = total_raw - width_limit;
        // shrink from the widest columns
        let mut ordered: Vec<(usize, usize)> = widths.iter().copied().enumerate().collect();
        ordered.sort_by(|a, b| b.1.cmp(&a.1)); // desc by width
        for (idx, _) in ordered {
            if overflow == 0 {
                break;
            }
            let target = widths[idx];
            if target > opts.min_col_width {
                let shrink = (target - opts.min_col_width).min(overflow);
                widths[idx] -= shrink;
                overflow -= shrink;
            }
        }
    }

    // Render
    let mut out = String::new();

    // Header
    for (i, h) in headers.iter().enumerate() {
        if i > 0 {
            out.push_str("  ");
        }
        let cell = pad_or_truncate(h, widths[i], opts.truncate);
        out.push_str(&color(Role::Accent, cell, style));
    }
    out.push('\n');

    if opts.header_sep {
        let mut sep = String::new();
        for (i, _) in headers.iter().enumerate() {
            if i > 0 {
                sep.push_str("  ");
            }
            sep.push_str(&"-".repeat(widths[i]));
        }
        out.push_str(&color(Role::Dim, sep, style));
        out.push('\n');
    }

    for (r_idx, row) in rows.iter().enumerate() {
        for c in 0..col_count {
            if c > 0 {
                out.push_str("  ");
            }
            let raw = row.get(c).map(|s| s.as_str()).unwrap_or("");
            let cell = pad_or_truncate(raw, widths[c], opts.truncate);
            if opts.zebra && (r_idx % 2 == 1) && style.use_color {
                out.push_str(&color(Role::Dim, cell, style));
            } else {
                out.push_str(&cell);
            }
        }
        if r_idx + 1 < rows.len() {
            out.push('\n');
        }
    }

    out
}

fn pad_or_truncate(s: &str, width: usize, truncate: bool) -> String {
    let len = display_width(s);
    if len == width {
        return s.to_string();
    }
    if len < width {
        let pad = width - len;
        return format!("{s}{}", " ".repeat(pad));
    }
    if !truncate {
        return s.to_string();
    }
    if width <= 1 {
        return "…".to_string();
    }
    // naive char-based truncate
    let mut out = String::new();
    for ch in s.chars() {
        if display_width(&out) + ch.len_utf8() >= width - 1 {
            break;
        }
        out.push(ch);
    }
    out.push('…');
    let final_len = display_width(&out);
    if final_len < width {
        out.push_str(&" ".repeat(width - final_len));
    }
    out
}

/* -------------------------------------------------------------------------- */
/* Text Helpers                                                                */
/* -------------------------------------------------------------------------- */

pub fn wrap_text(s: &str, max_width: usize) -> Vec<String> {
    if max_width == 0 {
        return vec![s.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in s.split_whitespace() {
        if display_width(&current) + word.len() + 1 > max_width && !current.is_empty() {
            lines.push(current);
            current = String::new();
        }
        if !current.is_empty() {
            current.push(' ');
        }
        current.push_str(word);
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

pub fn truncate_ellipsis(s: &str, max_chars: usize) -> String {
    if max_chars == 0 {
        return String::new();
    }
    let raw_len = s.chars().count();
    if raw_len <= max_chars {
        return s.to_string();
    }
    if max_chars <= 1 {
        return "…".into();
    }
    let mut out = String::new();
    for ch in s.chars().take(max_chars - 1) {
        out.push(ch);
    }
    out.push('…');
    out
}

/* -------------------------------------------------------------------------- */
/* ANSI / Width Utilities                                                      */
/* -------------------------------------------------------------------------- */

fn strip_ansi(s: &str) -> Cow<'_, str> {
    // Minimal implementation (no regex) — scans for ESC '[' ... 'm'
    if !s.contains('\x1b') {
        return Cow::Borrowed(s);
    }
    let mut buf = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == 0x1B {
            // attempt to skip CSI
            if i + 1 < bytes.len() && bytes[i + 1] == b'[' {
                i += 2;
                while i < bytes.len()
                    && !((bytes[i] >= b'A' && bytes[i] <= b'Z')
                        || (bytes[i] >= b'a' && bytes[i] <= b'z'))
                {
                    i += 1;
                }
                if i < bytes.len() {
                    i += 1;
                }
                continue;
            }
        }
        buf.push(bytes[i] as char);
        i += 1;
    }
    Cow::Owned(buf)
}

fn display_width(s: &str) -> usize {
    strip_ansi(s).chars().count()
}

/* -------------------------------------------------------------------------- */
/* Tests                                                                       */
/* -------------------------------------------------------------------------- */

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_box_header_basic() {
        let style = StyleOptions::detect();
        let b = box_header("Title", Some("sub info"), &style);
        assert!(b.contains("Title"));
    }

    #[test]
    fn test_table_basic() {
        let style = StyleOptions::detect();
        let t = table(
            &["A", "B"],
            &[
                vec!["x".into(), "y".into()],
                vec!["longer".into(), "val".into()],
            ],
            TableOpts::default(),
            &style,
        );
        assert!(t.contains("A"));
        assert!(t.contains("longer"));
    }

    #[test]
    fn test_wrap_text() {
        let lines = wrap_text("hello world from formatting", 10);
        assert!(lines.len() >= 2);
    }

    #[test]
    fn test_truncate() {
        let s = truncate_ellipsis("abcdef", 4);
        assert_eq!(s, "abc…");
    }

    #[test]
    fn test_strip_ansi() {
        let colored = "\x1b[31mRED\x1b[0m";
        let plain = strip_ansi(colored);
        assert_eq!(plain, "RED");
    }
}
