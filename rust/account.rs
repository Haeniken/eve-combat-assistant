//! Вход EVE SSO с PKCE и ручная загрузка профиля. В игровом цикле запросов нет.
use crate::{
    engine::Catalog,
    model::{array, id, s},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use chrono::Utc;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, jwk::JwkSet};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    path::Path,
    sync::atomic::{AtomicBool, Ordering},
    thread,
    time::{Duration, Instant},
};
use url::Url;

pub const CALLBACK: &str = "http://127.0.0.1:37219/callback";
pub const SCOPES: &str = "esi-skills.read_skills.v1 esi-skills.read_skillqueue.v1 esi-clones.read_implants.v1 esi-location.read_ship_type.v1 esi-assets.read_assets.v1 esi-fittings.read_fittings.v1";

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Profile {
    pub character_id: i64,
    pub name: String,
    pub skills: BTreeMap<i32, i32>,
    pub implants: Vec<i32>,
    pub ship: Value,
    pub current_fit: Option<String>,
    pub fittings: Vec<Value>,
    pub warnings: Vec<String>,
}
impl Profile {
    pub fn load(directory: &Path) -> Self {
        std::fs::read(directory.join("character.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}
#[derive(Serialize, Deserialize)]
struct Tokens {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    client_id: String,
}
#[derive(Deserialize)]
struct Claims {
    sub: String,
    name: String,
    aud: Vec<String>,
    scp: Vec<String>,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .max_redirects(0)
        .build()
        .into()
}
fn json_response(
    result: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<Value, String> {
    let mut response = result.map_err(|error| match error {
        ureq::Error::StatusCode(code) => format!("EVE API: HTTP {code}"),
        _ => "Не удалось связаться с EVE API".into(),
    })?;
    response
        .body_mut()
        .with_config()
        .limit(32 * 1024 * 1024)
        .read_json()
        .map_err(|_| "Некорректный ответ EVE API".into())
}
fn random_string() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|_| "Не удалось создать ключ входа")?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
pub(crate) fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}
pub(crate) fn callback_code(target: &str, state: &str) -> Result<String, String> {
    let url = Url::parse(&format!("http://127.0.0.1:37219{target}"))
        .map_err(|_| "Неверный ответ входа")?;
    let params: Vec<_> = url.query_pairs().collect();
    let values = |key: &str| {
        params
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, v)| v.to_string())
            .collect::<Vec<_>>()
    };
    if url.path() != "/callback" || values("state") != [state] || !values("error").is_empty() {
        return Err("Вход отменён или ответ не прошёл проверку".into());
    }
    let codes = values("code");
    if codes.len() != 1 || codes[0].is_empty() {
        return Err("В ответе нет кода входа".into());
    }
    Ok(codes[0].clone())
}

fn authorize(
    client: &str,
    cancel: &AtomicBool,
    progress: &dyn Fn(String),
) -> Result<Tokens, String> {
    let listener = TcpListener::bind("127.0.0.1:37219").map_err(|_| "Порт входа 37219 занят")?;
    listener.set_nonblocking(true).map_err(|e| e.to_string())?;
    let verifier = random_string()?;
    let state = random_string()?;
    let mut url = Url::parse("https://login.eveonline.com/v2/oauth/authorize").unwrap();
    url.query_pairs_mut().extend_pairs([
        ("response_type", "code"),
        ("client_id", client),
        ("redirect_uri", CALLBACK),
        ("scope", SCOPES),
        ("state", &state),
        ("code_challenge", &challenge(&verifier)),
        ("code_challenge_method", "S256"),
    ]);
    webbrowser::open(url.as_str()).map_err(|_| "Не удалось открыть браузер")?;
    progress("Подтвердите вход в браузере…".into());
    let deadline = Instant::now() + Duration::from_secs(180);
    let code = loop {
        if cancel.load(Ordering::Relaxed) {
            return Err("Вход отменён".into());
        }
        if Instant::now() > deadline {
            return Err("Время ожидания входа истекло".into());
        }
        match listener.accept() {
            Ok((mut stream, _)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .map_err(|e| e.to_string())?;
                let mut bytes = Vec::new();
                let mut byte = [0u8; 1];
                while bytes.len() < 16384 && !bytes.ends_with(b"\r\n\r\n") {
                    if stream.read(&mut byte).unwrap_or(0) == 0 {
                        break;
                    }
                    bytes.push(byte[0]);
                }
                let request = String::from_utf8_lossy(&bytes);
                let mut words = request.lines().next().unwrap_or("").split_whitespace();
                let method = words.next().unwrap_or("");
                let target = words.next().unwrap_or("");
                let result = callback_code(target, &state);
                let ok = method == "GET" && result.is_ok();
                let status = if ok { "200 OK" } else { "400 Bad Request" };
                let body = if ok {
                    "EVE Combat Assistant: login received. You can close this tab."
                } else {
                    "Invalid login response. Return to the application."
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                if ok {
                    break result?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(100))
            }
            Err(_) => return Err("Не удалось принять ответ входа".into()),
        }
    };
    let value = json_response(
        agent()
            .post("https://login.eveonline.com/v2/oauth/token")
            .send_form([
                ("grant_type", "authorization_code"),
                ("code", &code),
                ("client_id", client),
                ("code_verifier", &verifier),
                ("redirect_uri", CALLBACK),
            ]),
    )?;
    let mut tokens: Tokens = serde_json::from_value(value).map_err(|_| "SSO не вернул токен")?;
    tokens.client_id = client.into();
    Ok(tokens)
}

fn validate(token: &str, client: &str) -> Result<Claims, String> {
    // Проверяем RSA-подпись, issuer, срок действия, обе аудитории и все разрешения.
    let header = jsonwebtoken::decode_header(token).map_err(|_| "Неверный токен EVE SSO")?;
    if header.alg != Algorithm::RS256 {
        return Err("Неверная подпись EVE SSO".into());
    }
    let jwks = json_response(agent().get("https://login.eveonline.com/oauth/jwks").call())?;
    let keys: JwkSet = serde_json::from_value(jwks).map_err(|_| "Неверные ключи EVE SSO")?;
    let key = keys
        .find(header.kid.as_deref().ok_or("SSO не указал ключ")?)
        .ok_or("Ключ SSO не найден")?;
    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_issuer(&[
        "https://login.eveonline.com",
        "https://login.eveonline.com/",
        "login.eveonline.com",
    ]);
    validation.set_audience(&[client]);
    validation.set_required_spec_claims(&["exp", "iss", "aud", "sub"]);
    validation.validate_nbf = true;
    let claims = jsonwebtoken::decode::<Claims>(
        token,
        &DecodingKey::from_jwk(key).map_err(|_| "Неверный ключ SSO")?,
        &validation,
    )
    .map_err(|_| "Проверка токена EVE SSO не пройдена")?
    .claims;
    if !claims.aud.iter().any(|aud| aud == "EVE Online")
        || SCOPES
            .split_whitespace()
            .any(|scope| !claims.scp.iter().any(|s| s == scope))
    {
        return Err("При входе выданы не все разрешения".into());
    }
    Ok(claims)
}

#[cfg(windows)]
fn protect(bytes: &[u8], encrypt: bool) -> Result<Vec<u8>, String> {
    use windows_sys::Win32::{
        Foundation::LocalFree,
        Security::Cryptography::{
            CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
        },
    };
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        let ok = if encrypt {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        };
        if ok == 0 {
            return Err("Windows не смогла открыть хранилище входа".into());
        }
        let result = std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec();
        LocalFree(output.pbData.cast());
        Ok(result)
    }
}
#[cfg(not(windows))]
fn protect(_: &[u8], _: bool) -> Result<Vec<u8>, String> {
    Err("Хранилище EVE SSO доступно в Windows".into())
}

fn store_tokens(directory: &Path, tokens: &Tokens) -> Result<(), String> {
    let encrypted = protect(
        &serde_json::to_vec(tokens).map_err(|e| e.to_string())?,
        true,
    )?;
    std::fs::write(directory.join("sso.tmp"), encrypted).map_err(|e| e.to_string())?;
    crate::platform::replace_file(&directory.join("sso.tmp"), &directory.join("sso.dat"))
}
fn refresh(directory: &Path, client: &str) -> Result<Tokens, String> {
    let encrypted =
        std::fs::read(directory.join("sso.dat")).map_err(|_| "Сначала войдите через EVE Online")?;
    let old: Tokens = serde_json::from_slice(&protect(&encrypted, false)?)
        .map_err(|_| "Повторите вход через EVE Online")?;
    if old.client_id != client {
        return Err("Client ID изменился: повторите вход".into());
    }
    let value = json_response(
        agent()
            .post("https://login.eveonline.com/v2/oauth/token")
            .send_form([
                ("grant_type", "refresh_token"),
                ("refresh_token", old.refresh_token.as_str()),
                ("client_id", client),
            ]),
    )?;
    let mut tokens: Tokens =
        serde_json::from_value(value).map_err(|_| "Повторите вход через EVE Online")?;
    tokens.client_id = client.into();
    Ok(tokens)
}

/// Общий преобразователь сохранённых фитов и снимка assets. Cargo никогда не становится модулем.
pub fn items_to_eft(
    ship: i32,
    name: &str,
    items: &[Value],
    catalog: &Catalog,
) -> Result<String, String> {
    let hull = catalog.get(ship);
    if hull["category"] != 6 {
        return Err("Неизвестный корпус в ESI".into());
    }
    let mut lines = vec![
        format!(
            "[{}, {}]",
            catalog.name(ship),
            name.replace(['\r', '\n', ']'], " ")
        ),
        String::new(),
    ];
    for prefix in ["LoSlot", "MedSlot", "HiSlot", "RigSlot", "SubSystemSlot"] {
        let mut rack: Vec<_> = items
            .iter()
            .filter(|item| s(item, "flag").starts_with(prefix))
            .collect();
        rack.sort_by_key(|item| s(item, "flag"));
        for item in rack {
            lines.push(catalog.name(id(item, "type_id") as i32).to_string());
        }
        lines.push(String::new());
    }
    for flag in ["DroneBay", "Cargo"] {
        for item in items
            .iter()
            .filter(|item| s(item, "flag") == flag && id(item, "quantity") > 0)
        {
            lines.push(format!(
                "{} x{}",
                catalog.name(id(item, "type_id") as i32),
                id(item, "quantity")
            ));
        }
        lines.push(String::new());
    }
    Ok(lines.join("\n"))
}

pub fn synchronize(
    directory: &Path,
    client: &str,
    login: bool,
    catalog: &Catalog,
    cancel: &AtomicBool,
    progress: &dyn Fn(String),
) -> Result<Profile, String> {
    if client.trim().is_empty() {
        return Err("Введите Client ID приложения EVE SSO".into());
    }
    let tokens = if login {
        authorize(client.trim(), cancel, progress)?
    } else {
        refresh(directory, client.trim())?
    };
    let claims = validate(&tokens.access_token, client.trim())?;
    let character_id: i64 = claims
        .sub
        .strip_prefix("CHARACTER:EVE:")
        .ok_or("Неверный персонаж SSO")?
        .parse()
        .map_err(|_| "Неверный персонаж SSO")?;
    if character_id <= 0 {
        return Err("Неверный персонаж SSO".into());
    }
    // Сохраняем новый refresh token сразу: EVE может ротировать его при обновлении.
    store_tokens(directory, &tokens)?;
    let http = agent();
    let get = |route: &str| {
        if cancel.load(Ordering::Relaxed) {
            return Err("Загрузка профиля остановлена".into());
        }
        json_response(
            http.get(format!("https://esi.evetech.net/characters/{character_id}/{route}").as_str())
                .header("Authorization", &format!("Bearer {}", tokens.access_token))
                .header("X-Compatibility-Date", "2026-10-27")
                .header(
                    "User-Agent",
                    "EVECombatAssistant/0.3 (haeniken; local desktop)",
                )
                .call(),
        )
    };
    progress("Загрузка навыков и имплантов…".into());
    let raw = get("skills")?;
    let mut profile = Profile {
        character_id,
        name: claims.name,
        ..Default::default()
    };
    for skill in array(&raw["skills"]) {
        let tid = id(skill, "skill_id") as i32;
        if catalog.get(tid)["category"] == 16 {
            profile
                .skills
                .insert(tid, id(skill, "active_skill_level") as i32);
        } else {
            profile.warnings.push(format!(
                "Навык {tid} отсутствует в справочнике: обновите программу"
            ));
        }
    }
    // ESI предупреждает об устаревших skills у персонажей, давно не входивших в игру.
    match get("skillqueue") {
        Ok(queue) => {
            for entry in array(&queue) {
                let finished = chrono::DateTime::parse_from_rfc3339(s(entry, "finish_date"))
                    .is_ok_and(|date| date <= Utc::now());
                let tid = id(entry, "skill_id") as i32;
                if finished && catalog.get(tid)["category"] == 16 {
                    let level = profile.skills.entry(tid).or_default();
                    *level = (*level).max(id(entry, "finished_level") as i32);
                }
            }
        }
        Err(error) => profile.warnings.push(format!("Очередь навыков: {error}")),
    }
    profile.implants = array(&get("implants")?)
        .iter()
        .filter_map(|v| v.as_i64().map(|id| id as i32))
        .collect();
    profile.ship = get("ship")?;
    match get("fittings") {
        Ok(fits) => profile.fittings = array(&fits).to_vec(),
        Err(e) => profile.warnings.push(format!("Сохранённые фиты: {e}")),
    }
    progress("Чтение модулей и карго текущего корабля…".into());
    let ship_id = id(&profile.ship, "ship_item_id");
    let mut items = Vec::new();
    let mut assets_complete = false;
    for page in 1..=1000 {
        match get(&format!("assets?page={page}")) {
            Ok(assets) => {
                for item in array(&assets)
                    .iter()
                    .filter(|item| id(item, "location_id") == ship_id)
                {
                    let mut row = item.clone();
                    row["flag"] = row["location_flag"].clone();
                    items.push(row);
                }
                if array(&assets).len() < 1000 {
                    assets_complete = true;
                    break;
                }
                if page == 1000 {
                    return Err("Превышен предел страниц assets".into());
                }
            }
            Err(e) => {
                profile.warnings.push(format!("Assets: {e}"));
                break;
            }
        }
    }
    if assets_complete
        && items.iter().any(|item| {
            s(item, "flag").starts_with("HiSlot") || s(item, "flag").starts_with("MedSlot")
        })
    {
        profile.current_fit = Some(items_to_eft(
            id(&profile.ship, "ship_type_id") as i32,
            s(&profile.ship, "ship_name"),
            &items,
            catalog,
        )?);
    } else {
        profile.warnings.push("В assets нет модулей текущего корабля. Вставьте EFT или выберите сохранённый фит. ESI обновляет assets с задержкой.".into());
    }
    let bytes = serde_json::to_vec_pretty(&profile).map_err(|e| e.to_string())?;
    std::fs::write(directory.join("character.tmp"), bytes).map_err(|e| e.to_string())?;
    crate::platform::replace_file(
        &directory.join("character.tmp"),
        &directory.join("character.json"),
    )?;
    Ok(profile)
}

pub fn logout(directory: &Path) -> Result<(), String> {
    for name in ["sso.dat", "sso.tmp", "character.json", "character.tmp"] {
        match std::fs::remove_file(directory.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(())
}

#[cfg(all(test, windows))]
mod tests {
    #[test]
    fn windows_protects_login_data_and_rejects_tampering() {
        let secret = b"synthetic-test-token";
        let encrypted = super::protect(secret, true).unwrap();
        assert_ne!(encrypted, secret);
        assert_eq!(super::protect(&encrypted, false).unwrap(), secret);
        let mut tampered = encrypted;
        tampered[0] ^= 1;
        assert!(super::protect(&tampered, false).is_err());
    }
}
