use anyhow::{bail, Context, Result};
use rand::Rng;
use serde::Deserialize;
use std::time::Duration;

const ENDPOINT: &str = "https://fanyi-api.baidu.com/api/trans/vip/translate";

/// Baidu is a mainland-China service: it must connect direct. Routing it
/// through the user's GitHub proxy (an overseas node) breaks the TLS
/// handshake, so this module deliberately ignores `proxy_url`.
fn build_direct_client(timeout_secs: u64) -> Result<reqwest::blocking::Client> {
    reqwest::blocking::Client::builder()
        .user_agent("skills-manager")
        .timeout(std::time::Duration::from_secs(timeout_secs))
        .build()
        .context("Failed to build HTTP client")
}
/// Baidu caps a single query at 6000 UTF-8 bytes; stay well under so the
/// appid/salt/sign form overhead never pushes a request over the line.
const MAX_QUERY_BYTES: usize = 4000;
/// The free standard tier allows 1 QPS; pacing between requests avoids the
/// 54003 (rate limited) rejection on long documents.
const REQUEST_PACING: Duration = Duration::from_millis(1100);

#[derive(Debug, Deserialize)]
struct BaiduTranslateResponse {
    #[serde(default)]
    trans_result: Vec<BaiduTransResult>,
    error_code: Option<String>,
    error_msg: Option<String>,
}

#[derive(Debug, Deserialize)]
struct BaiduTransResult {
    #[serde(default)]
    dst: String,
}

/// One translatable unit: a piece of text plus the separator that must follow
/// it in the reassembled document (`""` for hard-split pieces of one long
/// line, `"\n"` for whole lines).
struct Unit {
    text: String,
    joiner: &'static str,
}

enum Segment {
    /// Fenced code blocks and their markers: passed through untouched so code
    /// survives translation byte-for-byte.
    Verbatim(String),
    Translatable(String),
}

/// Translate `text` into `to` via Baidu's general translation API. The result
/// preserves the input's line structure, so markdown still renders.
pub fn translate_text(
    text: &str,
    from: &str,
    to: &str,
    app_id: &str,
    api_key: &str,
) -> Result<String> {
    if text.trim().is_empty() {
        return Ok(text.to_string());
    }

    let client = build_direct_client(30)?;
    let mut out = String::with_capacity(text.len() + text.len() / 4);
    let mut first_request = true;

    for segment in split_code_segments(text) {
        match segment {
            Segment::Verbatim(part) => out.push_str(&part),
            Segment::Translatable(part) => {
                for chunk in translatable_chunks(&part, MAX_QUERY_BYTES) {
                    if !first_request {
                        std::thread::sleep(REQUEST_PACING);
                    }
                    first_request = false;
                    out.push_str(&translate_chunk(&client, &chunk, from, to, app_id, api_key)?);
                }
            }
        }
    }

    Ok(out)
}

/// Split the document at fenced code blocks: everything inside (and the fence
/// markers themselves) is verbatim, the rest is translatable.
fn split_code_segments(text: &str) -> Vec<Segment> {
    fn flush(buf: &mut String, kind: Option<bool>, segments: &mut Vec<Segment>) {
        if let Some(verbatim) = kind {
            if !buf.is_empty() {
                if verbatim {
                    segments.push(Segment::Verbatim(std::mem::take(buf)));
                } else {
                    segments.push(Segment::Translatable(std::mem::take(buf)));
                }
            }
        }
    }

    let mut segments = Vec::new();
    let mut buf = String::new();
    let mut buf_verbatim: Option<bool> = None;
    let mut in_fence = false;

    for line in text.split_inclusive('\n') {
        let is_marker = line.trim_start().starts_with("```");
        let verbatim = in_fence || is_marker;
        if is_marker {
            in_fence = !in_fence;
        }
        if buf_verbatim != Some(verbatim) {
            flush(&mut buf, buf_verbatim, &mut segments);
            buf_verbatim = Some(verbatim);
        }
        buf.push_str(line);
    }
    flush(&mut buf, buf_verbatim, &mut segments);
    segments
}

/// Turn a translatable segment into request-sized groups of units. Whole
/// lines become one unit each; a single line longer than `max` is hard-split
/// into several units that are re-joined without a newline.
fn translatable_chunks(part: &str, max: usize) -> Vec<Vec<Unit>> {
    let mut units: Vec<Unit> = Vec::new();
    for line in part.split_inclusive('\n') {
        let (content, joiner) = match line.strip_suffix('\n') {
            Some(stripped) => (stripped, "\n"),
            None => (line, ""),
        };
        if content.len() > max {
            // Hard-split an overlong line at char boundaries; the pieces are
            // glued back together without a newline on reassembly.
            let piece_len = (max / 3).max(1);
            let mut start = 0;
            let mut pieces: Vec<&str> = Vec::new();
            while start < content.len() {
                let mut end = (start + piece_len).min(content.len());
                while !content.is_char_boundary(end) {
                    end -= 1;
                }
                pieces.push(&content[start..end]);
                start = end;
            }
            for (index, piece) in pieces.iter().enumerate() {
                let piece_joiner = if index + 1 == pieces.len() { joiner } else { "" };
                units.push(Unit {
                    text: piece.to_string(),
                    joiner: piece_joiner,
                });
            }
        } else {
            units.push(Unit {
                text: content.to_string(),
                joiner,
            });
        }
    }

    let mut chunks: Vec<Vec<Unit>> = Vec::new();
    let mut current: Vec<Unit> = Vec::new();
    let mut current_bytes = 0usize;
    for unit in units {
        let unit_bytes = unit.text.len();
        if !current.is_empty() && current_bytes + 1 + unit_bytes > max {
            chunks.push(std::mem::take(&mut current));
            current_bytes = 0;
        }
        if !current.is_empty() {
            current_bytes += 1; // the newline that separates request lines
        }
        current_bytes += unit_bytes;
        current.push(unit);
    }
    if !current.is_empty() {
        chunks.push(current);
    }
    chunks
}

fn translate_chunk(
    client: &reqwest::blocking::Client,
    chunk: &[Unit],
    from: &str,
    to: &str,
    app_id: &str,
    api_key: &str,
) -> Result<String> {
    let mut out = String::new();
    // Empty lines never enter the request; their joiners pass straight through.
    let translatable: Vec<&Unit> = chunk.iter().filter(|unit| !unit.text.is_empty()).collect();
    if translatable.is_empty() {
        for unit in chunk {
            out.push_str(unit.joiner);
        }
        return Ok(out);
    }

    let q = translatable
        .iter()
        .map(|unit| unit.text.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    // Baidu splits a multi-line query into one trans_result entry per line,
    // so the response lines zip back onto the request lines one to one.
    let mut attempt = 0;
    let response = loop {
        attempt += 1;
        let salt: u64 = rand::thread_rng().gen_range(1_000_000_000u64..9_999_999_999u64);
        let sign = format!("{:x}", md5::compute(format!("{app_id}{q}{salt}{api_key}")));
        let salt_str = salt.to_string();
        let form = [
            ("q", q.as_str()),
            ("from", from),
            ("to", to),
            ("appid", app_id),
            ("salt", salt_str.as_str()),
            ("sign", sign.as_str()),
        ];
        let response: BaiduTranslateResponse = client
            .post(ENDPOINT)
            .form(&form)
            .send()
            .context("Failed to reach Baidu Translate")?
            .json()
            .context("Failed to parse Baidu Translate response")?;
        let error_code = response.error_code.clone();
        match error_code.as_deref() {
            // 54003: rate limited. One slow retry usually clears it.
            Some("54003") if attempt < 3 => {
                std::thread::sleep(Duration::from_millis(1500));
            }
            Some(code) => {
                bail!(
                    "Baidu Translate error {code}: {}",
                    response.error_msg.unwrap_or_default()
                )
            }
            None => break response,
        }
    };

    let mut dsts = response.trans_result.into_iter().map(|r| r.dst);
    for unit in chunk {
        if unit.text.is_empty() {
            out.push_str(unit.joiner);
        } else {
            out.push_str(&dsts.next().unwrap_or_default());
            out.push_str(unit.joiner);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{split_code_segments, translatable_chunks, Segment};

    #[test]
    fn code_fences_pass_through_verbatim() {
        let doc = "Intro line.\n```bash\nnpm install foo\n```\nOutro.\n";
        let segments = split_code_segments(doc);
        assert_eq!(segments.len(), 3);
        let Segment::Verbatim(code) = &segments[1] else {
            panic!("expected verbatim segment");
        };
        assert_eq!(code, "```bash\nnpm install foo\n```\n");
    }

    #[test]
    fn chunking_preserves_line_boundaries() {
        let part = "alpha\n\nbeta\ngamma\n";
        let chunks = translatable_chunks(part, 4000);
        assert_eq!(chunks.len(), 1);
        let rebuilt: String = chunks[0]
            .iter()
            .map(|unit| format!("{}{}", unit.text, unit.joiner))
            .collect();
        assert_eq!(rebuilt, part);
    }

    #[test]
    fn overlong_lines_split_and_rejoin() {
        let long_line = format!("x{}\n", "a".repeat(9000));
        let chunks = translatable_chunks(&long_line, 4000);
        let rebuilt: String = chunks
            .iter()
            .flatten()
            .map(|unit| format!("{}{}", unit.text, unit.joiner))
            .collect();
        assert_eq!(rebuilt, long_line);
    }
}
