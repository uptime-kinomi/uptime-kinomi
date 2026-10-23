//! SVG badges in the style of shields.io (SPEC.md §8.8): plain SVG, no
//! scripts, no external resources, all text XML-escaped.

use super::html::esc;

/// Approximate width of Verdana 11px text (no font metrics on board).
fn text_width(text: &str) -> u32 {
    let width: f64 = text
        .chars()
        .map(|c| match c {
            'i' | 'l' | 'j' | 'I' | '.' | ',' | ':' | ';' | '|' | '!' | '\'' | ' ' => 3.5,
            'm' | 'w' | 'M' | 'W' | '%' => 10.0,
            c if c.is_ascii_uppercase() || c.is_ascii_digit() => 7.5,
            _ => 6.5,
        })
        .sum();
    width.ceil() as u32
}

pub fn color(state: &str) -> &'static str {
    match state {
        "UP" => "#2ea44f",
        "WARN" | "PENDING" => "#dfb317",
        "DOWN" => "#e05d44",
        "MAINTENANCE" => "#3b6fd4",
        _ => "#9f9f9f",
    }
}

pub fn badge(label: &str, value: &str, color: &str) -> String {
    let label: String = label.chars().take(40).collect();
    let (lw, vw) = (text_width(&label) + 12, text_width(value) + 12);
    let total = lw + vw;
    let (label, value) = (esc(&label), esc(value));
    format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"{total}\" height=\"20\" role=\"img\" aria-label=\"{label}: {value}\">\
<title>{label}: {value}</title>\
<rect width=\"{lw}\" height=\"20\" rx=\"3\" fill=\"#555\"/>\
<rect x=\"{lw}\" width=\"{vw}\" height=\"20\" rx=\"3\" fill=\"{color}\"/>\
<rect x=\"{lw}\" width=\"4\" height=\"20\" fill=\"{color}\"/>\
<g fill=\"#fff\" text-anchor=\"middle\" font-family=\"Verdana,DejaVu Sans,sans-serif\" font-size=\"11\">\
<text x=\"{}\" y=\"14\">{label}</text><text x=\"{}\" y=\"14\">{value}</text></g></svg>\n",
        lw / 2,
        lw + vw / 2
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escaped_and_script_free() {
        let svg = badge("<script>x</script>", "UP", color("UP"));
        assert!(!svg.contains("<script"));
        assert!(svg.contains("&lt;script&gt;"));
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.contains("fill=\"#2ea44f\""));
    }

    #[test]
    fn widths_grow_with_text() {
        assert!(text_width("My long monitor name") > text_width("short"));
    }
}
