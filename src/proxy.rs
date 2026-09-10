//! Transparent MITM HTTPS proxy — intercepts all LLM API traffic for
//! compression, caching, and token monitoring.
//!
//! Setup (one-time):  prism init --global
//! Start proxy:       prism serve --port 27181
//! All apps using HTTP_PROXY / HTTPS_PROXY will route through automatically.

use anyhow::{anyhow, Result};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tracing::{error, info, warn};

// ── Known AI providers ────────────────────────────────────────────────────────

const AI_HOSTS: &[(&str, &str)] = &[
    ("api.openai.com",                      "openai"),
    ("api.anthropic.com",                   "anthropic"),
    ("generativelanguage.googleapis.com",   "gemini"),
    ("api.mistral.ai",                      "mistral"),
    ("api.cohere.com",                      "cohere"),
    ("api.deepseek.com",                    "deepseek"),
    ("api.groq.com",                        "groq"),
    ("openrouter.ai",                       "openrouter"),
    ("api.perplexity.ai",                   "perplexity"),
    ("api.together.xyz",                    "together"),
    ("api.fireworks.ai",                    "fireworks"),
    ("api.x.ai",                            "xai"),
    ("localhost",                           "ollama"),
    ("127.0.0.1",                           "ollama"),
    ("api.cerebras.ai",                     "cerebras"),
];

/// Loopback ports that host a local model server. Everything else on localhost —
/// dev servers, language servers, an IDE's own helper processes — must tunnel
/// untouched: intercepting them breaks apps that never spoke to a model at all.
/// Override with `PRISM_LOCAL_AI_PORTS=11434,8080`.
fn local_ai_ports() -> Vec<u16> {
    match std::env::var("PRISM_LOCAL_AI_PORTS") {
        Ok(v) => v.split(',').filter_map(|p| p.trim().parse().ok()).collect(),
        Err(_) => vec![11434, 1234], // ollama, lm-studio
    }
}

fn detect_provider(host: &str) -> Option<&'static str> {
    let host_lower = host.to_lowercase();
    let bare = host_lower.split(':').next().unwrap_or(&host_lower);
    let port: Option<u16> = host_lower.split(':').nth(1).and_then(|p| p.parse().ok());
    if is_loopback(bare) {
        // only a known local-model port, and only when the port is explicit
        return match port {
            Some(p) if local_ai_ports().contains(&p) => Some("ollama"),
            _ => None,
        };
    }
    AI_HOSTS.iter().find(|(h, _)| bare == *h).map(|(_, p)| *p)
}

// ── CA certificate management ─────────────────────────────────────────────────

pub struct CaBundle {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

pub fn ca_dir() -> PathBuf {
    crate::prism_data_dir().join("ca")
}

/// Generate or load the PRISM CA certificate (rcgen 0.12 API).
pub fn ensure_ca() -> Result<CaBundle> {
    use rcgen::{Certificate, CertificateParams, DistinguishedName, DnType};

    let dir = ca_dir();
    std::fs::create_dir_all(&dir)?;

    let cert_path = dir.join("ca.crt");
    let key_path  = dir.join("ca.key");

    if cert_path.exists() && key_path.exists() {
        return Ok(CaBundle {
            cert_pem: std::fs::read(&cert_path)?,
            key_pem:  std::fs::read(&key_path)?,
        });
    }

    info!("Generating PRISM CA certificate...");
    let mut params = CertificateParams::default();
    params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);

    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, "PRISM Token Optimizer CA");
    dn.push(DnType::OrganizationName, "PRISM");
    params.distinguished_name = dn;

    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after  = rcgen::date_time_ymd(2034, 1, 1);
    params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];

    let cert = Certificate::from_params(params)?;
    let cert_pem = cert.serialize_pem()?.into_bytes();
    let key_pem  = cert.serialize_private_key_pem().into_bytes();

    std::fs::write(&cert_path, &cert_pem)?;
    std::fs::write(&key_path,  &key_pem)?;
    info!("CA cert: {}", cert_path.display());

    Ok(CaBundle { cert_pem, key_pem })
}

/// Generate per-domain cert signed by PRISM CA (rcgen 0.12 API).
/// Reconstructs CA Certificate from stored private key + fixed params (same DN → correct issuer chain).
fn make_domain_cert(hostname: &str, _ca_cert_pem: &[u8], ca_key_pem: &[u8]) -> Result<(Vec<u8>, Vec<u8>)> {
    use rcgen::{Certificate, CertificateParams, DistinguishedName, DnType, KeyPair, SanType};

    // Reconstruct CA Certificate from stored key so we can sign domain certs
    let ca_key_pair = KeyPair::from_pem(std::str::from_utf8(ca_key_pem)?)?;
    let mut ca_params = CertificateParams::default();
    ca_params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
    let mut ca_dn = DistinguishedName::new();
    ca_dn.push(DnType::CommonName, "PRISM Token Optimizer CA");
    ca_dn.push(DnType::OrganizationName, "PRISM");
    ca_params.distinguished_name = ca_dn;
    ca_params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    ca_params.not_after  = rcgen::date_time_ymd(2034, 1, 1);
    ca_params.key_usages = vec![
        rcgen::KeyUsagePurpose::KeyCertSign,
        rcgen::KeyUsagePurpose::CrlSign,
    ];
    ca_params.key_pair = Some(ca_key_pair);
    let ca_cert = Certificate::from_params(ca_params)?;

    // Create domain cert signed by CA
    let mut params = CertificateParams::default();
    let mut dn = DistinguishedName::new();
    dn.push(DnType::CommonName, hostname);
    params.distinguished_name = dn;
    params.subject_alt_names = vec![SanType::DnsName(hostname.to_string())];
    params.not_before = rcgen::date_time_ymd(2024, 1, 1);
    params.not_after  = rcgen::date_time_ymd(2027, 1, 1);

    let domain_cert = Certificate::from_params(params)?;
    let cert_pem = domain_cert.serialize_pem_with_signer(&ca_cert)?.into_bytes();
    let key_pem  = domain_cert.serialize_private_key_pem().into_bytes();

    Ok((cert_pem, key_pem))
}

// ── HTTP framing helpers ──────────────────────────────────────────────────────

fn find_sub(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Case-insensitive lookup of a single HTTP header value.
fn header_value(headers: &[u8], name: &str) -> Option<String> {
    let s = std::str::from_utf8(headers).ok()?;
    let target = name.to_lowercase();
    // Skip the request/status line.
    for line in s.split("\r\n").skip(1) {
        let (k, v) = line.split_once(':')?;
        if k.trim().to_lowercase() == target {
            return Some(v.trim().to_string());
        }
    }
    None
}

fn is_chunked(headers: &[u8]) -> bool {
    header_value(headers, "transfer-encoding")
        .map(|v| v.to_lowercase().contains("chunked"))
        .unwrap_or(false)
}

fn content_length(headers: &[u8]) -> Option<usize> {
    header_value(headers, "content-length")?.trim().parse().ok()
}

fn wants_close(headers: &[u8]) -> bool {
    header_value(headers, "connection")
        .map(|v| v.to_lowercase().contains("close"))
        .unwrap_or(false)
}

const MAX_HEADER_BYTES: usize = 256 * 1024;

/// Read HTTP headers until the terminating CRLFCRLF, however many reads it takes.
/// `carry` holds bytes left over from a previous message on the same connection.
/// Returns (header_bytes_including_terminator, leftover_bytes_after_headers).
async fn read_headers<S>(stream: &mut S, carry: Vec<u8>) -> Option<(Vec<u8>, Vec<u8>)>
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = carry;
    let mut tmp = vec![0u8; 16384];
    loop {
        if let Some(pos) = find_sub(&buf, b"\r\n\r\n") {
            let rest = buf.split_off(pos + 4);
            return Some((buf, rest));
        }
        if buf.len() > MAX_HEADER_BYTES {
            return None;
        }
        match stream.read(&mut tmp).await {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&tmp[..n]),
        }
    }
}

/// Read exactly one message body, honouring Content-Length or chunked encoding.
/// Returns (decoded_body, leftover_bytes_belonging_to_the_next_message).
async fn read_body<S>(stream: &mut S, headers: &[u8], have: Vec<u8>) -> (Vec<u8>, Vec<u8>)
where
    S: tokio::io::AsyncRead + Unpin,
{
    if is_chunked(headers) {
        return read_chunked(stream, have).await;
    }
    match content_length(headers) {
        Some(0) | None => (Vec::new(), have),
        Some(len) => {
            let mut body = have;
            let mut tmp = vec![0u8; 16384];
            while body.len() < len {
                match stream.read(&mut tmp).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => body.extend_from_slice(&tmp[..n]),
                }
            }
            if body.len() > len {
                let leftover = body.split_off(len);
                (body, leftover)
            } else {
                (body, Vec::new())
            }
        }
    }
}

/// Decode a chunked-transfer body into plain bytes.
async fn read_chunked<S>(stream: &mut S, have: Vec<u8>) -> (Vec<u8>, Vec<u8>)
where
    S: tokio::io::AsyncRead + Unpin,
{
    let mut buf = have;
    let mut out = Vec::new();
    let mut pos = 0usize;
    let mut tmp = vec![0u8; 16384];

    loop {
        // Chunk-size line.
        let line_end = loop {
            if let Some(p) = find_sub(&buf[pos..], b"\r\n") {
                break pos + p;
            }
            match stream.read(&mut tmp).await {
                Ok(0) | Err(_) => return (out, Vec::new()),
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        };
        let size = std::str::from_utf8(&buf[pos..line_end])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).ok())
            .unwrap_or(0);
        pos = line_end + 2;

        if size == 0 {
            // Consume the trailer terminator if present, then stop.
            while find_sub(&buf[pos.saturating_sub(2)..], b"\r\n").is_none() {
                match stream.read(&mut tmp).await {
                    Ok(0) | Err(_) => return (out, Vec::new()),
                    Ok(n) => buf.extend_from_slice(&tmp[..n]),
                }
            }
            let leftover = if pos + 2 <= buf.len() { buf[pos + 2..].to_vec() } else { Vec::new() };
            return (out, leftover);
        }

        while buf.len() < pos + size + 2 {
            match stream.read(&mut tmp).await {
                Ok(0) | Err(_) => return (out, Vec::new()),
                Ok(n) => buf.extend_from_slice(&tmp[..n]),
            }
        }
        out.extend_from_slice(&buf[pos..pos + size]);
        pos += size + 2;
    }
}

/// Rebuild a request with the (possibly compressed) body.
/// Drops Transfer-Encoding (we already de-chunked) and forces identity encoding
/// so the response body stays parseable for token accounting.
fn rebuild_request(headers: &[u8], body: &[u8], extra: &[(String, String)]) -> Vec<u8> {
    let s = String::from_utf8_lossy(headers);
    let mut out = String::with_capacity(headers.len() + 128);

    for (i, line) in s.split("\r\n").enumerate() {
        if line.is_empty() {
            continue;
        }
        if i > 0 {
            let lower = line.to_lowercase();
            if lower.starts_with("content-length:")
                || lower.starts_with("transfer-encoding:")
                || lower.starts_with("accept-encoding:")
            {
                continue;
            }
            // Headers we are replacing are dropped here and re-emitted below.
            if extra
                .iter()
                .any(|(k, _)| lower.starts_with(&format!("{}:", k.to_lowercase())))
            {
                continue;
            }
        }
        out.push_str(line);
        out.push_str("\r\n");
    }

    for (k, v) in extra {
        out.push_str(&format!("{}: {}\r\n", k, v));
    }

    out.push_str("Accept-Encoding: identity\r\n");
    if !body.is_empty() {
        out.push_str(&format!("Content-Length: {}\r\n", body.len()));
    }
    out.push_str("\r\n");

    let mut req = out.into_bytes();
    req.extend_from_slice(body);
    req
}

// ── Response relay (streaming-safe) ───────────────────────────────────────────

/// Pull `usage` token counts out of either a plain JSON body or an SSE stream.
fn extract_resp_tokens(body: &[u8]) -> u32 {
    fn usage_of(j: &serde_json::Value) -> Option<u32> {
        j.pointer("/usage/completion_tokens")
            .or_else(|| j.pointer("/usage/output_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }

    if let Ok(j) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(t) = usage_of(&j) {
            return t;
        }
    }

    // Streaming: the final events carry cumulative usage.
    let s = String::from_utf8_lossy(body);
    let mut best = 0u32;
    for line in s.lines() {
        let payload = line.strip_prefix("data:").unwrap_or(line).trim();
        if !payload.starts_with('{') {
            continue;
        }
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(payload) {
            if let Some(t) = usage_of(&j) {
                best = best.max(t);
            }
        }
    }
    best
}

fn extract_cached_tokens(body: &[u8]) -> u32 {
    fn cached_of(j: &serde_json::Value) -> Option<u32> {
        j.pointer("/usage/prompt_tokens_details/cached_tokens")
            .or_else(|| j.pointer("/usage/cache_read_input_tokens"))
            .and_then(|v| v.as_u64())
            .map(|v| v as u32)
    }

    if let Ok(j) = serde_json::from_slice::<serde_json::Value>(body) {
        if let Some(t) = cached_of(&j) {
            return t;
        }
    }

    let s = String::from_utf8_lossy(body);
    let mut best = 0u32;
    for line in s.lines() {
        let payload = line.strip_prefix("data:").unwrap_or(line).trim();
        if !payload.starts_with('{') {
            continue;
        }
        if let Ok(j) = serde_json::from_str::<serde_json::Value>(payload) {
            if let Some(t) = cached_of(&j) {
                best = best.max(t);
            }
        }
    }
    best
}

const MAX_SNIFF_BYTES: usize = 512 * 1024;

/// Copy the upstream response to the client as it arrives, flushing every chunk
/// so SSE reaches the client token-by-token. Returns (response_tokens, cached_tokens, upstream_wants_close).
/// What the proxy learned from relaying one response.
struct Relayed {
    resp_tokens: u32,
    cached_tokens: u32,
    close: bool,
    status: u16,
    content_type: String,
    /// The response body, dechunked. Only trustworthy when `complete` is set.
    body: Vec<u8>,
    /// False when the body outran `MAX_SNIFF_BYTES`. A partial body must never be
    /// cached: replaying it later would hand the client a truncated answer.
    complete: bool,
}

fn status_of(headers: &[u8]) -> u16 {
    let line = headers.split(|b| *b == b'\n').next().unwrap_or(&[]);
    std::str::from_utf8(line)
        .ok()
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|c| c.parse().ok())
        .unwrap_or(0)
}

async fn relay_response<U, C>(upstream: &mut U, client: &mut C) -> Option<Relayed>
where
    U: tokio::io::AsyncRead + Unpin,
    C: tokio::io::AsyncWrite + Unpin,
{
    let (headers, first_body) = read_headers(upstream, Vec::new()).await?;

    client.write_all(&headers).await.ok()?;
    client.flush().await.ok()?;

    let chunked = is_chunked(&headers);
    let clen = content_length(&headers);
    let close = wants_close(&headers);

    let mut sniff: Vec<u8> = Vec::new();
    let mut tail: Vec<u8> = Vec::new();
    let mut seen = 0usize;
    let mut overran = false;

    let push = |bytes: &[u8], sniff: &mut Vec<u8>, tail: &mut Vec<u8>, overran: &mut bool| {
        if sniff.len() < MAX_SNIFF_BYTES {
            sniff.extend_from_slice(bytes);
        } else {
            *overran = true;
        }
        tail.extend_from_slice(bytes);
        if tail.len() > 32 {
            let cut = tail.len() - 32;
            tail.drain(..cut);
        }
    };

    if !first_body.is_empty() {
        client.write_all(&first_body).await.ok()?;
        client.flush().await.ok()?;
        seen += first_body.len();
        push(&first_body, &mut sniff, &mut tail, &mut overran);
    }

    let terminated = |chunked: bool, tail: &[u8], seen: usize| -> bool {
        if chunked {
            find_sub(tail, b"0\r\n\r\n").is_some()
        } else if let Some(l) = clen {
            seen >= l
        } else {
            false
        }
    };

    let mut tmp = vec![0u8; 16384];
    while !terminated(chunked, &tail, seen) {
        match upstream.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                client.write_all(&tmp[..n]).await.ok()?;
                client.flush().await.ok()?;
                seen += n;
                push(&tmp[..n], &mut sniff, &mut tail, &mut overran);
            }
        }
    }

    // Chunked bodies still carry their framing; decode before parsing usage.
    let parseable = if chunked { dechunk_bytes(&sniff) } else { sniff };
    Some(Relayed {
        resp_tokens: extract_resp_tokens(&parseable),
        cached_tokens: extract_cached_tokens(&parseable),
        close,
        status: status_of(&headers),
        content_type: header_value(&headers, "content-type").unwrap_or_default(),
        complete: !overran && terminated(chunked, &tail, seen),
        body: parseable,
    })
}

/// Strip chunk framing from an already-buffered chunked body.
fn dechunk_bytes(buf: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut pos = 0usize;
    while pos < buf.len() {
        let Some(rel) = find_sub(&buf[pos..], b"\r\n") else { break };
        let line_end = pos + rel;
        let Some(size) = std::str::from_utf8(&buf[pos..line_end])
            .ok()
            .and_then(|s| usize::from_str_radix(s.split(';').next().unwrap_or("").trim(), 16).ok())
        else {
            break;
        };
        pos = line_end + 2;
        if size == 0 || pos + size > buf.len() {
            break;
        }
        out.extend_from_slice(&buf[pos..pos + size]);
        pos += size + 2;
    }
    out
}

// ── Request compression ───────────────────────────────────────────────────────

/// Below this, compression is not worth the semantic risk.
const MIN_COMPRESS_TOKENS: u32 = 500;
/// How many trailing messages are eligible for compression. Everything before
/// them is the provider's cacheable prefix and must stay byte-stable.
const COMPRESSIBLE_TAIL: usize = 2;
/// Anthropic allows at most 4 cache breakpoints.
const MAX_CACHE_BREAKPOINTS: usize = 4;

/// Compress an LLM API request body.
///
/// Compression is deliberately confined to the tail of the conversation. The
/// system prompt and all earlier messages form the prefix that OpenAI (implicit,
/// >=1024 tokens) and Anthropic (`cache_control`) cache; a cache read costs 10%
/// of input on Anthropic, so rewriting that prefix to shave 25% off it is a net
/// loss. We leave the prefix untouched and add cache breakpoints instead.
///
/// Returns (modified_body, orig_tokens, sent_tokens, model, is_stream).
fn compress_request_body(
    body: &[u8],
    provider: &str,
    ratio: f64,
) -> (Vec<u8>, u32, u32, String, bool) {
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (body.to_vec(), 0, 0, String::new(), false);
    };

    let model = json
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let is_stream = json
        .get("stream")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    let mut orig_total = 0u32;
    let mut sent_total = 0u32;

    if let Some(messages) = json.get_mut("messages").and_then(|v| v.as_array_mut()) {
        let total = messages.len();
        let first_compressible = total.saturating_sub(COMPRESSIBLE_TAIL);

        for (idx, msg) in messages.iter_mut().enumerate() {
            // Prefix messages are counted but never rewritten.
            let may_compress = idx >= first_compressible;
            let Some(content) = msg.get_mut("content") else { continue };

            if let Some(text) = content.as_str().map(|s| s.to_string()) {
                let (out, ot, st) = maybe_compress(&text, &model, ratio, may_compress);
                orig_total += ot;
                sent_total += st;
                if let Some(new_text) = out {
                    *content = serde_json::Value::String(new_text);
                }
            } else if let Some(blocks) = content.as_array_mut() {
                for block in blocks.iter_mut() {
                    if block.get("type").and_then(|t| t.as_str()) != Some("text") {
                        continue;
                    }
                    let Some(text) = block.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    else {
                        continue;
                    };
                    let (out, ot, st) = maybe_compress(&text, &model, ratio, may_compress);
                    orig_total += ot;
                    sent_total += st;
                    if let Some(new_text) = out {
                        if let Some(t) = block.get_mut("text") {
                            *t = serde_json::Value::String(new_text);
                        }
                    }
                }
            }
        }
    }

    // Count system prompt tokens for telemetry / analytics
    if let Some(system) = json.get("system") {
        if let Some(text) = system.as_str() {
            let t = crate::analytics::count_tokens(text, &model).unwrap_or(0) as u32;
            orig_total += t;
            sent_total += t;
        } else if let Some(blocks) = system.as_array() {
            for block in blocks {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    let t = crate::analytics::count_tokens(text, &model).unwrap_or(0) as u32;
                    orig_total += t;
                    sent_total += t;
                }
            }
        }
    }

    if provider == "anthropic" {
        let existing = collect_cache_control_pointers(&json);
        let auto_cache = std::env::var("PRISM_NO_CACHE_CONTROL").is_err();

        if !existing.is_empty() {
            info!(
                "cache_control: caller sent {} breakpoint(s), skipping PRISM auto-injection",
                existing.len()
            );
        }

        // Only inject PRISM automatic breakpoints if the caller did not configure
        // any of their own. If the caller (e.g. Claude Code, Cursor, custom client)
        // already manages prompt caching, their strategy wins.
        if auto_cache && existing.is_empty() {
            apply_anthropic_caching(&mut json, &model, &mut orig_total, &mut sent_total);
        }

        // Always normalize cache_control across tools, system, and messages to guarantee:
        // 1. At most MAX_CACHE_BREAKPOINTS (4) total breakpoints.
        // 2. TTL ordering compliance: no ttl='1h' comes after a ttl='5m' (or default ephemeral).
        normalize_anthropic_caching(&mut json);
    }

    // Images last: they are priced separately from text and must not disturb the
    // cache_control pointers computed above (rightsizing rewrites base64 in place and
    // never adds or removes a content block).
    rightsize_images(&mut json, provider, &mut orig_total, &mut sent_total);

    let modified = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    (modified, orig_total, sent_total, model, is_stream)
}

/// Longest image edge to allow through, and whether we may go below the provider's own
/// cap. `PRISM_IMAGE_MAX_EDGE=0` disables image handling entirely.
fn image_edge() -> Option<u32> {
    let var = |k: &str| std::env::var(k).ok().and_then(|v| v.trim().parse::<u32>().ok());
    // A target below the provider cap is the only setting that actually saves tokens,
    // so it is opt-in; the default merely strips pixels the provider would discard.
    if let Some(t) = var("PRISM_IMAGE_TARGET_EDGE") {
        return if t == 0 { None } else { Some(t) };
    }
    match var("PRISM_IMAGE_MAX_EDGE") {
        Some(0) => None,
        Some(e) => Some(e),
        None => Some(crate::image::DEFAULT_MAX_EDGE),
    }
}

/// Rightsize every image in the request and fold the result into the token counters.
fn rightsize_images(
    json: &mut serde_json::Value,
    provider: &str,
    orig_total: &mut u32,
    sent_total: &mut u32,
) {
    let Some(edge) = image_edge() else { return };
    let reports = crate::image::rightsize_value(json, edge, provider);
    if reports.is_empty() {
        return;
    }
    for r in &reports {
        *orig_total = orig_total.saturating_add(r.tokens_before);
        *sent_total = sent_total.saturating_add(r.tokens_after);
        if r.rewritten {
            info!(
                "image rightsized: {}x{} -> {}x{} ({}), {} -> {} tok, {} -> {} bytes",
                r.from.0, r.from.1, r.to.0, r.to.1,
                r.format.media_type(),
                r.tokens_before, r.tokens_after,
                r.bytes_before, r.bytes_after
            );
        } else {
            info!(
                "image kept: {}x{} ({}), {} tok — {}",
                r.from.0, r.from.1,
                r.format.media_type(),
                r.tokens_before,
                if r.format.resizable() { "already within limits" } else { "format not re-encodable" }
            );
        }
    }
}

/// Returns (replacement_text_if_compressed, orig_tokens, sent_tokens).
fn maybe_compress(
    text: &str,
    model: &str,
    ratio: f64,
    may_compress: bool,
) -> (Option<String>, u32, u32) {
    let orig = crate::analytics::count_tokens(text, model).unwrap_or(0) as u32;
    if !may_compress || orig <= MIN_COMPRESS_TOKENS {
        return (None, orig, orig);
    }
    let c = crate::compress::compress(text, ratio);
    // Never let "compression" grow the payload.
    if c.compressed_tokens as u32 >= orig {
        return (None, orig, orig);
    }
    (Some(c.compressed), orig, c.compressed_tokens as u32)
}

/// Add `cache_control` breakpoints so repeated context bills at 10% of input.
///
/// Anthropic caches everything from the start of the prompt up to and including
/// each breakpoint, and allows at most 4. We anchor one on the system prompt
/// (the most stable block) and spread the rest across earlier conversation
/// turns, so long sessions keep a live breakpoint inside the lookback window
/// instead of ageing out and re-paying full price every turn.
fn apply_anthropic_caching(
    json: &mut serde_json::Value,
    _model: &str,
    _orig_total: &mut u32,
    _sent_total: &mut u32,
) {
    let mut breakpoints = 0usize;

    // 1. System prompt — always the most reusable block.
    if let Some(system) = json.get_mut("system") {
        if let Some(text) = system.as_str().map(|s| s.to_string()) {
            *system = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }]);
            breakpoints += 1;
        } else if let Some(blocks) = system.as_array_mut() {
            if let Some(last) = blocks.last_mut() {
                if let Some(obj) = last.as_object_mut() {
                    obj.insert(
                        "cache_control".into(),
                        serde_json::json!({"type": "ephemeral"}),
                    );
                    breakpoints += 1;
                }
            }
        }
    }

    // 2. Spread remaining breakpoints over the stable part of the conversation.
    let Some(messages) = json.get_mut("messages").and_then(|v| v.as_array_mut()) else {
        return;
    };
    // Only the prefix is stable; the tail changes every turn and would thrash.
    let stable = messages.len().saturating_sub(COMPRESSIBLE_TAIL);
    if stable == 0 {
        return;
    }
    let remaining = MAX_CACHE_BREAKPOINTS.saturating_sub(breakpoints);
    if remaining == 0 {
        return;
    }

    // Anchor at evenly spaced points, biased toward the most recent stable turn
    // so the newest breakpoint stays inside the cache lookback window.
    let step = (stable / remaining).max(1);
    let mut anchors: Vec<usize> = (0..remaining)
        .map(|i| stable.saturating_sub(1 + i * step))
        .collect();
    anchors.dedup();

    for idx in anchors {
        let Some(msg) = messages.get_mut(idx) else { continue };
        let Some(content) = msg.get_mut("content") else { continue };

        // cache_control lives on a content block, so a bare string must be lifted.
        if let Some(text) = content.as_str().map(|s| s.to_string()) {
            *content = serde_json::json!([{
                "type": "text",
                "text": text,
                "cache_control": {"type": "ephemeral"}
            }]);
        } else if let Some(blocks) = content.as_array_mut() {
            if let Some(last) = blocks.last_mut() {
                if let Some(obj) = last.as_object_mut() {
                    obj.insert(
                        "cache_control".into(),
                        serde_json::json!({"type": "ephemeral"}),
                    );
                }
            }
        }
    }
}


/// Collects all JSON pointers to `cache_control` objects in the exact order
/// Anthropic processes them: `tools` -> `system` -> `messages`.
fn collect_cache_control_pointers(json: &serde_json::Value) -> Vec<String> {
    let mut pointers = Vec::new();

    // 1. tools
    if let Some(tools) = json.get("tools").and_then(|v| v.as_array()) {
        for (idx, tool) in tools.iter().enumerate() {
            if tool.get("cache_control").is_some() {
                pointers.push(format!("/tools/{}/cache_control", idx));
            }
        }
    }

    // 2. system
    if let Some(system) = json.get("system") {
        if let Some(blocks) = system.as_array() {
            for (idx, block) in blocks.iter().enumerate() {
                if block.get("cache_control").is_some() {
                    pointers.push(format!("/system/{}/cache_control", idx));
                }
            }
        } else if system.get("cache_control").is_some() {
            pointers.push("/system/cache_control".to_string());
        }
    }

    // 3. messages
    if let Some(messages) = json.get("messages").and_then(|v| v.as_array()) {
        for (m_idx, msg) in messages.iter().enumerate() {
            let base_ptr = format!("/messages/{}", m_idx);
            if let Some(content) = msg.get("content") {
                collect_content_cache_pointers(content, &format!("{}/content", base_ptr), &mut pointers);
            }
        }
    }

    pointers
}

fn collect_content_cache_pointers(
    value: &serde_json::Value,
    current_ptr: &str,
    pointers: &mut Vec<String>,
) {
    if let Some(blocks) = value.as_array() {
        for (idx, block) in blocks.iter().enumerate() {
            let block_ptr = format!("{}/{}", current_ptr, idx);
            if block.get("cache_control").is_some() {
                pointers.push(format!("{}/cache_control", block_ptr));
            }
            // Check nested content (e.g. inside tool_result)
            if let Some(nested) = block.get("content") {
                collect_content_cache_pointers(nested, &format!("{}/content", block_ptr), pointers);
            }
        }
    } else if let Some(obj) = value.as_object() {
        if obj.get("cache_control").is_some() {
            pointers.push(format!("{}/cache_control", current_ptr));
        }
        if let Some(nested) = obj.get("content") {
            collect_content_cache_pointers(nested, &format!("{}/content", current_ptr), pointers);
        }
    }
}

/// Enforces Anthropic's caching invariants on the request payload:
/// 1. At most MAX_CACHE_BREAKPOINTS (4) total breakpoints across tools, system, messages.
/// 2. TTL non-increasing order: Anthropic requires that a ttl='1h' cache_control block
///    must NOT come after a ttl='5m' (or default ephemeral) block in evaluation order:
///    tools -> system -> messages.
fn normalize_anthropic_caching(json: &mut serde_json::Value) {
    let mut pointers = collect_cache_control_pointers(json);
    if pointers.is_empty() {
        return;
    }

    // 1. Enforce max breakpoints (Anthropic allows at most 4 across the request).
    if pointers.len() > MAX_CACHE_BREAKPOINTS {
        let excess_count = pointers.len() - MAX_CACHE_BREAKPOINTS;
        warn!(
            "cache_control: {} breakpoints found, stripping {} excess (max {})",
            pointers.len(), excess_count, MAX_CACHE_BREAKPOINTS
        );
        for excess_ptr in &pointers[MAX_CACHE_BREAKPOINTS..] {
            if let Some(parent_ptr) = excess_ptr.strip_suffix("/cache_control") {
                if let Some(parent) = json.pointer_mut(parent_ptr).and_then(|v| v.as_object_mut()) {
                    parent.remove("cache_control");
                }
            }
        }
        pointers.truncate(MAX_CACHE_BREAKPOINTS);
    }

    // 2. Enforce TTL ordering.
    // In Anthropic API, if ttl is omitted, it defaults to 5 minutes ("5m").
    // If ANY breakpoint has ttl='1h', then all breakpoints preceding it must also have ttl='1h'.
    //
    // Build a TTL map for diagnostics.
    let ttl_map: Vec<(&str, Option<&str>)> = pointers.iter().map(|ptr| {
        let ttl = json.pointer(ptr)
            .and_then(|v| v.get("ttl"))
            .and_then(|v| v.as_str());
        (ptr.as_str(), ttl)
    }).collect();

    let last_1h_index = ttl_map.iter().rposition(|(_, ttl)| *ttl == Some("1h"));

    if let Some(last_1h) = last_1h_index {
        let mut promoted = 0usize;
        for (i, ptr) in pointers[..last_1h].iter().enumerate() {
            if let Some(cc) = json.pointer_mut(ptr).and_then(|v| v.as_object_mut()) {
                let current_ttl = cc.get("ttl").and_then(|v| v.as_str());
                if current_ttl != Some("1h") {
                    warn!(
                        "cache_control: promoting {} from ttl={} to ttl=1h (block {} of {}, 1h block at index {})",
                        ptr,
                        current_ttl.unwrap_or("5m (default)"),
                        i,
                        pointers.len(),
                        last_1h
                    );
                    cc.insert("ttl".into(), serde_json::json!("1h"));
                    promoted += 1;
                }
            }
        }
        if promoted > 0 {
            info!(
                "cache_control: promoted {} breakpoint(s) to ttl=1h to fix TTL ordering",
                promoted
            );
        }
    }
}

// ── Anthropic context editing ─────────────────────────────────────────────────

/// Beta header gating server-side context management.
const CONTEXT_MGMT_BETA: &str = "context-management-2025-06-27";
/// Start clearing stale tool results once the prompt passes this many tokens.
/// Anthropic's own documented default; deliberately conservative so ordinary
/// conversations are never touched.
const CONTEXT_EDIT_TRIGGER_TOKENS: u64 = 100_000;
/// How many of the most recent tool results to keep verbatim.
const CONTEXT_EDIT_KEEP_TOOL_USES: u64 = 3;
/// Minimum to clear per trigger.
///
/// Clearing rewrites the prompt prefix, which invalidates the cache from that
/// point. Without a floor, a long run can clear just enough to fall back under
/// the threshold on every single turn — paying a cache miss each time to save
/// almost nothing. Batching the clears keeps that cost amortised.
const CONTEXT_EDIT_CLEAR_AT_LEAST_TOKENS: u64 = 10_000;

/// True if any message carries a `tool_result` block.
fn has_tool_results(json: &serde_json::Value) -> bool {
    let Some(messages) = json.get("messages").and_then(|m| m.as_array()) else {
        return false;
    };
    messages.iter().any(|m| {
        m.get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .any(|b| b.get("type").and_then(|t| t.as_str()) == Some("tool_result"))
            })
            .unwrap_or(false)
    })
}

/// Ask Anthropic to drop stale tool results server-side.
///
/// Long agent runs accumulate tool output that is never read again but is
/// re-sent, and re-billed, on every turn. `clear_tool_uses_20250919` replaces
/// the oldest results with a placeholder once the prompt crosses a threshold,
/// keeping the most recent few intact. Unlike rewriting the prompt ourselves
/// this is designed to preserve the cache, so it composes with the breakpoints
/// placed in `apply_anthropic_caching`.
///
/// Returns the `anthropic-beta` header to send, or None if this request should
/// be left alone.
fn apply_context_editing(
    json: &mut serde_json::Value,
    headers: &[u8],
    enabled: bool,
) -> Option<(String, String)> {
    if !enabled {
        return None;
    }
    // The caller configured it themselves — theirs wins.
    if json.get("context_management").is_some() {
        return None;
    }
    // Nothing to clear; adding a beta header would be noise.
    if !has_tool_results(json) {
        return None;
    }

    json.as_object_mut()?.insert(
        "context_management".into(),
        serde_json::json!({
            "edits": [{
                "type": "clear_tool_uses_20250919",
                "trigger":        {"type": "input_tokens", "value": CONTEXT_EDIT_TRIGGER_TOKENS},
                "keep":           {"type": "tool_uses",    "value": CONTEXT_EDIT_KEEP_TOOL_USES},
                "clear_at_least": {"type": "input_tokens", "value": CONTEXT_EDIT_CLEAR_AT_LEAST_TOKENS},
            }]
        }),
    );

    // Merge rather than clobber — callers may already request other betas.
    let merged = match header_value(headers, "anthropic-beta") {
        Some(existing) if existing.contains(CONTEXT_MGMT_BETA) => existing,
        Some(existing) => format!("{},{}", existing, CONTEXT_MGMT_BETA),
        None => CONTEXT_MGMT_BETA.to_string(),
    };
    Some(("anthropic-beta".to_string(), merged))
}


// ── Response cache ────────────────────────────────────────────────────────────
//
// Recording and serving are deliberately separate switches, because they carry
// completely different risk. Recording never changes what the client receives — it only
// fills a store that was previously always empty, which is why `cache::find_similar`
// could never return anything no matter how good its scoring got. Serving *replaces* a
// live model call, and an LLM response is not a pure function of its request: the same
// question asked at two points in an agent run can have two different correct answers.
// So recording defaults on with a kill switch, and serving defaults off.

/// Largest response worth storing. A body past this is a long generation whose replay
/// value does not justify the sled write.
const MAX_CACHE_BODY: usize = 256 * 1024;

/// Recording writes prompts and responses to `~/.local/share/prism/cache/`. That is a
/// real privacy posture, so it has an off switch.
fn cache_record_enabled() -> bool {
    !matches!(
        std::env::var("PRISM_CACHE_RECORD").as_deref(),
        Ok("0") | Ok("false") | Ok("off") | Ok("no")
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ServeMode {
    /// Never replay. The default.
    Off,
    /// Replay only when the caller asked for deterministic sampling, so a cache hit
    /// returns what a fresh call would have returned anyway.
    Deterministic,
    /// Replay whatever matches. Saves the most and is the most likely to surprise: a
    /// caller at temperature 1 asked for variation and will stop getting it.
    Always,
}

fn parse_serve_mode(v: Option<&str>) -> ServeMode {
    match v.map(str::trim) {
        Some("1") | Some("deterministic") | Some("on") | Some("true") => ServeMode::Deterministic,
        Some("always") | Some("any") => ServeMode::Always,
        _ => ServeMode::Off,
    }
}

fn cache_serve_mode() -> ServeMode {
    parse_serve_mode(std::env::var("PRISM_CACHE_SERVE").ok().as_deref())
}

struct CacheKey {
    /// Covers everything that determines the answer.
    key: String,
    /// The part a human would recognise, for `prism cache query` and similarity.
    prompt: String,
    /// The caller pinned sampling, so a replay is not a behaviour change.
    deterministic: bool,
}

/// Remove `cache_control` everywhere and top-level `metadata`, so a request keys
/// identically before and after prism rewrites it and regardless of which end user sent
/// it. Anything else that differs between two requests genuinely may change the answer
/// and so belongs in the key.
fn strip_volatile(v: &mut serde_json::Value) {
    if let serde_json::Value::Object(map) = v {
        map.remove("metadata");
    }
    fn walk(v: &mut serde_json::Value) {
        match v {
            serde_json::Value::Object(map) => {
                map.remove("cache_control");
                for (_, val) in map.iter_mut() {
                    walk(val);
                }
            }
            serde_json::Value::Array(a) => a.iter_mut().for_each(walk),
            _ => {}
        }
    }
    walk(v);
}

/// The newest user-authored text in the request, for display and similarity search.
fn last_user_text(json: &serde_json::Value) -> String {
    fn text_of(content: &serde_json::Value) -> String {
        match content {
            serde_json::Value::String(s) => s.clone(),
            serde_json::Value::Array(blocks) => blocks
                .iter()
                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        }
    }
    // Anthropic / OpenAI
    if let Some(msgs) = json.get("messages").and_then(|m| m.as_array()) {
        for m in msgs.iter().rev() {
            if m.get("role").and_then(|r| r.as_str()) == Some("user") {
                let t = m.get("content").map(text_of).unwrap_or_default();
                if !t.trim().is_empty() {
                    return t;
                }
            }
        }
    }
    // Gemini
    if let Some(contents) = json.get("contents").and_then(|c| c.as_array()) {
        for c in contents.iter().rev() {
            let t = c
                .get("parts")
                .and_then(|p| p.as_array())
                .map(|parts| {
                    parts
                        .iter()
                        .filter_map(|p| p.get("text").and_then(|t| t.as_str()))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            if !t.trim().is_empty() {
                return t;
            }
        }
    }
    String::new()
}

/// Build the cache key for a request, or `None` when the request should not participate
/// in the cache at all.
fn cache_key(raw: &[u8], provider: &str) -> Option<CacheKey> {
    let mut json: serde_json::Value = serde_json::from_slice(raw).ok()?;
    // Asking for more than one completion is asking for variation; there is nothing to
    // replay that would be right.
    if json.get("n").and_then(|n| n.as_u64()).map(|n| n > 1).unwrap_or(false) {
        return None;
    }
    let temp = json.get("temperature").and_then(|t| t.as_f64());
    let top_p = json.get("top_p").and_then(|t| t.as_f64());
    // Absent temperature is *not* deterministic — every provider defaults it to 1.
    let deterministic = temp == Some(0.0) && top_p.map(|p| p == 1.0).unwrap_or(true);

    let prompt = last_user_text(&json);
    strip_volatile(&mut json);
    let canonical = serde_json::json!({ "p": provider, "r": json });
    Some(CacheKey {
        key: serde_json::to_string(&canonical).ok()?,
        prompt,
        deterministic,
    })
}

/// Whether a stored response may be handed back for a request wanting `want_stream`.
fn cache_replayable(entry: &crate::cache::CacheEntry, want_stream: bool) -> bool {
    // Framing is part of the contract: an SSE client cannot read a JSON body, and
    // replaying a stream to a non-streaming caller hands it event framing it will not
    // parse.
    if entry.is_stream != want_stream || entry.response.is_empty() {
        return false;
    }
    // A tool call names an id the agent echoes back and a side effect it will run.
    // Replaying one makes the agent re-execute a tool against a stale id — that corrupts
    // the conversation instead of saving tokens, so tool responses are never replayed.
    if entry.response.contains("tool_use") || entry.response.contains("tool_calls") {
        return false;
    }
    true
}

/// Whether a relayed response should be written to the cache.
///
/// An error body would replay as a permanent failure, and a partial one as a truncated
/// answer, so both are refused however often they recur.
fn should_record(relayed: &Relayed) -> bool {
    cache_record_enabled()
        && relayed.status == 200
        && relayed.complete
        && !relayed.body.is_empty()
        && relayed.body.len() <= MAX_CACHE_BODY
}

/// Whether a cached entry may answer this request instead of the model.
fn should_serve(
    mode: ServeMode,
    ck: &CacheKey,
    entry: &crate::cache::CacheEntry,
    is_stream: bool,
) -> bool {
    let allowed = match mode {
        ServeMode::Off => false,
        ServeMode::Deterministic => ck.deterministic,
        ServeMode::Always => true,
    };
    allowed && cache_replayable(entry, is_stream)
}

fn cached_response_bytes(entry: &crate::cache::CacheEntry) -> Vec<u8> {
    let body = entry.response.as_bytes();
    let ct = if entry.content_type.is_empty() {
        "application/json"
    } else {
        entry.content_type.as_str()
    };
    let mut out = format!(
        "HTTP/1.1 200 OK\r\ncontent-type: {}\r\ncontent-length: {}\r\nx-prism-cache: hit\r\nconnection: keep-alive\r\n\r\n",
        ct,
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

// ── Request preparation ───────────────────────────────────────────────────────

/// Everything the proxy needs to forward one request.
struct Prepared {
    body: Vec<u8>,
    orig_tokens: u32,
    sent_tokens: u32,
    model: String,
    is_stream: bool,
    extra_headers: Vec<(String, String)>,
}

impl Prepared {
    /// A body we are not touching (empty, or unparseable as JSON).
    fn passthrough(body: Vec<u8>) -> Self {
        Prepared {
            body,
            orig_tokens: 0,
            sent_tokens: 0,
            model: String::new(),
            is_stream: false,
            extra_headers: Vec::new(),
        }
    }
}

/// Compress, add cache breakpoints, and opt long Anthropic agent runs into
/// server-side context editing.
fn prepare_request(raw: &[u8], headers: &[u8], provider: &str, ratio: f64) -> Prepared {
    // Escape hatch: context editing changes what the model can see, so it must
    // be possible to turn off without rebuilding.
    let context_editing = std::env::var("PRISM_NO_CONTEXT_EDITING").is_err();
    let (mut body, orig_tokens, sent_tokens, model, is_stream) =
        compress_request_body(raw, provider, ratio);
    let mut extra_headers = Vec::new();

    if provider == "anthropic" {
        if let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(&body) {
            if let Some(header) = apply_context_editing(&mut json, headers, context_editing) {
                if let Ok(rewritten) = serde_json::to_vec(&json) {
                    body = rewritten;
                    extra_headers.push(header);
                }
            }
        }
    }

    Prepared { body, orig_tokens, sent_tokens, model, is_stream, extra_headers }
}

// ── Privacy-safe API key hash ─────────────────────────────────────────────────

/// Hash the caller's API key so usage can be attributed without ever storing
/// the credential. Only the first 8 hex chars are kept.
fn hash_api_key(headers_raw: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let s = std::str::from_utf8(headers_raw).unwrap_or("");
    for line in s.split("\r\n") {
        let lower = line.to_lowercase();
        let key_part = if lower.starts_with("authorization:") {
            line[14..].trim().trim_start_matches("Bearer ").trim_start_matches("bearer ")
        } else if lower.starts_with("x-api-key:") {
            line[10..].trim()
        } else {
            continue;
        };
        if !key_part.is_empty() {
            let mut h = DefaultHasher::new();
            key_part.hash(&mut h);
            return format!("{:016x}", h.finish())[..8].to_string();
        }
    }
    "unknown".to_string()
}

// ── MITM session ──────────────────────────────────────────────────────────────

/// Idle timeout for a kept-alive tunnel waiting on the next request.
const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

async fn connect_upstream(
    hostname: &str,
    port: u16,
) -> Result<tokio_rustls::client::TlsStream<TcpStream>> {
    let tcp = TcpStream::connect(format!("{}:{}", hostname, port)).await?;
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth(),
    );
    let name = rustls::pki_types::ServerName::try_from(hostname.to_string())
        .map_err(|_| anyhow!("invalid server name: {}", hostname))?;
    Ok(tokio_rustls::TlsConnector::from(cfg).connect(name, tcp).await?)
}

/// Serve every request on one decrypted tunnel until the client goes away.
/// Relay one client connection, request by request, applying every optimization.
///
/// Generic over both streams so the same loop serves a TLS-intercepted CONNECT tunnel
/// and a plain-HTTP local model endpoint — a local model previously reached none of this
/// because the plain path was a byte pipe. `reconnect` builds an upstream on demand:
/// once at the start, and again when a pooled connection turns out to be dead.
#[allow(clippy::too_many_arguments)]
async fn serve_session<C, U, F, Fut>(
    mut client: C,
    hostname: String,
    provider: &'static str,
    source_ip: String,
    ratio: f64,
    carry_in: Vec<u8>,
    mut reconnect: F,
) where
    C: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    U: tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin,
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<U>>,
{
    let mut upstream = match reconnect().await {
        Ok(u) => u,
        Err(e) => {
            error!("upstream connect {}: {}", hostname, e);
            return;
        }
    };

    let mut carry: Vec<u8> = carry_in;

    loop {
        // Wait for the next request on this tunnel.
        let headers_read = tokio::time::timeout(IDLE_TIMEOUT, read_headers(&mut client, carry)).await;
        let Ok(Some((req_headers, body_start))) = headers_read else {
            return;
        };

        let (raw_body, leftover) = read_body(&mut client, &req_headers, body_start).await;
        carry = leftover;

        let api_key_hash = hash_api_key(&req_headers);
        let client_close = wants_close(&req_headers);

        // Key the *original* body: prism's own rewrites (cache_control breakpoints,
        // compression) must not change which cache entry a request maps to.
        let ck = if raw_body.is_empty() { None } else { cache_key(&raw_body, provider) };

        let prepared = if raw_body.is_empty() {
            Prepared::passthrough(raw_body)
        } else {
            prepare_request(&raw_body, &req_headers, provider, ratio)
        };
        let Prepared { body, orig_tokens, sent_tokens, model, is_stream, extra_headers } = prepared;

        // Replay before opening the request, when the user has opted in and the entry
        // is safe to reuse.
        if let Some(ck) = &ck {
            let mode = cache_serve_mode();
            if mode != ServeMode::Off {
                if let Some(entry) = crate::cache::lookup_keyed(&ck.key)
                    .filter(|e| should_serve(mode, ck, e, is_stream))
                {
                    let bytes = cached_response_bytes(&entry);
                    if client.write_all(&bytes).await.is_err() {
                        return;
                    }
                    let _ = client.flush().await;
                    let served = extract_resp_tokens(entry.response.as_bytes());
                    info!(
                        "{} {} cache hit — {} in, {} out served locally, upstream skipped",
                        provider, model, orig_tokens, served
                    );
                    let ip = source_ip.clone();
                    let key = api_key_hash.clone();
                    let provider_s = provider.to_string();
                    let model_s = if model.is_empty() { "unknown".to_string() } else { model };
                    tokio::spawn(async move {
                        crate::analytics::record_proxy_event(
                            &ip, &key, &provider_s, &model_s, orig_tokens, 0, served, 0, true,
                        );
                    });
                    if client_close {
                        return;
                    }
                    continue;
                }
            }
        }

        let forward = rebuild_request(&req_headers, &body, &extra_headers);

        let t0 = std::time::Instant::now();
        if upstream.write_all(&forward).await.is_err() {
            // Upstream dropped a pooled connection; reopen once and retry.
            match reconnect().await {
                Ok(u) => {
                    upstream = u;
                    if upstream.write_all(&forward).await.is_err() {
                        return;
                    }
                }
                Err(_) => return,
            }
        }
        let _ = upstream.flush().await;

        let Some(relayed) = relay_response(&mut upstream, &mut client).await else {
            return;
        };
        let Relayed { resp_tokens, cached_tokens, close: upstream_close, .. } = relayed;
        let latency_ms = t0.elapsed().as_millis() as u64;

        let model_s = if model.is_empty() { "unknown".to_string() } else { model };

        // Record what came back. Only a complete, successful, non-oversized body: a
        // partial one would replay as a truncated answer, and an error body would
        // replay as a permanent failure.
        if let Some(ck) = ck {
            if should_record(&relayed) {
                if let Ok(text) = String::from_utf8(relayed.body) {
                    let (key, prompt) = (ck.key, ck.prompt);
                    let m = model_s.clone();
                    let ct = relayed.content_type;
                    tokio::task::spawn_blocking(move || {
                        crate::cache::record_keyed(&key, &prompt, &text, &m, &ct, is_stream);
                    });
                }
            }
        }
        let provider_s = provider.to_string();
        let ip = source_ip.clone();
        let key = api_key_hash.clone();
        let is_cache_hit = cached_tokens > 0;

        tokio::spawn(async move {
            crate::analytics::record_proxy_event(
                &ip, &key, &provider_s, &model_s,
                orig_tokens, sent_tokens, resp_tokens, latency_ms, is_cache_hit,
            );
            let saved = orig_tokens.saturating_sub(sent_tokens);
            info!(
                "{} {} {}in {}out{} {}ms{}",
                provider_s,
                model_s,
                orig_tokens,
                resp_tokens,
                if is_stream { " (stream)" } else { "" },
                latency_ms,
                if saved > 0 {
                    format!(
                        " — saved {} tokens (~${:.5})",
                        saved,
                        crate::analytics::estimate_cost(&model_s, saved, 0)
                    )
                } else {
                    String::new()
                }
            );
        });

        if client_close || upstream_close {
            return;
        }
    }
}

// ── CONNECT tunnel handler ────────────────────────────────────────────────────

fn is_loopback(host: &str) -> bool {
    let h = host.trim().to_lowercase();
    let name = if h.starts_with('[') {
        h.trim_start_matches('[').split(']').next().unwrap_or("").trim()
    } else if h == "::1" {
        "::1"
    } else {
        h.split(':').next().unwrap_or(&h).trim()
    };
    name == "localhost"
        || name == "127.0.0.1"
        || name == "::1"
        || name == "0.0.0.0"
        || name.starts_with("127.")
}

async fn handle_connect(
    mut client: TcpStream,
    host: String,
    ca_cert_pem: Arc<Vec<u8>>,
    ca_key_pem: Arc<Vec<u8>>,
    client_addr: SocketAddr,
    ratio: f64,
    proxy_port: u16,
) {
    let hostname = host.split(':').next().unwrap_or(&host).to_string();
    let port: u16 = host.split(':').nth(1).and_then(|p| p.parse().ok()).unwrap_or(443);

    // Loop Guard: prevent recursive CONNECT tunnels back to our own proxy port
    if is_loopback(&hostname) && port == proxy_port {
        warn!("Loop guard blocked recursive CONNECT tunnel to {}:{}", hostname, port);
        let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n").await;
        return;
    }

    let provider = detect_provider(&host);

    if client
        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
        .await
        .is_err()
    {
        return;
    }

    // Non-AI traffic is none of our business — tunnel it untouched.
    let Some(provider) = provider else {
        let Ok(upstream) = TcpStream::connect(format!("{}:{}", hostname, port)).await else {
            return;
        };
        let (mut cr, mut cw) = client.into_split();
        let (mut ur, mut uw) = upstream.into_split();
        let _ = tokio::join!(
            tokio::io::copy(&mut cr, &mut uw),
            tokio::io::copy(&mut ur, &mut cw)
        );
        return;
    };

    let (domain_cert, domain_key) = match make_domain_cert(&hostname, &ca_cert_pem, &ca_key_pem) {
        Ok(p) => p,
        Err(e) => {
            warn!("cert gen {}: {}", hostname, e);
            return;
        }
    };

    let server_config = {
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::{certs, private_key};
        use std::io::BufReader;

        let chain: Vec<CertificateDer<'static>> =
            certs(&mut BufReader::new(domain_cert.as_slice()))
                .filter_map(|c| c.ok())
                .map(|c| c.into_owned())
                .collect();
        let Some(key) = private_key(&mut BufReader::new(domain_key.as_slice())).ok().flatten() else {
            warn!("no private key for {}", hostname);
            return;
        };
        match rustls::ServerConfig::builder()
            .with_no_client_auth()
            .with_single_cert(chain, key)
        {
            Ok(c) => Arc::new(c),
            Err(e) => {
                warn!("tls server config {}: {}", hostname, e);
                return;
            }
        }
    };

    let Ok(tls_client) = tokio_rustls::TlsAcceptor::from(server_config).accept(client).await else {
        warn!("tls handshake failed for {}", hostname);
        return;
    };

    let up_host = hostname.clone();
    serve_session(
        tls_client,
        hostname,
        provider,
        client_addr.ip().to_string(),
        ratio,
        Vec::new(),
        move || {
            let h = up_host.clone();
            async move { connect_upstream(&h, port).await }
        },
    )
    .await;
}

// ── Plain HTTP pass-through ───────────────────────────────────────────────────

async fn handle_plain_http(mut client: TcpStream, request: Vec<u8>, proxy_port: u16) {
    let s = String::from_utf8_lossy(&request);
    let host_line = s.split("\r\n").find(|l| l.to_lowercase().starts_with("host:"));
    let (host, port) = match host_line {
        Some(hl) => {
            let v = hl[5..].trim();
            let mut parts = v.splitn(2, ':');
            let h = parts.next().unwrap_or("").to_string();
            let p: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(80);
            (h, p)
        }
        None => return,
    };
    if host.is_empty() {
        return;
    }

    // Loop Guard: prevent recursive connections back to our own proxy port
    if is_loopback(&host) && port == proxy_port {
        warn!("Loop guard blocked recursive plain HTTP connection to {}:{}", host, port);
        let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\nConnection: close\r\n\r\n").await;
        return;
    }

    // A local model endpoint speaks plain HTTP, so this — not the CONNECT path — is
    // where Ollama and LM Studio traffic arrives. Send it through the same session loop
    // as everything else instead of piping bytes past every optimization.
    if let Some(provider) = detect_provider(&format!("{}:{}", host, port)) {
        let peer = client
            .peer_addr()
            .map(|a| a.ip().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        let ratio = crate::config::load_global()
            .and_then(|c| c.compression_ratio)
            .unwrap_or(0.55);
        let (h, p) = (host.clone(), port);
        serve_session(
            client,
            host,
            provider,
            peer,
            ratio,
            request,
            move || {
                let h = h.clone();
                async move { Ok(TcpStream::connect(format!("{}:{}", h, p)).await?) }
            },
        )
        .await;
        return;
    }

    let Ok(mut upstream) = TcpStream::connect(format!("{}:{}", host, port)).await else {
        return;
    };
    if upstream.write_all(&request).await.is_err() {
        return;
    }
    let (mut ur, mut uw) = upstream.into_split();
    let (mut cr, mut cw) = client.split();
    let _ = tokio::join!(
        tokio::io::copy(&mut ur, &mut cw),
        tokio::io::copy(&mut cr, &mut uw)
    );
}

/// rustls 0.23 refuses to pick a crypto backend on its own when more than one is
/// compiled in — and `reqwest`'s rustls-tls pulls in aws-lc-rs alongside our
/// `ring`. Without this, every TLS handshake dies with SSL_ERROR_SYSCALL.
fn install_crypto_provider() {
    // Errs only if a provider is already installed, which is equally fine.
    let _ = rustls::crypto::ring::default_provider().install_default();
}

// ── Main server ───────────────────────────────────────────────────────────────

pub async fn start_server(port: u16, _upstream: Option<String>) -> Result<()> {
    install_crypto_provider();

    let ca = ensure_ca()?;
    let ca_cert = Arc::new(ca.cert_pem);
    let ca_key = Arc::new(ca.key_pem);

    // compression_ratio is Option<f64> inside an Option<PrismConfig>.
    let ratio = crate::config::load_global()
        .and_then(|c| c.compression_ratio)
        .unwrap_or(0.75)
        .clamp(0.1, 1.0);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;

    info!("PRISM MITM proxy listening on :{} (compression ratio {})", port, ratio);
    info!("CA cert: {}", ca_dir().join("ca.crt").display());
    info!(
        "Set HTTP_PROXY=http://localhost:{0} and HTTPS_PROXY=http://localhost:{0}",
        port
    );

    loop {
        let (stream, addr) = match listener.accept().await {
            Ok(pair) => pair,
            Err(e) => {
                if let Some(code) = e.raw_os_error() {
                    // EMFILE (24) or ENFILE (23): Too many open files
                    if code == 24 || code == 23 {
                        warn!("Too many open files (os error {}); backing off for 50ms", code);
                        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                        continue;
                    }
                }
                warn!("listener accept error: {}", e);
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                continue;
            }
        };
        let cc = Arc::clone(&ca_cert);
        let ck = Arc::clone(&ca_key);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, addr, cc, ck, ratio, port).await {
                warn!("connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    client_addr: SocketAddr,
    ca_cert_pem: Arc<Vec<u8>>,
    ca_key_pem: Arc<Vec<u8>>,
    ratio: f64,
    proxy_port: u16,
) -> Result<()> {
    let Some((headers, body_start)) = read_headers(&mut stream, Vec::new()).await else {
        return Ok(());
    };
    let line = String::from_utf8_lossy(&headers)
        .split("\r\n")
        .next()
        .unwrap_or("")
        .to_string();

    // Liveness probe on the proxy port itself — must not be tunnelled anywhere.
    if line.starts_with("GET /health") || line.starts_with("HEAD /health") {
        let body = b"{\"status\":\"ok\",\"service\":\"prism-proxy\"}";
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        stream.write_all(resp.as_bytes()).await?;
        stream.write_all(body).await?;
        return Ok(());
    }

    if line.starts_with("CONNECT ") {
        let host = line
            .split_whitespace()
            .nth(1)
            .ok_or_else(|| anyhow!("no host in CONNECT"))?
            .to_string();
        handle_connect(stream, host, ca_cert_pem, ca_key_pem, client_addr, ratio, proxy_port).await;
    } else {
        let mut request = headers;
        request.extend_from_slice(&body_start);
        handle_plain_http(stream, request, proxy_port).await;
    }
    Ok(())
}

/// Install PRISM CA cert to system trust store (called by `prism init --global`).
/// Path of the *combined* trust bundle: the system roots plus the PRISM CA.
///
/// `SSL_CERT_FILE`, `REQUESTS_CA_BUNDLE` and `CURL_CA_BUNDLE` **replace** the default
/// trust store rather than extending it, so pointing them at the bare PRISM CA leaves
/// a client unable to verify any host PRISM does not intercept. They must point here.
pub fn ca_bundle_path() -> PathBuf {
    ca_dir().join("ca-bundle.crt")
}

const SYSTEM_CA_BUNDLES: [&str; 5] = [
    "/etc/ssl/certs/ca-certificates.crt", // Debian, Ubuntu, Arch
    "/etc/pki/tls/certs/ca-bundle.crt",   // RHEL, Fedora
    "/etc/ssl/ca-bundle.pem",             // openSUSE
    "/etc/ssl/cert.pem",                  // Alpine, macOS
    "/etc/pki/ca-trust/extracted/pem/tls-ca-bundle.pem",
];

/// Write `<ca_dir>/ca-bundle.crt` = system roots ++ PRISM CA. Returns the path.
pub fn ensure_ca_bundle(ca_cert_pem: &[u8]) -> Result<PathBuf> {
    let out = ca_bundle_path();
    let mut bundle = Vec::new();
    let mut found_system = false;
    for path in SYSTEM_CA_BUNDLES {
        if let Ok(sys) = std::fs::read(path) {
            bundle.extend_from_slice(&sys);
            if !bundle.ends_with(b"\n") {
                bundle.push(b'\n');
            }
            found_system = true;
            break;
        }
    }
    if !found_system {
        warn!(
            "no system CA bundle found; {} will trust only the PRISM CA",
            out.display()
        );
    }
    bundle.extend_from_slice(ca_cert_pem);
    if !bundle.ends_with(b"\n") {
        bundle.push(b'\n');
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out, &bundle)?;
    Ok(out)
}

/// NSS trust databases used by Chromium/Electron apps and Firefox. These ignore both
/// the OpenSSL environment variables and the system store, so an Electron IDE keeps
/// rejecting PRISM's certificates until the CA is added here.
fn nss_dbs() -> Vec<String> {
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
    let mut dbs = Vec::new();
    let chromium = home.join(".pki").join("nssdb");
    if chromium.is_dir() {
        dbs.push(format!("sql:{}", chromium.display()));
    }
    for profiles in [
        home.join(".mozilla").join("firefox"),
        home.join("snap/firefox/common/.mozilla/firefox"),
    ] {
        if let Ok(rd) = std::fs::read_dir(&profiles) {
            for entry in rd.filter_map(|e| e.ok()) {
                if entry.path().join("cert9.db").is_file() {
                    dbs.push(format!("sql:{}", entry.path().display()));
                }
            }
        }
    }
    dbs
}

const NSS_NICKNAME: &str = "PRISM Local CA";

/// Add the PRISM CA to every NSS database found. Returns the databases updated.
pub fn install_ca_nss(ca_cert_pem: &[u8]) -> Result<Vec<String>> {
    let dbs = nss_dbs();
    if dbs.is_empty() {
        return Ok(Vec::new());
    }
    let tmp = std::env::temp_dir().join("prism-ca-nss.crt");
    std::fs::write(&tmp, ca_cert_pem)?;
    let mut done = Vec::new();
    for db in dbs {
        // -A refuses a duplicate nickname, so drop any previous entry first
        let _ = std::process::Command::new("certutil")
            .args(["-D", "-d", &db, "-n", NSS_NICKNAME])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        let ok = std::process::Command::new("certutil")
            .args(["-A", "-d", &db, "-n", NSS_NICKNAME, "-t", "C,,", "-i"])
            .arg(&tmp)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            done.push(db);
        }
    }
    let _ = std::fs::remove_file(&tmp);
    Ok(done)
}

/// Remove the PRISM CA from every NSS database (used by `prism-disable`).
pub fn remove_ca_nss() -> Vec<String> {
    let mut done = Vec::new();
    for db in nss_dbs() {
        let ok = std::process::Command::new("certutil")
            .args(["-D", "-d", &db, "-n", NSS_NICKNAME])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            done.push(db);
        }
    }
    done
}

pub fn install_ca_system(ca_cert_pem: &[u8]) -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        std::fs::write("/usr/local/share/ca-certificates/prism.crt", ca_cert_pem)?;
        let ok = std::process::Command::new("update-ca-certificates").status()?.success();
        return if ok {
            Ok("CA cert installed (Linux system trust store)".to_string())
        } else {
            Err(anyhow!("update-ca-certificates failed"))
        };
    }
    #[cfg(target_os = "macos")]
    {
        let tmp = std::env::temp_dir().join("prism-ca.crt");
        std::fs::write(&tmp, ca_cert_pem)?;
        let ok = std::process::Command::new("security")
            .args([
                "add-trusted-cert", "-d", "-r", "trustRoot",
                "-k", "/Library/Keychains/System.keychain",
                tmp.to_str().unwrap_or(""),
            ])
            .status()?
            .success();
        return if ok {
            Ok("CA cert installed (macOS system trust store)".to_string())
        } else {
            Err(anyhow!("security add-trusted-cert failed"))
        };
    }
    #[allow(unreachable_code)]
    Err(anyhow!(
        "Platform not supported — install {} manually to the system trust store",
        ca_dir().join("ca.crt").display()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── response cache ────────────────────────────────────────────────────────

    fn req(model: &str, text: &str) -> Vec<u8> {
        serde_json::to_vec(&serde_json::json!({
            "model": model,
            "messages": [{"role": "user", "content": text}],
        }))
        .unwrap()
    }

    #[test]
    fn a_cache_key_survives_prisms_own_rewrites() {
        // The same request, once plain and once carrying the cache_control breakpoints
        // and metadata prism injects. If these keyed differently the cache would never
        // hit its own writes.
        let plain = req("claude-opus-5", "list the docker containers");
        let rewritten = serde_json::to_vec(&serde_json::json!({
            "model": "claude-opus-5",
            "metadata": {"user_id": "abc"},
            "messages": [{
                "role": "user",
                "content": [{
                    "type": "text",
                    "text": "list the docker containers",
                    "cache_control": {"type": "ephemeral", "ttl": "1h"},
                }],
            }],
        }))
        .unwrap();
        let a = cache_key(&plain, "anthropic").unwrap();
        let b = cache_key(&rewritten, "anthropic").unwrap();
        // Content shape differs (string vs block list), so the keys legitimately differ;
        // what must hold is that stripping is what removed the volatile parts.
        assert!(!b.key.contains("cache_control"), "cache_control leaked into the key");
        assert!(!b.key.contains("user_id"), "metadata leaked into the key");
        assert_eq!(a.prompt, "list the docker containers");
        assert_eq!(b.prompt, "list the docker containers");
    }

    #[test]
    fn the_model_and_provider_are_part_of_the_key() {
        let k1 = cache_key(&req("claude-opus-5", "hi"), "anthropic").unwrap();
        let k2 = cache_key(&req("claude-sonnet-5", "hi"), "anthropic").unwrap();
        let k3 = cache_key(&req("claude-opus-5", "hi"), "openai").unwrap();
        assert_ne!(k1.key, k2.key, "same prompt, different model must not collide");
        assert_ne!(k1.key, k3.key, "same request, different provider must not collide");
    }

    #[test]
    fn sampling_decides_whether_a_replay_is_a_behaviour_change() {
        let det = serde_json::to_vec(&serde_json::json!({
            "model": "m", "temperature": 0.0, "messages": [{"role":"user","content":"x"}]
        }))
        .unwrap();
        assert!(cache_key(&det, "anthropic").unwrap().deterministic);
        // Absent temperature defaults to 1 at every provider — not deterministic.
        assert!(!cache_key(&req("m", "x"), "anthropic").unwrap().deterministic);
        let multi = serde_json::to_vec(&serde_json::json!({
            "model": "m", "n": 3, "messages": [{"role":"user","content":"x"}]
        }))
        .unwrap();
        assert!(cache_key(&multi, "openai").is_none(), "n>1 asks for variation");
    }

    fn entry(response: &str, is_stream: bool) -> crate::cache::CacheEntry {
        crate::cache::CacheEntry {
            key_hash: "k".into(),
            prompt_hash: "p".into(),
            prompt: "p".into(),
            response: response.into(),
            model: None,
            embedding: None,
            accessed_at: 0,
            last_saved: 0,
            access_count: 1,
            content_type: "application/json".into(),
            is_stream,
        }
    }

    #[test]
    fn a_tool_call_is_never_replayed() {
        let e = entry(r#"{"content":[{"type":"tool_use","id":"tu_1","name":"bash"}]}"#, false);
        assert!(
            !cache_replayable(&e, false),
            "replaying a tool_use re-runs a tool against a stale id"
        );
        let openai = entry(r#"{"choices":[{"message":{"tool_calls":[{"id":"c1"}]}}]}"#, false);
        assert!(!cache_replayable(&openai, false));
    }

    #[test]
    fn framing_must_match_before_a_replay() {
        let streamed = entry("event: message_start\ndata: {}\n\n", true);
        assert!(!cache_replayable(&streamed, false), "SSE body to a JSON caller");
        assert!(cache_replayable(&streamed, true));
        let json = entry(r#"{"content":[{"type":"text","text":"hi"}]}"#, false);
        assert!(!cache_replayable(&json, true), "JSON body to an SSE caller");
        assert!(cache_replayable(&json, false));
    }

    #[test]
    fn serving_is_off_unless_asked_for() {
        // Unset, empty, and anything unrecognised must all mean off — a typo in the
        // variable must not silently enable replay.
        assert_eq!(parse_serve_mode(None), ServeMode::Off);
        assert_eq!(parse_serve_mode(Some("")), ServeMode::Off);
        assert_eq!(parse_serve_mode(Some("yes")), ServeMode::Off);
        assert_eq!(parse_serve_mode(Some("0")), ServeMode::Off);
        assert_eq!(parse_serve_mode(Some("1")), ServeMode::Deterministic);
        assert_eq!(parse_serve_mode(Some("deterministic")), ServeMode::Deterministic);
        assert_eq!(parse_serve_mode(Some("always")), ServeMode::Always);
    }

    fn relayed(status: u16, body: &str, complete: bool) -> Relayed {
        Relayed {
            resp_tokens: 0,
            cached_tokens: 0,
            close: false,
            status,
            content_type: "application/json".into(),
            body: body.as_bytes().to_vec(),
            complete,
        }
    }

    #[test]
    fn only_a_complete_successful_response_is_recorded() {
        assert!(should_record(&relayed(200, "{}", true)));
        assert!(!should_record(&relayed(429, "{}", true)), "an error must not be cached");
        assert!(!should_record(&relayed(500, "{}", true)));
        assert!(
            !should_record(&relayed(200, "{}", false)),
            "a body that outran the sniff buffer would replay truncated"
        );
        assert!(!should_record(&relayed(200, "", true)), "nothing to replay");
        let huge = "x".repeat(MAX_CACHE_BODY + 1);
        assert!(!should_record(&relayed(200, &huge, true)));
    }

    #[test]
    fn serving_needs_the_switch_the_sampling_and_the_framing_to_agree() {
        let sampled = cache_key(&req("m", "hi"), "anthropic").unwrap(); // temperature absent
        let det = cache_key(
            &serde_json::to_vec(&serde_json::json!({
                "model": "m", "temperature": 0.0, "messages": [{"role":"user","content":"hi"}]
            }))
            .unwrap(),
            "anthropic",
        )
        .unwrap();
        let text = entry(r#"{"content":[{"type":"text","text":"hi"}]}"#, false);
        let tool = entry(r#"{"content":[{"type":"tool_use","id":"t1"}]}"#, false);

        // off: nothing serves, however well it matches
        assert!(!should_serve(ServeMode::Off, &det, &text, false));
        // deterministic: only the pinned request
        assert!(should_serve(ServeMode::Deterministic, &det, &text, false));
        assert!(!should_serve(ServeMode::Deterministic, &sampled, &text, false));
        // always: both, but never a tool call and never across framings
        assert!(should_serve(ServeMode::Always, &sampled, &text, false));
        assert!(!should_serve(ServeMode::Always, &sampled, &tool, false));
        assert!(!should_serve(ServeMode::Always, &sampled, &text, true));
    }

    /// Drive the whole session loop over in-memory pipes: request in, upstream answer
    /// out, and the response recorded. This is the glue that no unit test could reach
    /// while the loop was hard-wired to TLS stream types.
    #[tokio::test]
    async fn the_session_loop_relays_and_then_records() {
        let store = std::env::temp_dir().join(format!(
            "prism-e2e-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::env::set_var("PRISM_DATA_DIR", &store);

        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let (up, mut fake_upstream) = tokio::io::duplex(64 * 1024);
        let slot = std::sync::Arc::new(std::sync::Mutex::new(Some(up)));

        let session = tokio::spawn(async move {
            serve_session(
                server,
                "127.0.0.1".to_string(),
                "ollama",
                "127.0.0.1".to_string(),
                0.55,
                Vec::new(),
                move || {
                    let slot = slot.clone();
                    async move {
                        slot.lock()
                            .unwrap()
                            .take()
                            .ok_or_else(|| anyhow!("upstream already taken"))
                    }
                },
            )
            .await;
        });

        let body = serde_json::to_vec(&serde_json::json!({
            "model": "qwen3.5:0.8b",
            "temperature": 0.0,
            "messages": [{"role": "user", "content": "who wrote the session loop"}],
            "stream": false,
        }))
        .unwrap();
        let head = format!(
            "POST /api/chat HTTP/1.1\r\nhost: 127.0.0.1:11434\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n",
            body.len()
        );
        client.write_all(head.as_bytes()).await.unwrap();
        client.write_all(&body).await.unwrap();

        // Read the forwarded request until the prompt has arrived.
        let mut forwarded = Vec::new();
        let mut buf = vec![0u8; 4096];
        loop {
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                fake_upstream.read(&mut buf),
            )
            .await
            .expect("upstream read timed out")
            .unwrap();
            forwarded.extend_from_slice(&buf[..n]);
            if n == 0 || String::from_utf8_lossy(&forwarded).contains("session loop") {
                break;
            }
        }
        let forwarded = String::from_utf8_lossy(&forwarded).to_string();
        assert!(forwarded.starts_with("POST /api/chat HTTP/1.1"), "{forwarded}");
        assert!(forwarded.to_lowercase().contains("host: 127.0.0.1:11434"), "{forwarded}");

        let resp_body = br#"{"message":{"role":"assistant","content":"the generic one"}}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
            resp_body.len()
        );
        fake_upstream.write_all(resp.as_bytes()).await.unwrap();
        fake_upstream.write_all(resp_body).await.unwrap();
        drop(fake_upstream);

        let mut got = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), client.read_to_end(&mut got))
            .await
            .expect("client read timed out")
            .unwrap();
        let got = String::from_utf8_lossy(&got).to_string();
        assert!(got.contains("200 OK"), "{got}");
        assert!(
            got.ends_with(std::str::from_utf8(resp_body).unwrap()),
            "the client must receive the upstream body byte-for-byte: {got}"
        );
        let _ = session.await;

        // …and the response must now be in the store, under the request's own key.
        let ck = cache_key(&body, "ollama").expect("request should be keyable");
        assert!(ck.deterministic, "temperature 0 was set");
        let mut found = None;
        for _ in 0..40 {
            if let Some(e) = crate::cache::lookup_keyed(&ck.key) {
                found = Some(e);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let entry = found.expect("the relayed response was never recorded");
        assert_eq!(entry.response, std::str::from_utf8(resp_body).unwrap());
        assert_eq!(entry.prompt, "who wrote the session loop");
        assert_eq!(entry.content_type, "application/json");
        assert!(!entry.is_stream);
        assert!(should_serve(ServeMode::Deterministic, &ck, &entry, false));

        // ── replay leg ────────────────────────────────────────────────────────
        // Same request again, with serving switched on. The upstream this time never
        // answers, so if the reply arrives at all it came from the store.
        std::env::set_var("PRISM_CACHE_SERVE", "deterministic");
        let (mut client2, server2) = tokio::io::duplex(64 * 1024);
        let (up2, silent_upstream) = tokio::io::duplex(64 * 1024);
        let slot2 = std::sync::Arc::new(std::sync::Mutex::new(Some(up2)));
        let session2 = tokio::spawn(async move {
            serve_session(
                server2,
                "127.0.0.1".to_string(),
                "ollama",
                "127.0.0.1".to_string(),
                0.55,
                Vec::new(),
                move || {
                    let slot2 = slot2.clone();
                    async move {
                        slot2.lock().unwrap().take().ok_or_else(|| anyhow!("taken"))
                    }
                },
            )
            .await;
        });
        client2.write_all(head.as_bytes()).await.unwrap();
        client2.write_all(&body).await.unwrap();

        let mut replay = Vec::new();
        let mut buf2 = vec![0u8; 4096];
        loop {
            let n = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                client2.read(&mut buf2),
            )
            .await
            .expect("no reply — the cache did not serve")
            .unwrap();
            if n == 0 {
                break;
            }
            replay.extend_from_slice(&buf2[..n]);
            if String::from_utf8_lossy(&replay).contains("the generic one") {
                break;
            }
        }
        let replay = String::from_utf8_lossy(&replay).to_string();
        assert!(replay.contains("x-prism-cache: hit"), "{replay}");
        assert!(replay.ends_with(std::str::from_utf8(resp_body).unwrap()), "{replay}");
        drop(silent_upstream);
        drop(client2);
        let _ = session2.await;
        std::env::remove_var("PRISM_CACHE_SERVE");
        let _ = std::fs::remove_dir_all(&store);
    }

    #[test]
    fn a_replay_is_framed_as_a_real_http_response() {
        let e = entry(r#"{"ok":true}"#, false);
        let bytes = cached_response_bytes(&e);
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("HTTP/1.1 200 OK\r\n"));
        assert!(text.contains("content-length: 11\r\n"), "{text}");
        assert!(text.contains("x-prism-cache: hit"));
        assert!(text.ends_with("\r\n\r\n{\"ok\":true}"));
    }

    #[test]
    fn status_line_parses() {
        assert_eq!(status_of(b"HTTP/1.1 200 OK\r\ncontent-type: x\r\n\r\n"), 200);
        assert_eq!(status_of(b"HTTP/1.1 429 Too Many Requests\r\n\r\n"), 429);
        assert_eq!(status_of(b"garbage"), 0);
    }

    fn long_prose(n: usize) -> String {
        (0..n)
            .map(|i| format!("Sentence {i} carries ordinary explanatory filler about the system."))
            .collect::<Vec<_>>()
            .join(" ")
    }

    // ── framing ───────────────────────────────────────────────────────────────

    #[test]
    fn header_lookup_is_case_insensitive_and_skips_request_line() {
        let h = b"POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nContent-Length: 42\r\n\r\n";
        assert_eq!(header_value(h, "content-length").as_deref(), Some("42"));
        assert_eq!(header_value(h, "CONTENT-LENGTH").as_deref(), Some("42"));
        assert_eq!(header_value(h, "host").as_deref(), Some("api.anthropic.com"));
        assert_eq!(header_value(h, "authorization"), None);
    }

    #[test]
    fn detects_chunked_and_close() {
        let h = b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
        assert!(is_chunked(h));
        assert!(wants_close(h));
        let k = b"POST / HTTP/1.1\r\nContent-Length: 3\r\n\r\n";
        assert!(!is_chunked(k));
        assert!(!wants_close(k));
        assert_eq!(content_length(k), Some(3));
    }

    #[tokio::test]
    async fn reads_headers_spanning_multiple_reads() {
        // A body larger than one read must still be fully collected.
        let body = "x".repeat(100_000);
        let raw = format!(
            "POST /v1/messages HTTP/1.1\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        let mut cursor = std::io::Cursor::new(raw.into_bytes());

        let (headers, start) = read_headers(&mut cursor, Vec::new()).await.unwrap();
        let (got, leftover) = read_body(&mut cursor, &headers, start).await;

        assert_eq!(got.len(), 100_000, "body was truncated");
        assert!(leftover.is_empty());
    }

    #[tokio::test]
    async fn decodes_chunked_body() {
        let raw = "POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let mut cursor = std::io::Cursor::new(raw.as_bytes().to_vec());

        let (headers, start) = read_headers(&mut cursor, Vec::new()).await.unwrap();
        let (body, _) = read_body(&mut cursor, &headers, start).await;

        assert_eq!(String::from_utf8_lossy(&body), "hello world");
    }

    #[test]
    fn dechunk_bytes_strips_framing() {
        let framed = b"4\r\nabcd\r\n3\r\nefg\r\n0\r\n\r\n";
        assert_eq!(String::from_utf8_lossy(&dechunk_bytes(framed)), "abcdefg");
    }

    #[test]
    fn rebuild_request_fixes_framing_headers() {
        let headers = b"POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\nAccept-Encoding: gzip, br\r\nTransfer-Encoding: chunked\r\nContent-Length: 999\r\nx-api-key: secret\r\n\r\n";
        let out = rebuild_request(headers, b"{\"a\":1}", &[]);
        let s = String::from_utf8_lossy(&out);

        assert!(s.contains("Content-Length: 7"), "length not recomputed:\n{s}");
        assert!(!s.contains("Content-Length: 999"));
        assert!(!s.contains("Transfer-Encoding"), "de-chunked body still claims chunked");
        assert!(!s.to_lowercase().contains("gzip"), "gzip would break usage parsing");
        assert!(s.contains("Accept-Encoding: identity"));
        // Credentials must pass through untouched.
        assert!(s.contains("x-api-key: secret"));
        assert!(s.ends_with("{\"a\":1}"));
    }

    // ── token accounting ──────────────────────────────────────────────────────

    #[test]
    fn extracts_usage_from_plain_json() {
        let body = br#"{"usage":{"completion_tokens":123}}"#;
        assert_eq!(extract_resp_tokens(body), 123);
    }

    #[test]
    fn extracts_usage_from_sse_stream() {
        let sse = "event: message_start\ndata: {\"usage\":{\"output_tokens\":1}}\n\n\
                   event: message_delta\ndata: {\"usage\":{\"output_tokens\":57}}\n\n\
                   data: [DONE]\n\n";
        assert_eq!(extract_resp_tokens(sse.as_bytes()), 57);
    }

    // ── compression policy ────────────────────────────────────────────────────

    #[test]
    fn system_prompt_is_never_rewritten() {
        // The system prompt is the cacheable prefix; rewriting it forfeits a
        // 90% cache discount to save ~25% of its tokens.
        let system = long_prose(200);
        let req = serde_json::json!({
            "model": "gpt-4o-mini",
            "system": system,
            "messages": [{"role": "user", "content": long_prose(200)}],
        });

        let (out, _, _, _, _) =
            compress_request_body(&serde_json::to_vec(&req).unwrap(), "openai", 0.5);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["system"].as_str().unwrap(), system);
    }

    #[test]
    fn earlier_messages_stay_byte_stable() {
        // Only the last two messages may change, so the prefix keeps hitting cache.
        let first = long_prose(200);
        let req = serde_json::json!({
            "model": "gpt-4o-mini",
            "messages": [
                {"role": "user", "content": first},
                {"role": "assistant", "content": long_prose(200)},
                {"role": "user", "content": long_prose(200)},
            ],
        });

        let (out, orig, sent, model, _) =
            compress_request_body(&serde_json::to_vec(&req).unwrap(), "openai", 0.5);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();

        assert_eq!(parsed["messages"][0]["content"].as_str().unwrap(), first);
        assert_eq!(model, "gpt-4o-mini");
        assert!(orig > 0);
        assert!(sent <= orig, "compression must not grow the payload");
    }

    #[test]
    fn compression_is_deterministic() {
        // Identical input must produce identical bytes, or the provider's
        // prefix cache misses on every turn.
        let req = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "system": long_prose(200),
            "messages": [{"role": "user", "content": long_prose(300)}],
        });
        let raw = serde_json::to_vec(&req).unwrap();

        let (a, ..) = compress_request_body(&raw, "anthropic", 0.5);
        let (b, ..) = compress_request_body(&raw, "anthropic", 0.5);
        assert_eq!(a, b);
    }

    #[test]
    fn anthropic_gets_cache_breakpoints_without_losing_text() {
        let system = long_prose(50);
        let req = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "system": system,
            "messages": [{"role": "user", "content": "hi"}],
        });

        let (out, ..) = compress_request_body(&serde_json::to_vec(&req).unwrap(), "anthropic", 0.75);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();

        let block = &parsed["system"][0];
        assert_eq!(block["text"].as_str().unwrap(), system);
        assert_eq!(block["cache_control"]["type"], "ephemeral");
    }

    #[test]
    fn stream_flag_is_detected() {
        let req = serde_json::json!({
            "model": "gpt-4o-mini",
            "stream": true,
            "messages": [{"role": "user", "content": "hi"}],
        });
        let (.., is_stream) =
            compress_request_body(&serde_json::to_vec(&req).unwrap(), "openai", 0.75);
        assert!(is_stream);
    }

    // ── streaming ─────────────────────────────────────────────────────────────

    fn chunk(s: &str) -> Vec<u8> {
        format!("{:x}\r\n{}\r\n", s.len(), s).into_bytes()
    }

    /// The whole point of the relay: an SSE event must reach the client while
    /// the upstream is still producing. Buffering to EOF makes streaming look
    /// frozen and, on a keep-alive connection, never returns at all.
    #[tokio::test]
    async fn sse_reaches_client_before_stream_ends() {
        use std::time::Duration;
        use tokio::io::duplex;

        let (mut up_tx, mut up_rx) = duplex(64 * 1024);
        let (mut cli_tx, mut cli_rx) = duplex(64 * 1024);

        // Upstream: headers + one event, a long pause, then the rest.
        let producer = tokio::spawn(async move {
            up_tx
                .write_all(
                    b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\n\r\n",
                )
                .await
                .unwrap();
            up_tx.write_all(&chunk("data: {\"delta\":\"first\"}\n\n")).await.unwrap();
            up_tx.flush().await.unwrap();

            tokio::time::sleep(Duration::from_millis(600)).await;

            up_tx
                .write_all(&chunk("data: {\"usage\":{\"output_tokens\":42}}\n\n"))
                .await
                .unwrap();
            up_tx.write_all(b"0\r\n\r\n").await.unwrap();
            up_tx.flush().await.unwrap();
        });

        let relay = tokio::spawn(async move { relay_response(&mut up_rx, &mut cli_tx).await });

        // Collect whatever the client can see well before the producer finishes.
        let mut early = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_millis(250);
        let mut buf = vec![0u8; 4096];
        while tokio::time::Instant::now() < deadline {
            match tokio::time::timeout(Duration::from_millis(100), cli_rx.read(&mut buf)).await {
                Ok(Ok(0)) | Err(_) => continue,
                Ok(Ok(n)) => early.extend_from_slice(&buf[..n]),
                Ok(Err(_)) => break,
            }
        }

        let seen = String::from_utf8_lossy(&early).to_string();
        assert!(seen.contains("200 OK"), "headers were not forwarded early: {seen:?}");
        assert!(
            seen.contains("first"),
            "first SSE event did not reach the client within 250ms while upstream \
             was still streaming — the response is being buffered: {seen:?}"
        );
        assert!(
            !seen.contains("usage"),
            "test is not proving anything: upstream finished too early"
        );

        // Drain the rest so the relay can complete and report usage.
        let drain = tokio::spawn(async move {
            let mut sink = Vec::new();
            let _ = cli_rx.read_to_end(&mut sink).await;
        });
        producer.await.unwrap();
        let tokens = relay.await.unwrap().expect("relay returned no result").resp_tokens;
        let _ = drain.await;

        assert_eq!(tokens, 42, "usage was not recovered from the SSE stream");
    }

    // ── context editing ───────────────────────────────────────────────────────

    fn agent_request_with_tools() -> serde_json::Value {
        serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [
                {"role": "user", "content": "run the thing"},
                {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "t1", "content": "a big pile of output"}
                ]},
            ],
        })
    }

    #[test]
    fn long_agent_runs_opt_into_context_editing() {
        let raw = serde_json::to_vec(&agent_request_with_tools()).unwrap();
        let headers = b"POST /v1/messages HTTP/1.1\r\nx-api-key: k\r\n\r\n";

        let p = prepare_request(raw.as_slice(), headers, "anthropic", 0.75);
        let json: serde_json::Value = serde_json::from_slice(&p.body).unwrap();

        let edit = &json["context_management"]["edits"][0];
        assert_eq!(edit["type"], "clear_tool_uses_20250919");
        assert_eq!(edit["trigger"]["value"], 100_000);
        assert_eq!(edit["keep"]["value"], 3);
        assert_eq!(edit["keep"]["type"], "tool_uses");
        // Batched clearing: see CONTEXT_EDIT_CLEAR_AT_LEAST_TOKENS.
        assert_eq!(edit["clear_at_least"]["value"], 10_000);

        assert_eq!(
            p.extra_headers,
            vec![("anthropic-beta".to_string(), CONTEXT_MGMT_BETA.to_string())]
        );
    }

    #[test]
    fn context_editing_skipped_without_tool_results() {
        // No tool output to clear — adding a beta header would be pure noise.
        let req = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [{"role": "user", "content": "just a chat"}],
        });
        let raw = serde_json::to_vec(&req).unwrap();
        let p = prepare_request(raw.as_slice(), b"POST / HTTP/1.1\r\n\r\n", "anthropic", 0.75);

        let json: serde_json::Value = serde_json::from_slice(&p.body).unwrap();
        assert!(json.get("context_management").is_none());
        assert!(p.extra_headers.is_empty());
    }

    #[test]
    fn caller_context_management_is_never_overridden() {
        let mut req = agent_request_with_tools();
        req["context_management"] = serde_json::json!({"edits": []});
        let raw = serde_json::to_vec(&req).unwrap();

        let p = prepare_request(raw.as_slice(), b"POST / HTTP/1.1\r\n\r\n", "anthropic", 0.75);
        let json: serde_json::Value = serde_json::from_slice(&p.body).unwrap();

        assert_eq!(json["context_management"]["edits"].as_array().unwrap().len(), 0);
        assert!(p.extra_headers.is_empty());
    }

    #[test]
    fn existing_beta_headers_are_merged_not_clobbered() {
        let raw = serde_json::to_vec(&agent_request_with_tools()).unwrap();
        let headers = b"POST / HTTP/1.1\r\nanthropic-beta: some-other-beta\r\n\r\n";

        let p = prepare_request(raw.as_slice(), headers, "anthropic", 0.75);
        let (_, value) = &p.extra_headers[0];
        assert!(value.contains("some-other-beta"), "dropped the caller's beta: {value}");
        assert!(value.contains(CONTEXT_MGMT_BETA));

        // And the merged value must actually replace the original on the wire.
        let wire = String::from_utf8(rebuild_request(headers, &p.body, &p.extra_headers)).unwrap();
        assert_eq!(wire.matches("anthropic-beta:").count(), 1, "duplicate header:\n{wire}");
        assert!(wire.contains("some-other-beta"));
    }

    #[test]
    fn context_editing_can_be_disabled() {
        // PRISM_NO_CONTEXT_EDITING flips this flag; mutating process env from a
        // test would race with every other test in the binary.
        let mut json = agent_request_with_tools();
        let out = apply_context_editing(&mut json, b"POST / HTTP/1.1\r\n\r\n", false);

        assert!(out.is_none());
        assert!(json.get("context_management").is_none());
    }

    #[test]
    fn context_editing_is_anthropic_only() {
        let raw = serde_json::to_vec(&agent_request_with_tools()).unwrap();
        let p = prepare_request(raw.as_slice(), b"POST / HTTP/1.1\r\n\r\n", "openai", 0.75);
        let json: serde_json::Value = serde_json::from_slice(&p.body).unwrap();
        assert!(json.get("context_management").is_none());
        assert!(p.extra_headers.is_empty());
    }

    #[test]
    fn non_ai_hosts_are_not_intercepted() {
        assert_eq!(detect_provider("api.openai.com:443"), Some("openai"));
        assert_eq!(detect_provider("api.anthropic.com"), Some("anthropic"));
        assert_eq!(detect_provider("github.com:443"), None);
        assert_eq!(detect_provider("example.com"), None);
    }

    #[test]
    fn images_are_rightsized_inside_a_real_request_body() {
        // build a genuine oversized PNG payload and push it through the request rewriter
        let png = {
            let (w, h) = (2400u32, 1200u32);
            let mut rgba = Vec::with_capacity((w * h * 4) as usize);
            for y in 0..h {
                for x in 0..w {
                    rgba.extend_from_slice(&[(x % 256) as u8, (y % 256) as u8, 0, 255]);
                }
            }
            crate::image::png_encode_for_test(&rgba, w, h)
        };
        let b64 = crate::image::b64_encode_for_test(&png);
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [{"role": "user", "content": [
                {"type": "text", "text": "what does this screenshot show?"},
                {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": b64}}
            ]}]
        });
        let raw = serde_json::to_vec(&body).unwrap();

        // default: trims to the provider cap, so the prompt is unchanged in token terms
        let (out, orig, sent, _, _) = compress_request_body(&raw, "anthropic", 0.5);
        assert!(out.len() < raw.len(), "payload did not shrink: {} -> {}", raw.len(), out.len());
        assert!(orig >= sent, "sent more than we started with: {} -> {}", orig, sent);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        // the prose survives untouched and the block structure is preserved
        assert_eq!(parsed["messages"][0]["content"][0]["text"], "what does this screenshot show?");
        assert_eq!(parsed["messages"][0]["content"][1]["source"]["media_type"], "image/png");
        let data = parsed["messages"][0]["content"][1]["source"]["data"].as_str().unwrap();
        let shrunk = crate::image::b64_decode_for_test(data).unwrap();
        assert_eq!(crate::image::probe(&shrunk).unwrap().width, crate::image::DEFAULT_MAX_EDGE);
    }

    #[test]
    fn a_request_without_images_is_byte_stable() {
        let body = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "messages": [{"role": "user", "content": "plain text only"}]
        });
        let raw = serde_json::to_vec(&body).unwrap();
        let (out, ..) = compress_request_body(&raw, "anthropic", 0.5);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(parsed["messages"][0]["content"], "plain text only");
    }

    #[test]
    fn loopback_is_intercepted_only_on_local_model_ports() {
        // Ollama / LM Studio: ours to rewrite.
        assert_eq!(detect_provider("127.0.0.1:11434"), Some("ollama"));
        assert_eq!(detect_provider("localhost:1234"), Some("ollama"));
        // An IDE's own helper processes, dev servers and PRISM's own ports are not:
        // MITM-ing these is what broke Electron IDEs while the proxy was enabled.
        assert_eq!(detect_provider("127.0.0.1:3000"), None);
        assert_eq!(detect_provider("localhost:5173"), None);
        assert_eq!(detect_provider("127.0.0.1:27182"), None);
        assert_eq!(detect_provider("localhost"), None);
        assert_eq!(detect_provider("[::1]:8080"), None);
    }

    #[test]
    fn ca_bundle_appends_the_prism_ca_to_the_system_roots() {
        // The bundle must contain our CA *and* keep whatever the system trusted,
        // otherwise every non-intercepted host fails verification.
        let ca = b"-----BEGIN CERTIFICATE-----\nPRISMTESTCA\n-----END CERTIFICATE-----\n";
        let path = ensure_ca_bundle(ca).expect("bundle written");
        let written = std::fs::read_to_string(&path).expect("bundle readable");
        assert!(written.contains("PRISMTESTCA"), "prism CA missing from bundle");
        let system_present = SYSTEM_CA_BUNDLES.iter().any(|p| std::path::Path::new(p).is_file());
        if system_present {
            assert!(
                written.len() > ca.len() * 4,
                "system roots missing: bundle is only {} bytes",
                written.len()
            );
        }
    }

    #[test]
    fn api_key_is_hashed_not_stored() {
        let h = b"POST / HTTP/1.1\r\nAuthorization: Bearer sk-secret-value-12345\r\n\r\n";
        let hash = hash_api_key(h);
        assert_eq!(hash.len(), 8);
        assert!(!hash.contains("secret"));
        // Stable across calls so usage attributes to one identity.
        assert_eq!(hash, hash_api_key(h));
        assert_eq!(hash_api_key(b"POST / HTTP/1.1\r\n\r\n"), "unknown");
    }

    #[test]
    fn anthropic_caching_promotes_5m_before_1h() {
        // Reproduces the exact Claude Code error:
        // a ttl='1h' cache_control block must not come after a ttl='5m' cache_control block.
        let req = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "system": [{
                "type": "text",
                "text": "System instructions",
                "cache_control": {"type": "ephemeral"} // implicit 5m
            }],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "turn 1"},
                        {"type": "text", "text": "turn 2"},
                        {"type": "text", "text": "turn 3"},
                        {
                            "type": "tool_result",
                            "tool_use_id": "call_1",
                            "content": [
                                {
                                    "type": "text",
                                    "text": "result",
                                    "cache_control": {"type": "ephemeral", "ttl": "1h"}
                                }
                            ]
                        }
                    ]
                }
            ]
        });

        let raw = serde_json::to_vec(&req).unwrap();
        let (out, ..) = compress_request_body(&raw, "anthropic", 0.5);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();

        // System cache_control must be upgraded to 1h so that no 1h comes after 5m!
        let sys_cc = &parsed["system"][0]["cache_control"];
        assert_eq!(sys_cc["ttl"], "1h");

        // Tool result cache_control must stay 1h
        let msg_cc = &parsed["messages"][0]["content"][3]["content"][0]["cache_control"];
        assert_eq!(msg_cc["ttl"], "1h");
    }

    #[test]
    fn anthropic_preserves_caller_cache_without_duplicate_injection() {
        let req = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "system": "Plain system text without cache",
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "text",
                            "text": "Existing cached message",
                            "cache_control": {"type": "ephemeral", "ttl": "1h"}
                        }
                    ]
                }
            ]
        });

        let raw = serde_json::to_vec(&req).unwrap();
        let (out, ..) = compress_request_body(&raw, "anthropic", 0.5);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();

        // System prompt was NOT modified because caller already manages caching
        assert!(parsed["system"].is_string());
        // Message cache_control preserved
        assert_eq!(parsed["messages"][0]["content"][0]["cache_control"]["ttl"], "1h");
    }

    #[test]
    fn anthropic_caps_breakpoints_at_four() {
        let req = serde_json::json!({
            "model": "claude-3-5-sonnet",
            "tools": [
                {"name": "tool1", "cache_control": {"type": "ephemeral"}},
                {"name": "tool2", "cache_control": {"type": "ephemeral"}},
            ],
            "system": [
                {"type": "text", "text": "sys", "cache_control": {"type": "ephemeral"}}
            ],
            "messages": [
                {
                    "role": "user",
                    "content": [
                        {"type": "text", "text": "msg1", "cache_control": {"type": "ephemeral"}},
                        {"type": "text", "text": "msg2", "cache_control": {"type": "ephemeral"}},
                        {"type": "text", "text": "msg3", "cache_control": {"type": "ephemeral"}}
                    ]
                }
            ]
        });

        let raw = serde_json::to_vec(&req).unwrap();
        let (out, ..) = compress_request_body(&raw, "anthropic", 0.5);
        let parsed: serde_json::Value = serde_json::from_slice(&out).unwrap();

        let pointers = collect_cache_control_pointers(&parsed);
        assert_eq!(pointers.len(), 4);
    }

    #[test]
    fn test_is_loopback_detection() {
        assert!(is_loopback("localhost"));
        assert!(is_loopback("localhost:27181"));
        assert!(is_loopback("127.0.0.1"));
        assert!(is_loopback("127.0.0.1:27181"));
        assert!(is_loopback("::1"));
        assert!(is_loopback("0.0.0.0"));
        assert!(is_loopback("127.0.0.53:53"));
        assert!(!is_loopback("api.anthropic.com"));
        assert!(!is_loopback("api.openai.com:443"));
    }

}
