//! Transparent MITM HTTPS proxy — intercepts all LLM API traffic for
//! compression, caching, and token monitoring.
//!
//! Setup (one-time):  prism init --global
//! Start proxy:       prism serve --port 8080
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
];

fn detect_provider(host: &str) -> Option<&'static str> {
    let host_lower = host.to_lowercase();
    let bare = host_lower.split(':').next().unwrap_or(&host_lower);
    AI_HOSTS.iter().find(|(h, _)| bare == *h).map(|(_, p)| *p)
}

// ── CA certificate management ─────────────────────────────────────────────────

pub struct CaBundle {
    pub cert_pem: Vec<u8>,
    pub key_pem: Vec<u8>,
}

pub fn ca_dir() -> PathBuf {
    dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("prism")
        .join("ca")
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

// ── Request compression ───────────────────────────────────────────────────────

/// Compress `messages[].content` in an LLM API JSON body.
/// Returns (modified_body, orig_tokens, sent_tokens, model).
fn compress_request_body(body: &[u8], provider: &str) -> (Vec<u8>, u32, u32, String) {
    let Ok(mut json) = serde_json::from_slice::<serde_json::Value>(body) else {
        return (body.to_vec(), 0, 0, String::new());
    };

    let model = json.get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let mut orig_total = 0u32;
    let mut sent_total = 0u32;

    if let Some(messages) = json.get_mut("messages").and_then(|v| v.as_array_mut()) {
        for msg in messages.iter_mut() {
            if let Some(content) = msg.get_mut("content") {
                if let Some(text) = content.as_str() {
                    let ot = crate::analytics::count_tokens(text, &model).unwrap_or(0) as u32;
                    orig_total += ot;
                    if ot > 500 {
                        let c = crate::compress::compress(text, 0.75);
                        *content = serde_json::Value::String(c.compressed);
                        sent_total += c.compressed_tokens as u32;
                    } else {
                        sent_total += ot;
                    }
                } else if let Some(blocks) = content.as_array_mut() {
                    for block in blocks.iter_mut() {
                        if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                            if let Some(text_val) = block.get("text").and_then(|t| t.as_str()).map(|s| s.to_string()) {
                                let ot = crate::analytics::count_tokens(&text_val, &model).unwrap_or(0) as u32;
                                orig_total += ot;
                                if ot > 500 {
                                    let c = crate::compress::compress(&text_val, 0.75);
                                    sent_total += c.compressed_tokens as u32;
                                    if let Some(t) = block.get_mut("text") {
                                        *t = serde_json::Value::String(c.compressed);
                                    }
                                } else {
                                    sent_total += ot;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Inject Anthropic context caching on system prompt (saves 90% on repeated contexts)
    if provider == "anthropic" {
        if let Some(system) = json.get_mut("system") {
            if let Some(text) = system.as_str().map(|s| s.to_string()) {
                *system = serde_json::json!([{
                    "type": "text",
                    "text": text,
                    "cache_control": {"type": "ephemeral"}
                }]);
            }
        }
    }

    let modified = serde_json::to_vec(&json).unwrap_or_else(|_| body.to_vec());
    (modified, orig_total, sent_total, model)
}

// ── Privacy-safe API key hash ─────────────────────────────────────────────────

fn hash_api_key(headers_raw: &[u8]) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    let s = std::str::from_utf8(headers_raw).unwrap_or("");
    for line in s.lines() {
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

// ── CONNECT tunnel handler ────────────────────────────────────────────────────

async fn handle_connect(
    mut client: TcpStream,
    host: String,
    ca_cert_pem: Arc<Vec<u8>>,
    ca_key_pem: Arc<Vec<u8>>,
    client_addr: SocketAddr,
) {
    let provider = detect_provider(&host);
    let hostname = host.split(':').next().unwrap_or(&host).to_string();
    let port: u16 = host.split(':').nth(1).and_then(|p| p.parse().ok()).unwrap_or(443);

    // Acknowledge CONNECT
    if client.write_all(b"HTTP/1.1 200 Connection established\r\n\r\n").await.is_err() {
        return;
    }

    if provider.is_none() {
        // Not an AI host — raw passthrough tunnel
        let Ok(upstream) = TcpStream::connect(format!("{}:{}", hostname, port)).await else { return; };
        let (mut cr, mut cw) = client.into_split();
        let (mut ur, mut uw) = upstream.into_split();
        tokio::join!(tokio::io::copy(&mut cr, &mut uw), tokio::io::copy(&mut ur, &mut cw));
        return;
    }

    let provider = provider.unwrap();

    // Generate domain cert
    let (domain_cert, domain_key) = match make_domain_cert(&hostname, &ca_cert_pem, &ca_key_pem) {
        Ok(p) => p,
        Err(e) => { warn!("Cert gen for {}: {}", hostname, e); return; }
    };

    // Build TLS server config (presented to client)
    let server_config = {
        use rustls::pki_types::CertificateDer;
        use rustls_pemfile::{certs, private_key};
        use std::io::BufReader;

        let certs_vec: Vec<CertificateDer<'static>> =
            certs(&mut BufReader::new(domain_cert.as_slice()))
                .filter_map(|c| c.ok())
                .map(|c| c.into_owned())
                .collect();
        let key = match private_key(&mut BufReader::new(domain_key.as_slice())).ok().flatten() {
            Some(k) => k,
            None => { warn!("No key for {}", hostname); return; }
        };
        match rustls::ServerConfig::builder().with_no_client_auth().with_single_cert(certs_vec, key) {
            Ok(c) => Arc::new(c),
            Err(e) => { warn!("TLS server config: {}", e); return; }
        }
    };

    // TLS handshake with client (we impersonate the AI host)
    let Ok(mut tls_client) = tokio_rustls::TlsAcceptor::from(server_config).accept(client).await else {
        warn!("TLS accept failed for {}", hostname);
        return;
    };

    // Read decrypted HTTP request
    let mut req_buf = vec![0u8; 65536];
    let n = match tls_client.read(&mut req_buf).await {
        Ok(n) if n > 0 => n,
        _ => return,
    };
    let request_bytes = &req_buf[..n];

    // Split headers / body
    let header_end = request_bytes.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .map(|i| i + 4)
        .unwrap_or(n);
    let headers_raw = &request_bytes[..header_end.min(n)];
    let body_raw = if header_end < n { &request_bytes[header_end..] } else { &[] };

    let api_key_hash = hash_api_key(headers_raw);
    let is_post = request_bytes.starts_with(b"POST");

    let (final_body, orig_tokens, sent_tokens, model) = if is_post && !body_raw.is_empty() {
        compress_request_body(body_raw, provider)
    } else {
        (body_raw.to_vec(), 0, 0, String::new())
    };

    // Connect to real upstream with proper TLS
    let Ok(upstream_tcp) = TcpStream::connect(format!("{}:{}", hostname, port)).await else {
        error!("Upstream connect failed: {}", hostname);
        return;
    };
    let mut root_store = rustls::RootCertStore::empty();
    root_store.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let client_cfg = Arc::new(
        rustls::ClientConfig::builder()
            .with_root_certificates(root_store)
            .with_no_client_auth(),
    );
    let server_name = match rustls::pki_types::ServerName::try_from(hostname.clone()) {
        Ok(s) => s,
        Err(_) => return,
    };
    let Ok(mut tls_upstream) = tokio_rustls::TlsConnector::from(client_cfg)
        .connect(server_name, upstream_tcp).await else {
        error!("TLS connect to {} failed", hostname);
        return;
    };

    // Rebuild request with compressed body (update Content-Length)
    let forward_request = if is_post && !final_body.is_empty() {
        let header_str = std::str::from_utf8(headers_raw).unwrap_or("");
        let updated: String = header_str.lines()
            .filter(|l| !l.is_empty())
            .map(|line| {
                if line.to_lowercase().starts_with("content-length:") {
                    format!("Content-Length: {}\r\n", final_body.len())
                } else {
                    format!("{}\r\n", line)
                }
            })
            .collect();
        let mut req = updated.into_bytes();
        req.extend_from_slice(b"\r\n");
        req.extend_from_slice(&final_body);
        req
    } else {
        request_bytes.to_vec()
    };

    // Forward & collect response
    let t0 = std::time::Instant::now();
    if tls_upstream.write_all(&forward_request).await.is_err() { return; }

    let mut resp_buf: Vec<u8> = Vec::with_capacity(32768);
    let mut tmp = vec![0u8; 8192];
    loop {
        match tls_upstream.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => resp_buf.extend_from_slice(&tmp[..n]),
        }
    }
    let latency_ms = t0.elapsed().as_millis() as u64;

    // Count response tokens from usage field
    let resp_tokens: u32 = resp_buf.windows(4)
        .position(|w| w == b"\r\n\r\n")
        .and_then(|i| serde_json::from_slice::<serde_json::Value>(&resp_buf[i + 4..]).ok())
        .and_then(|j| {
            j.pointer("/usage/completion_tokens")
                .or_else(|| j.pointer("/usage/output_tokens"))
                .and_then(|v| v.as_u64())
        })
        .unwrap_or(0) as u32;

    // Send response back to client
    let _ = tls_client.write_all(&resp_buf).await;

    // Log telemetry
    let provider_s = provider.to_string();
    let model_s = if model.is_empty() { "unknown".to_string() } else { model };
    let source_ip = client_addr.ip().to_string();
    tokio::spawn(async move {
        crate::analytics::record_proxy_event(
            &source_ip, &api_key_hash, &provider_s, &model_s,
            orig_tokens, sent_tokens, resp_tokens, latency_ms, false,
        );
        if orig_tokens > 0 && sent_tokens < orig_tokens {
            let saved = orig_tokens - sent_tokens;
            info!("[PRISM proxy] {} {} — {} → {} tokens ({} saved, ~${:.5})",
                provider_s, model_s, orig_tokens, sent_tokens, saved,
                crate::analytics::estimate_cost(&model_s, saved, 0));
        }
    });
}

// ── Plain HTTP pass-through ───────────────────────────────────────────────────

async fn handle_plain_http(mut client: TcpStream, request: Vec<u8>) {
    let s = std::str::from_utf8(&request).unwrap_or("");
    let host_line = s.lines().find(|l| l.to_lowercase().starts_with("host:"));
    let (host, port) = if let Some(hl) = host_line {
        let v = hl[5..].trim();
        let mut parts = v.splitn(2, ':');
        let h = parts.next().unwrap_or("localhost").to_string();
        let p: u16 = parts.next().and_then(|p| p.parse().ok()).unwrap_or(80);
        (h, p)
    } else {
        ("localhost".to_string(), 80)
    };
    if let Ok(mut upstream) = TcpStream::connect(format!("{}:{}", host, port)).await {
        let _ = upstream.write_all(&request).await;
        let mut buf = vec![0u8; 65536];
        if let Ok(n) = upstream.read(&mut buf).await {
            let _ = client.write_all(&buf[..n]).await;
        }
    }
}

// ── Main server ───────────────────────────────────────────────────────────────

pub async fn start_server(port: u16, _upstream: Option<String>) -> Result<()> {
    let ca = ensure_ca()?;
    let ca_cert = Arc::new(ca.cert_pem);
    let ca_key  = Arc::new(ca.key_pem);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let listener = TcpListener::bind(addr).await?;

    info!("PRISM transparent HTTPS proxy on :{}", port);
    info!("CA cert: {}", ca_dir().join("ca.crt").display());
    info!("Set: HTTP_PROXY=http://localhost:{0}  HTTPS_PROXY=http://localhost:{0}", port);

    loop {
        let (stream, addr) = listener.accept().await?;
        let cc = Arc::clone(&ca_cert);
        let ck = Arc::clone(&ca_key);
        tokio::spawn(async move {
            if let Err(e) = handle_connection(stream, addr, cc, ck).await {
                warn!("Connection error: {}", e);
            }
        });
    }
}

async fn handle_connection(
    mut stream: TcpStream,
    client_addr: SocketAddr,
    ca_cert_pem: Arc<Vec<u8>>,
    ca_key_pem: Arc<Vec<u8>>,
) -> Result<()> {
    let mut buf = vec![0u8; 4096];
    let n = stream.read(&mut buf).await?;
    if n == 0 { return Ok(()); }
    let request = &buf[..n];

    if request.starts_with(b"CONNECT ") {
        let host = std::str::from_utf8(request)?
            .lines()
            .next()
            .and_then(|l| l.split_whitespace().nth(1))
            .ok_or_else(|| anyhow!("No host in CONNECT"))?
            .to_string();
        handle_connect(stream, host, ca_cert_pem, ca_key_pem, client_addr).await;
    } else {
        handle_plain_http(stream, request.to_vec()).await;
    }
    Ok(())
}

/// Install PRISM CA cert to system trust store (called by `prism init --global`).
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
            .args(["add-trusted-cert", "-d", "-r", "trustRoot",
                   "-k", "/Library/Keychains/System.keychain",
                   tmp.to_str().unwrap_or("")])
            .status()?.success();
        return if ok {
            Ok("CA cert installed (macOS system trust store)".to_string())
        } else {
            Err(anyhow!("security add-trusted-cert failed"))
        };
    }
    #[allow(unreachable_code)]
    Err(anyhow!("Platform not supported — install {} manually to system trust store",
        ca_dir().join("ca.crt").display()))
}
