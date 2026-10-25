//! Safe offline document view. Source HTML is always text; only explicit known
//! Wiki links or HTTPS links become anchors. No images/scripts are fetched.
use super::{escape, page};
use crate::{Error, Result, api::Request, runtime::Engine};
use std::sync::Arc;

pub(super) fn view(engine: &Arc<Engine>, path: &str) -> Result<String> {
    let pages = crate::wiki::pages()?;
    let mut body = String::from("<p><a href=\"/console\">Workspace</a></p><nav><ul>");
    for page in &pages {
        body +=
            &format!("<li><a href=\"/wiki/{}\">{}</a></li>", escape(&page.id), escape(&page.title));
    }
    body += "</ul></nav>";
    if path == "/wiki" {
        body += "<p>Eight offline Ryoku guides with pinned source provenance and Tumbleweed views. Reading a guide executes none of its commands. Index a guide explicitly to use it as assistant evidence.</p><p><a href=\"/wiki/license\">License and source notice</a></p>";
    } else if path == "/wiki/license" {
        body += &format!(
            "<pre>{}</pre><pre>{}</pre>",
            escape(include_str!("../../wiki/NOTICE.md")),
            escape(include_str!("../../wiki/COPYING"))
        );
    } else {
        let id = path.strip_prefix("/wiki/").ok_or(Error::NotFound)?;
        let (id, original) =
            id.strip_suffix("/upstream").map(|id| (id, true)).unwrap_or((id, false));
        let guide = crate::wiki::page(id, original)?;
        body += &format!(
            "<p>Pinned upstream reference: {} · blob {} · SHA-256 {}</p>",
            escape(&guide.upstream_path),
            guide.upstream_blob,
            guide.upstream_sha256
        );
        body += &format!("<p>Current view SHA-256: {}</p>", guide.sha256);
        if original {
            body += "<p>Exact upstream reference edition. Distribution and runtime assumptions may differ from Tumbleweed.</p>";
            body += &format!("<p><a href=\"/wiki/{id}\">Tumbleweed view</a></p>");
        } else {
            body += &format!("<p><a href=\"/wiki/{id}/upstream\">Exact original edition</a></p>");
            let index = engine.dispatch(Request::GetIndex {})?;
            let name = format!("wiki/{id}.md");
            let existing = index["documents"]
                .as_array()
                .ok_or(Error::Integrity)?
                .iter()
                .find(|document| document["name"] == name);
            body += &format!(
                "<form method=\"post\" action=\"/console/action\"><input type=\"hidden\" name=\"action\" value=\"index_wiki\"><input type=\"hidden\" name=\"id\" value=\"{id}\"><input type=\"hidden\" name=\"sha256\" value=\"{}\">",
                guide.sha256
            );
            if let Some(existing) = existing.filter(|document| document["sha256"] != guide.sha256) {
                let old = existing["sha256"].as_str().ok_or(Error::Integrity)?;
                body += &format!(
                    "<p>An indexed document named {} already has a different digest: {}.</p><input type=\"hidden\" name=\"replace_sha256\" value=\"{}\"><label><input type=\"checkbox\" name=\"confirm_replace\" value=\"yes\" required>Replace only this named indexed document with the guide below.</label>",
                    escape(&name),
                    escape(old),
                    escape(old)
                );
            }
            body += "<button>Index this guide for assistant evidence</button></form>";
        }
        body += &render(&guide.content);
    }
    Ok(page("Offline Ryoku Wiki", &body))
}
fn inline(value: &str) -> String {
    let mut out = String::new();
    let mut rest = value;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix('`')
            && let Some(end) = after.find('`')
        {
            out += &format!("<code>{}</code>", escape(&after[..end]));
            rest = &after[end + 1..];
            continue;
        }
        if let Some(after) = rest.strip_prefix("**")
            && let Some(end) = after.find("**")
        {
            out += &format!("<strong>{}</strong>", escape(&after[..end]));
            rest = &after[end + 2..];
            continue;
        }
        if let Some(after) = rest.strip_prefix('[')
            && let Some(mid) = after.find("](")
            && let Some(end) = after[mid + 2..].find(')')
        {
            let target = &after[mid + 2..mid + 2 + end];
            let link = crate::wiki::link(target).or_else(|| {
                (target.starts_with("https://")
                    && target.len() <= 2048
                    && !target.chars().any(|c| c.is_control() || c.is_whitespace())
                    && !target.contains(['"', '\'', '<', '>']))
                .then(|| target.to_owned())
            });
            if let Some(link) = link {
                out += &format!(
                    "<a href=\"{}\" rel=\"noreferrer noopener\">{}</a>",
                    escape(&link),
                    escape(&after[..mid])
                );
                rest = &after[mid + 2 + end + 1..];
                continue;
            }
        }
        let Some(character) = rest.chars().next() else { break };
        out += &escape(&character.to_string());
        rest = &rest[character.len_utf8()..];
    }
    out
}
fn render(markdown: &str) -> String {
    fn paragraph(out: &mut String, text: &mut String) {
        if !text.is_empty() {
            *out += &format!("<p>{}</p>", inline(text));
            text.clear();
        }
    }
    let mut out = String::from("<article>");
    let mut text = String::new();
    let mut code = false;
    for line in markdown.lines() {
        if line.starts_with("```") {
            paragraph(&mut out, &mut text);
            out += if code { "</code></pre>" } else { "<pre><code>" };
            code = !code;
            continue;
        }
        if code {
            out += &escape(line);
            out.push('\n');
            continue;
        }
        if line.trim().is_empty() {
            paragraph(&mut out, &mut text);
            continue;
        }
        let level = line.bytes().take_while(|byte| *byte == b'#').count();
        if (1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ') {
            paragraph(&mut out, &mut text);
            out += &format!("<h{level}>{}</h{level}>", inline(&line[level + 1..]));
            continue;
        }
        if !text.is_empty() {
            text.push(' ');
        }
        text += line;
    }
    paragraph(&mut out, &mut text);
    if code {
        out += "</code></pre>";
    }
    out += "</article>";
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn offline_rendering_keeps_code_and_unicode_and_never_activates_source_html_or_bad_urls() {
        let markdown = "# Wiki Δ\n\n[Linux](linux-basics.md) and [Docs](https://docs.example.invalid/)\n\n<script>raw</script> [bad](javascript:alert(1)) [path](../../private)\n\n```html\n<img src=\"https://remote.invalid/image\" onerror=\"bad()\">\n```\n\nUse `a < b` and **safe <text>**.";
        let html = render(markdown);
        assert!(html.contains("<h1>Wiki Δ</h1>"));
        assert!(html.contains("href=\"/wiki/linux-basics\""));
        assert!(html.contains("href=\"https://docs.example.invalid/\""));
        assert!(html.contains("&lt;script&gt;raw&lt;/script&gt;"));
        assert!(!html.contains("<script>"));
        assert!(!html.contains("<img "));
        assert!(!html.contains("href=\"javascript:"));
        assert!(!html.contains("href=\"../../"));
        assert!(html.contains("<code>a &lt; b</code>"));
        assert!(html.contains("<strong>safe &lt;text&gt;</strong>"));
    }
}
