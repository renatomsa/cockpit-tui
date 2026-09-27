//! OAuth 2.0 do Google para "App para computador": autorização com PKCE e redirect para
//! `http://127.0.0.1:<porta>` (no WSL2 o Windows repassa o localhost para o Linux), troca
//! do código por tokens e renovação com o refresh token. Escopo só de leitura.

use std::{
    fs,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::TcpListener,
};

pub const SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
const AUTH_URI: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_URI: &str = "https://oauth2.googleapis.com/token";

#[derive(Clone, Debug)]
pub struct Client {
    pub id: String,
    pub secret: String,
    pub auth_uri: String,
    pub token_uri: String,
}

/// O JSON baixado do Google Cloud: `{"installed": {...}}` (app para computador) ou `{"web": {...}}`.
pub fn load_client(path: &Path) -> Result<Client> {
    #[derive(Deserialize)]
    struct Inner {
        client_id: String,
        client_secret: String,
        auth_uri: Option<String>,
        token_uri: Option<String>,
    }
    #[derive(Deserialize)]
    struct File {
        installed: Option<Inner>,
        web: Option<Inner>,
    }
    let raw = fs::read_to_string(path).with_context(|| format!("não li {}", path.display()))?;
    let f: File = serde_json::from_str(&raw).with_context(|| format!("{} não é um client secret do Google", path.display()))?;
    let i = f.installed.or(f.web).ok_or_else(|| anyhow!("{} não tem \"installed\" nem \"web\"", path.display()))?;
    Ok(Client {
        id: i.client_id,
        secret: i.client_secret,
        auth_uri: i.auth_uri.unwrap_or_else(|| AUTH_URI.into()),
        token_uri: i.token_uri.unwrap_or_else(|| TOKEN_URI.into()),
    })
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: String,
    /// Epoch (s) em que o access token vence.
    pub expires_at: i64,
    #[serde(default)]
    pub scope: String,
}

impl Token {
    pub fn fresh(&self) -> bool {
        self.expires_at > Utc::now().timestamp() + 60
    }
}

pub fn load_token(path: &Path) -> Option<Token> {
    serde_json::from_str(&fs::read_to_string(path).ok()?).ok()
}

/// Grava com permissão 0600: o refresh token dá acesso de leitura à agenda.
pub fn save_token(path: &Path, t: &Token) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let mut f = fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    f.write_all(serde_json::to_string_pretty(t)?.as_bytes())?;
    f.sync_all()?;
    fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    expires_in: i64,
    refresh_token: Option<String>,
    #[serde(default)]
    scope: String,
}

#[derive(Deserialize)]
struct TokenError {
    error: String,
    error_description: Option<String>,
}

async fn token_request(http: &reqwest::Client, c: &Client, form: &[(&str, &str)]) -> Result<TokenResponse> {
    let resp = http.post(&c.token_uri).form(form).send().await?;
    let status = resp.status();
    let body = resp.text().await?;
    if !status.is_success() {
        let msg = serde_json::from_str::<TokenError>(&body)
            .map(|e| format!("{}{}", e.error, e.error_description.map(|d| format!(": {d}")).unwrap_or_default()))
            .unwrap_or(body);
        bail!("Google recusou ({status}): {msg}");
    }
    Ok(serde_json::from_str(&body)?)
}

/// Renova o access token. O Google normalmente não manda refresh token novo: mantém o antigo.
pub async fn refresh(http: &reqwest::Client, c: &Client, t: &Token) -> Result<Token> {
    let r = token_request(
        http,
        c,
        &[
            ("client_id", &c.id),
            ("client_secret", &c.secret),
            ("refresh_token", &t.refresh_token),
            ("grant_type", "refresh_token"),
        ],
    )
    .await
    .map_err(|e| {
        if e.to_string().contains("invalid_grant") {
            anyhow!("o acesso à agenda foi revogado ou venceu; rode `cockpit auth google` de novo")
        } else {
            e
        }
    })?;
    Ok(Token {
        access_token: r.access_token,
        refresh_token: r.refresh_token.unwrap_or_else(|| t.refresh_token.clone()),
        expires_at: Utc::now().timestamp() + r.expires_in,
        scope: if r.scope.is_empty() { t.scope.clone() } else { r.scope },
    })
}

/// Fluxo interativo do `cockpit auth google`: abre o navegador, espera o Google devolver o
/// código no 127.0.0.1 (ou o usuário colar a URL) e grava o token.
pub async fn authorize(http: &reqwest::Client, c: &Client, token_path: &Path) -> Result<Token> {
    let verifier = random_token(48);
    let challenge = base64_url(&Sha256::digest(verifier.as_bytes()));
    let state = random_token(16);
    let listener = TcpListener::bind("127.0.0.1:0").await.context("não consegui abrir uma porta local")?;
    let port = listener.local_addr()?.port();
    let redirect = format!("http://127.0.0.1:{port}");
    let url = reqwest::Url::parse_with_params(
        &c.auth_uri,
        &[
            ("client_id", c.id.as_str()),
            ("redirect_uri", &redirect),
            ("response_type", "code"),
            ("scope", SCOPE),
            ("code_challenge", &challenge),
            ("code_challenge_method", "S256"),
            ("state", &state),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )?;

    println!("Abrindo o navegador para você autorizar a leitura da agenda (somente leitura).");
    println!("Se ele não abrir, acesse:\n\n  {url}\n");
    if let Err(e) = crate::platform::open_url(url.as_str()) {
        println!("({e})");
    }
    println!("Esperando a autorização… Se o navegador mostrar \"não é possível acessar esse site\",");
    println!("copie a URL inteira da barra de endereço e cole aqui:");

    let code = tokio::select! {
        r = wait_callback(&listener, &state) => r?,
        r = read_pasted(&state) => r?,
        _ = tokio::time::sleep(Duration::from_secs(600)) => bail!("passaram 10 min sem autorização"),
    };

    let r = token_request(
        http,
        c,
        &[
            ("client_id", &c.id),
            ("client_secret", &c.secret),
            ("code", &code),
            ("code_verifier", &verifier),
            ("redirect_uri", &redirect),
            ("grant_type", "authorization_code"),
        ],
    )
    .await?;
    let refresh_token = r.refresh_token.ok_or_else(|| {
        anyhow!("o Google não mandou refresh token; remova o acesso do app em myaccount.google.com/permissions e rode de novo")
    })?;
    let t = Token { access_token: r.access_token, refresh_token, expires_at: Utc::now().timestamp() + r.expires_in, scope: r.scope };
    save_token(token_path, &t)?;
    Ok(t)
}

/// Espera o redirect do Google no servidor local e responde uma página simples.
async fn wait_callback(listener: &TcpListener, state: &str) -> Result<String> {
    loop {
        let (mut sock, _) = listener.accept().await?;
        let mut buf = vec![0u8; 8192];
        let n = tokio::time::timeout(Duration::from_secs(5), sock.read(&mut buf)).await.unwrap_or(Ok(0)).unwrap_or(0);
        let req = String::from_utf8_lossy(&buf[..n]);
        let path = req.lines().next().and_then(|l| l.split_whitespace().nth(1)).unwrap_or("/");
        if !path.contains("code=") && !path.contains("error=") {
            let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
            continue; // favicon etc.
        }
        let result = parse_redirect(&format!("http://127.0.0.1{path}"), state);
        let (title, msg) = match &result {
            Ok(_) => ("Pronto", "Agenda conectada ao cockpit. Pode fechar esta aba."),
            Err(_) => ("Não deu certo", "Volte ao terminal para ver o motivo."),
        };
        let html = format!(
            "<!doctype html><meta charset=utf-8><title>cockpit</title><body style=\"font:16px system-ui;margin:3em\"><h2>{title}</h2><p>{msg}</p></body>"
        );
        let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{html}", html.len());
        let _ = sock.write_all(resp.as_bytes()).await;
        return result;
    }
}

/// Alternativa ao servidor local: o usuário cola a URL de retorno (ou só o código).
async fn read_pasted(state: &str) -> Result<String> {
    let mut lines = BufReader::new(tokio::io::stdin()).lines();
    loop {
        let Some(line) = lines.next_line().await? else {
            // sem stdin (não interativo): deixa só o servidor local esperando
            std::future::pending::<()>().await;
            unreachable!()
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with("http") {
            match parse_redirect(line, state) {
                Ok(code) => return Ok(code),
                Err(e) => println!("{e:#}; tente de novo:"),
            }
        } else {
            return Ok(line.to_string());
        }
    }
}

fn parse_redirect(url: &str, state: &str) -> Result<String> {
    let u = reqwest::Url::parse(url).context("URL inválida")?;
    let get = |k: &str| u.query_pairs().find(|(key, _)| key == k).map(|(_, v)| v.into_owned());
    if let Some(err) = get("error") {
        bail!("o Google devolveu erro: {err}");
    }
    if get("state").as_deref() != Some(state) {
        bail!("a resposta não é desta tentativa (state diferente)");
    }
    get("code").ok_or_else(|| anyhow!("a URL não tem o código"))
}

fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        use std::io::Read;
        let _ = f.read_exact(&mut buf);
    }
    base64_url(&buf)
}

/// Base64 URL-safe sem `=` (RFC 4648 §5), como o PKCE pede.
fn base64_url(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in input.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (b[0] as u32) << 16 | (b[1] as u32) << 8 | b[2] as u32;
        for i in 0..=chunk.len() {
            out.push(T[(n >> (18 - 6 * i) & 63) as usize] as char);
        }
    }
    out
}

pub fn client_path(cfg: &crate::config::CalendarConfig) -> PathBuf {
    crate::config::expand_home(&cfg.client_secret)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_rfc7636_example() {
        // RFC 7636, apêndice B
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        assert_eq!(base64_url(&Sha256::digest(verifier.as_bytes())), "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
        assert_eq!(base64_url(b"foobar"), "Zm9vYmFy");
        assert_eq!(base64_url(b"fo"), "Zm8");
        let t = random_token(48);
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
    }

    #[test]
    fn parses_redirect() {
        assert_eq!(parse_redirect("http://127.0.0.1:5555/?state=abc&code=4/0Ab&scope=x", "abc").unwrap(), "4/0Ab");
        assert!(parse_redirect("http://127.0.0.1:5555/?state=outro&code=x", "abc").is_err());
        assert!(parse_redirect("http://127.0.0.1:5555/?error=access_denied&state=abc", "abc").unwrap_err().to_string().contains("access_denied"));
    }

    #[tokio::test]
    async fn local_server_receives_the_code_and_ignores_favicon() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let browser = tokio::spawn(async move {
            let get = |path: &'static str| async move {
                let mut s = tokio::net::TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                s.write_all(format!("GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes()).await.unwrap();
                let mut out = String::new();
                s.read_to_string(&mut out).await.unwrap();
                out
            };
            let fav = get("/favicon.ico").await;
            let page = get("/?state=abc&code=4%2F0Ab-xyz&scope=x").await;
            (fav, page)
        });
        let code = wait_callback(&listener, "abc").await.unwrap();
        let (fav, page) = browser.await.unwrap();
        assert_eq!(code, "4/0Ab-xyz");
        assert!(fav.starts_with("HTTP/1.1 404"));
        assert!(page.contains("Agenda conectada"));
    }

    #[test]
    fn loads_client_secret_and_saves_token_privately() {
        let dir = std::env::temp_dir().join(format!("cockpit-oauth-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("client.json");
        fs::write(&p, r#"{"installed":{"client_id":"id.apps.googleusercontent.com","client_secret":"s","redirect_uris":["http://localhost"]}}"#).unwrap();
        let c = load_client(&p).unwrap();
        assert_eq!((c.id.as_str(), c.token_uri.as_str()), ("id.apps.googleusercontent.com", TOKEN_URI));
        fs::write(&p, r#"{"outra":{}}"#).unwrap();
        assert!(load_client(&p).is_err());

        let tp = dir.join("sub/token.json");
        let t = Token { access_token: "a".into(), refresh_token: "r".into(), expires_at: 1, scope: SCOPE.into() };
        save_token(&tp, &t).unwrap();
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(fs::metadata(&tp).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(load_token(&tp), Some(t.clone()));
        assert!(!t.fresh());
        fs::remove_dir_all(&dir).ok();
    }
}
