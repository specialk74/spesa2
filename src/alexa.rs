//! Alexa shopping list → SpesaApp (server only).
//!
//! Every few seconds the items of the Alexa shopping list are added to the
//! SpesaApp list and deleted from Alexa, so that "Alexa, aggiungi il latte alla
//! lista della spesa" ends up here.
//!
//! Amazon closed its official list API on 1 July 2024. This module uses the same
//! private endpoints as Home Assistant's "Alexa Devices" integration
//! (python library `aioamazondevices`), authenticated with the cookies obtained by
//! `tools/alexa_login.py`. Expired cookies are renewed with the refresh token and
//! saved back to the login file.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};

/// Path of the shopping-list API on the retail site (e.g. www.amazon.it).
const LISTS_API: &str = "alexashoppinglists/api/v2/lists";
/// Same user agent and app identity as aioamazondevices.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/152.0.0.0 Safari/537.36 Edg/152.0.0.0";
const APP_NAME: &str = "AioAmazonDevices";
const APP_VERSION: &str = "2.2.663733.0";
const APP_BUNDLE_ID: &str = "com.amazon.echo";
const CLIENT_OS: &str = "18.5";

#[derive(Debug)]
pub enum AlexaError {
    /// Not logged in (or the login could not be renewed): run the login script again.
    Auth(String),
    /// Network problems or unexpected answers: retried at the next round.
    Other(String),
}

impl std::fmt::Display for AlexaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlexaError::Auth(e) => write!(f, "login Alexa non valido ({e}): rifai tools/alexa_login.py"),
            AlexaError::Other(e) => write!(f, "{e}"),
        }
    }
}

type Result<T> = std::result::Result<T, AlexaError>;

fn other(e: impl std::fmt::Display) -> AlexaError {
    AlexaError::Other(e.to_string())
}

/// An item of the Alexa shopping list.
#[derive(Debug, Clone, PartialEq)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub version: i64,
}

/// Client for the Alexa shopping list, logged in with the data of `tools/alexa_login.py`.
pub struct Alexa {
    file: PathBuf,
    /// Content of the login file (kept whole, to save it back unchanged but for the cookies).
    login: Value,
    /// Retail site, e.g. `https://www.amazon.it`.
    site: String,
    /// Alexa web site, e.g. `https://alexa.amazon.it`.
    alexa_site: String,
    /// Amazon token service.
    api: String,
    language: String,
    agent: ureq::Agent,
    /// Cookies set by Amazon during this session (e.g. `csrf`).
    session_cookies: BTreeMap<String, String>,
    shopping_list: Option<String>,
    /// Items already added to SpesaApp whose deletion from Alexa failed:
    /// they must not be added twice.
    added: HashSet<String>,
}

impl Alexa {
    /// Loads the login file written by `tools/alexa_login.py`.
    pub fn load(file: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(file)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", file.display()))?;
        let login: Value = serde_json::from_str(&text)?;
        for key in ["site", "refresh_token", "website_cookies", "store_authentication_cookie"] {
            if login.get(key).is_none() {
                anyhow::bail!("{}: missing '{key}', run tools/alexa_login.py again", file.display());
            }
        }
        let site = login["site"].as_str().unwrap_or_default().trim_end_matches('/').to_string();
        let domain = site.rsplit_once("amazon.").map_or("com", |(_, d)| d).to_string();
        Ok(Self {
            file: file.to_path_buf(),
            alexa_site: site.replacen("://www.", "://alexa.", 1),
            api: login
                .get("api_url")
                .and_then(Value::as_str)
                .unwrap_or("https://api.amazon.com")
                .to_string(),
            language: language_for(&domain).to_string(),
            site,
            login,
            agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0) // a redirect means "go to the sign-in page"
                .timeout_global(Some(Duration::from_secs(30)))
                .build()
                .into(),
            session_cookies: BTreeMap::new(),
            shopping_list: None,
            added: HashSet::new(),
        })
    }

    /// Moves the active items of the Alexa shopping list to SpesaApp:
    /// `add` is called with each name, then the item is deleted from Alexa.
    /// Returns how many items were moved.
    pub fn sync(&mut self, mut add: impl FnMut(&str) -> anyhow::Result<()>) -> Result<usize> {
        if !self.session_cookies.contains_key("csrf") {
            // Validates the login and obtains the anti-CSRF cookie.
            self.call("GET", &format!("{}/api/users/me", self.alexa_site), None)?;
        }
        let list = match &self.shopping_list {
            Some(id) => id.clone(),
            None => {
                let id = self.find_shopping_list()?;
                self.shopping_list = Some(id.clone());
                id
            }
        };
        let items = self.items(&list)?;
        let mut moved = 0;
        for item in items {
            if !self.added.contains(&item.id) {
                add(&item.name).map_err(|e| other(format!("database: {e:#}")))?;
                self.added.insert(item.id.clone());
                moved += 1;
            }
            let url = format!("{}/{LISTS_API}/{list}/items/{}?version={}", self.site, item.id, item.version);
            self.call("DELETE", &url, Some(json!({})))?;
            self.added.remove(&item.id);
        }
        Ok(moved)
    }

    /// Reads the active items of the Alexa shopping list without changing anything.
    pub fn peek(&mut self) -> Result<Vec<Item>> {
        self.call("GET", &format!("{}/api/users/me", self.alexa_site), None)?;
        let list = self.find_shopping_list()?;
        self.items(&list)
    }

    fn find_shopping_list(&mut self) -> Result<String> {
        let resp = self.call("POST", &format!("{}/{LISTS_API}/fetch", self.site), Some(json!({})))?;
        resp["listInfoList"]
            .as_array()
            .into_iter()
            .flatten()
            .find(|l| l["listType"] == "SHOP")
            .and_then(|l| l["listId"].as_str())
            .map(str::to_string)
            .ok_or_else(|| other("Alexa shopping list not found"))
    }

    /// Active (not completed) items of a list.
    fn items(&mut self, list: &str) -> Result<Vec<Item>> {
        let url = format!("{}/{LISTS_API}/{list}/items/fetch?limit=100", self.site);
        let resp = self.call("POST", &url, Some(json!({})))?;
        Ok(resp["itemInfoList"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["itemStatus"] == "ACTIVE")
            .filter_map(|i| {
                Some(Item {
                    id: i["itemId"].as_str()?.to_string(),
                    name: i["itemName"].as_str()?.trim().to_string(),
                    version: i["version"].as_i64().unwrap_or(1),
                })
            })
            .filter(|i| !i.name.is_empty())
            .collect())
    }

    /// Request with automatic renewal of the cookies if the login expired.
    fn call(&mut self, method: &str, url: &str, body: Option<Value>) -> Result<Value> {
        match self.request(method, url, body.as_ref()) {
            Err(AlexaError::Auth(_)) => {
                self.refresh_cookies()?;
                self.request(method, url, body.as_ref())
            }
            result => result,
        }
    }

    fn cookie_header(&self) -> String {
        let mut cookies: BTreeMap<String, String> = self.login["website_cookies"]
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
            .collect();
        if let Some(token) = self.login["store_authentication_cookie"]["cookie"].as_str() {
            cookies.insert("session-token".into(), token.into());
        }
        cookies.insert("lc-acbit".into(), self.language.clone());
        cookies.extend(self.session_cookies.clone());
        cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ")
    }

    fn request(&mut self, method: &str, url: &str, body: Option<&Value>) -> Result<Value> {
        let headers = [
            ("Accept-Charset", "utf-8".to_string()),
            ("User-Agent", USER_AGENT.to_string()),
            ("Accept-Language", self.language.clone()),
            ("Cookie", self.cookie_header()),
        ];
        let csrf = self.session_cookies.get("csrf").cloned();
        let response = match (method, body) {
            ("GET", _) => {
                let mut req = self.agent.get(url);
                for (k, v) in &headers {
                    req = req.header(*k, v);
                }
                if let Some(c) = &csrf {
                    req = req.header("csrf", c);
                }
                req.call()
            }
            (_, body) => {
                let mut req = match method {
                    "POST" => self.agent.post(url),
                    "PUT" => self.agent.put(url),
                    "DELETE" => self.agent.delete(url).force_send_body(),
                    other_method => return Err(other(format!("unsupported method {other_method}"))),
                };
                for (k, v) in &headers {
                    req = req.header(*k, v);
                }
                if let Some(c) = &csrf {
                    req = req.header("csrf", c);
                }
                req.header("Content-Type", "application/json; charset=utf-8")
                    .send(body.map(Value::to_string).unwrap_or_default())
            }
        };
        let mut response = response.map_err(|e| other(format!("{method} {url}: {e}")))?;

        for set_cookie in response.headers().get_all("set-cookie") {
            if let Some((name, value)) = set_cookie
                .to_str()
                .ok()
                .and_then(|c| c.split(';').next())
                .and_then(|c| c.split_once('='))
            {
                self.session_cookies.insert(name.trim().into(), value.trim().into());
            }
        }

        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        match status {
            200..=299 if text.trim().is_empty() => Ok(Value::Null),
            200..=299 => serde_json::from_str(&text)
                // HTML instead of JSON: the sign-in page.
                .map_err(|_| AlexaError::Auth(format!("{method} {url}: not JSON"))),
            300..=399 | 401 | 403 | 407 => Err(AlexaError::Auth(format!("{method} {url}: HTTP {status}"))),
            _ => Err(other(format!("{method} {url}: HTTP {status}"))),
        }
    }

    /// Gets new website cookies from the refresh token and saves them to the login file.
    fn refresh_cookies(&mut self) -> Result<()> {
        let refresh_token = self.login["refresh_token"]
            .as_str()
            .ok_or_else(|| AlexaError::Auth("missing refresh_token".into()))?
            .to_string();
        let domain = self.site.split("://").nth(1).unwrap_or_default().to_string();
        let form = [
            ("app_name", APP_NAME),
            ("app_version", APP_VERSION),
            ("di.sdk.version", "6.12.4"),
            ("source_token", refresh_token.as_str()),
            ("package_name", APP_BUNDLE_ID),
            ("di.hw.version", "iPhone"),
            ("platform", "iOS"),
            ("requested_token_type", "auth_cookies"),
            ("source_token_type", "refresh_token"),
            ("di.os.name", "iOS"),
            ("di.os.version", CLIENT_OS),
            ("current_version", "6.12.4"),
            ("previous_version", "6.12.4"),
            ("domain", domain.as_str()),
        ];
        let mut response = self
            .agent
            .post(format!("{}/auth/token", self.api))
            .header("User-Agent", USER_AGENT)
            .header("Accept-Language", &self.language)
            .send_form(form)
            .map_err(|e| other(format!("auth/token: {e}")))?;
        let status = response.status().as_u16();
        let text = response.body_mut().read_to_string().unwrap_or_default();
        if status != 200 {
            return Err(AlexaError::Auth(format!("cookie renewal refused (HTTP {status})")));
        }
        let json: Value = serde_json::from_str(&text).map_err(other)?;
        let by_domain = json["response"]["tokens"]["cookies"]
            .as_object()
            .ok_or_else(|| AlexaError::Auth("cookie renewal: unexpected answer".into()))?;

        let mut website_cookies = serde_json::Map::new();
        for cookie in by_domain.values().filter_map(Value::as_array).flatten() {
            let (Some(name), Some(value)) = (cookie["Name"].as_str(), cookie["Value"].as_str()) else {
                continue;
            };
            let value = value.replace('"', "");
            if name == "session-token" {
                self.login["store_authentication_cookie"] = json!({ "cookie": value });
            }
            website_cookies.insert(name.to_string(), Value::String(value));
        }
        self.login["website_cookies"] = Value::Object(website_cookies);
        self.session_cookies.clear(); // a new csrf comes with the next request
        self.save().map_err(other)?;
        eprintln!("alexa: cookies renewed");
        Ok(())
    }

    /// Writes the login file back (atomically, readable only by the owner).
    fn save(&self) -> std::io::Result<()> {
        let tmp = self.file.with_extension("tmp");
        let text = serde_json::to_string_pretty(&self.login)?;
        {
            use std::io::Write;
            let mut options = std::fs::OpenOptions::new();
            options.write(true).create(true).truncate(true);
            #[cfg(unix)]
            std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
            options.open(&tmp)?.write_all(text.as_bytes())?;
        }
        std::fs::rename(&tmp, &self.file)
    }
}

/// `Accept-Language` for an Amazon domain (same choice as aioamazondevices).
fn language_for(domain: &str) -> &'static str {
    match domain {
        "it" => "it-IT",
        "de" => "de-DE",
        "fr" => "fr-FR",
        "es" => "es-ES",
        "co.uk" => "en-GB",
        "ca" => "en-CA",
        "com.au" => "en-AU",
        "in" => "en-IN",
        "co.jp" => "ja-JP",
        "com.br" => "pt-BR",
        "com.mx" => "es-MX",
        "nl" => "nl-NL",
        _ => "en-US",
    }
}

/// Starts the background synchronization: every `interval` the Alexa shopping
/// list is moved to the SpesaApp database. Errors are logged once; after a
/// login error the next attempts are spaced out (10 minutes).
pub fn spawn(login_file: PathBuf, interval: Duration) -> anyhow::Result<()> {
    let mut alexa = Alexa::load(&login_file)?;
    std::thread::Builder::new().name("alexa-sync".into()).spawn(move || {
        let mut last_error: Option<String> = None;
        loop {
            let pause = match alexa.sync(|name| crate::db::with_db(|conn| crate::db::add_from_voice(conn, name))) {
                Ok(moved) => {
                    if last_error.take().is_some() {
                        eprintln!("alexa: sync working again");
                    }
                    if moved > 0 {
                        eprintln!("alexa: {moved} item(s) moved to the list");
                    }
                    interval
                }
                Err(e) => {
                    let msg = e.to_string();
                    if last_error.as_ref() != Some(&msg) {
                        eprintln!("alexa: {msg}");
                        last_error = Some(msg);
                    }
                    match e {
                        AlexaError::Auth(_) => Duration::from_secs(600).max(interval),
                        AlexaError::Other(_) => interval,
                    }
                }
            };
            std::thread::sleep(pause);
        }
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use axum::extract::{Path as UrlPath, Query, State};
    use axum::http::{HeaderMap, StatusCode, header};
    use axum::response::{IntoResponse, Response};
    use axum::routing::{delete, get, post};
    use axum::{Form, Json, Router};

    /// Fake Amazon: valid session token "good", csrf "tok", one shopping list "L1".
    #[derive(Default)]
    struct Fake {
        items: Vec<(String, String, &'static str)>, // id, name, status
        renewals: usize,
        deletes: Vec<String>,
        fail_deletes: bool,
    }
    type Shared = Arc<Mutex<Fake>>;

    fn authorized(h: &HeaderMap) -> bool {
        h.get(header::COOKIE).and_then(|c| c.to_str().ok()).is_some_and(|c| c.contains("session-token=good"))
    }
    fn api_ok(h: &HeaderMap) -> bool {
        authorized(h) && h.get("csrf").is_some_and(|v| v == "tok")
    }

    async fn users_me(h: HeaderMap) -> Response {
        if !authorized(&h) {
            return StatusCode::UNAUTHORIZED.into_response();
        }
        ([(header::SET_COOKIE, "csrf=tok; Path=/; Secure")], Json(json!({"id": "C1", "countryOfResidence": "IT"})))
            .into_response()
    }
    async fn token(State(s): State<Shared>, Form(form): Form<BTreeMap<String, String>>) -> Response {
        if form.get("source_token").map(String::as_str) != Some("refresh") || form["requested_token_type"] != "auth_cookies" {
            return StatusCode::BAD_REQUEST.into_response();
        }
        s.lock().unwrap().renewals += 1;
        Json(json!({"response": {"tokens": {"cookies": {".amazon.it": [
            {"Name": "session-token", "Value": "\"good\""},
            {"Name": "ubid-acbit", "Value": "123"}
        ]}}}}))
        .into_response()
    }
    async fn lists(h: HeaderMap) -> Response {
        if !api_ok(&h) {
            return StatusCode::FORBIDDEN.into_response();
        }
        Json(json!({"listInfoList": [
            {"listId": "T1", "listType": "TODO"},
            {"listId": "L1", "listType": "SHOP", "listName": null}
        ]}))
        .into_response()
    }
    async fn items(State(s): State<Shared>, h: HeaderMap, UrlPath(list): UrlPath<String>) -> Response {
        if !api_ok(&h) || list != "L1" {
            return StatusCode::FORBIDDEN.into_response();
        }
        let items: Vec<_> = s.lock().unwrap().items.iter()
            .map(|(id, name, status)| json!({"itemId": id, "itemName": name, "itemStatus": status, "version": 3}))
            .collect();
        Json(json!({"itemInfoList": items})).into_response()
    }
    async fn delete_item(
        State(s): State<Shared>,
        h: HeaderMap,
        UrlPath((list, id)): UrlPath<(String, String)>,
        Query(q): Query<BTreeMap<String, String>>,
    ) -> Response {
        let mut s = s.lock().unwrap();
        if !api_ok(&h) || list != "L1" || q.get("version").map(String::as_str) != Some("3") || s.fail_deletes {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        s.items.retain(|(i, _, _)| *i != id);
        s.deletes.push(id);
        Json(json!({})).into_response()
    }

    fn start_fake(state: Shared) -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            tokio::runtime::Runtime::new().unwrap().block_on(async move {
                let app = Router::new()
                    .route("/api/users/me", get(users_me))
                    .route("/auth/token", post(token))
                    .route("/alexashoppinglists/api/v2/lists/fetch", post(lists))
                    .route("/alexashoppinglists/api/v2/lists/{list}/items/fetch", post(items))
                    .route("/alexashoppinglists/api/v2/lists/{list}/items/{id}", delete(delete_item))
                    .with_state(state);
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                axum::serve(listener, app).await.unwrap();
            });
        });
        format!("http://127.0.0.1:{}", addr.port())
    }

    fn login_file(base: &str, session: &str) -> PathBuf {
        static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("spesa-alexa-{}-{n}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("alexa.json");
        let login = json!({
            "site": base, "api_url": base, "refresh_token": "refresh",
            "website_cookies": {"at-acbit": "x"},
            "store_authentication_cookie": {"cookie": session},
            "customer_info": {"user_id": "u"}
        });
        std::fs::write(&file, login.to_string()).unwrap();
        file
    }

    #[test]
    fn moves_items_and_renews_expired_cookies() {
        let state = Shared::default();
        state.lock().unwrap().items = vec![
            ("i1".into(), "latte".into(), "ACTIVE"),
            ("i2".into(), "pane".into(), "ACTIVE"),
            ("i3".into(), "vecchio".into(), "COMPLETE"),
        ];
        let base = start_fake(state.clone());
        let file = login_file(&base, "expired");
        let mut alexa = Alexa::load(&file).unwrap();

        let mut added = Vec::new();
        let moved = alexa.sync(|n| { added.push(n.to_string()); Ok(()) }).unwrap();
        assert_eq!(moved, 2);
        assert_eq!(added, ["latte", "pane"]);
        {
            let s = state.lock().unwrap();
            assert_eq!(s.renewals, 1, "expired session renewed once");
            assert_eq!(s.deletes, ["i1", "i2"]);
            assert_eq!(s.items.len(), 1, "completed items are left alone");
        }
        // renewed cookies saved to the login file, other fields kept
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(saved["store_authentication_cookie"]["cookie"], "good");
        assert_eq!(saved["website_cookies"]["ubid-acbit"], "123");
        assert_eq!(saved["customer_info"]["user_id"], "u");

        // nothing new
        assert_eq!(alexa.sync(|_| panic!("nothing to add")).unwrap(), 0);
        assert_eq!(state.lock().unwrap().renewals, 1);
    }

    #[test]
    fn failed_delete_does_not_add_twice() {
        let state = Shared::default();
        state.lock().unwrap().items = vec![("i1".into(), "uova".into(), "ACTIVE")];
        state.lock().unwrap().fail_deletes = true;
        let base = start_fake(state.clone());
        let mut alexa = Alexa::load(&login_file(&base, "good")).unwrap();

        let mut added = 0;
        assert!(alexa.sync(|_| { added += 1; Ok(()) }).is_err());
        state.lock().unwrap().fail_deletes = false;
        assert_eq!(alexa.sync(|_| { added += 1; Ok(()) }).unwrap(), 0);
        assert_eq!(added, 1, "added once even if the first deletion failed");
        assert!(state.lock().unwrap().items.is_empty());
    }

    #[test]
    fn invalid_refresh_token_is_a_login_error() {
        let state = Shared::default();
        let base = start_fake(state);
        let file = login_file(&base, "expired");
        let mut text: Value = serde_json::from_str(&std::fs::read_to_string(&file).unwrap()).unwrap();
        text["refresh_token"] = json!("revoked");
        std::fs::write(&file, text.to_string()).unwrap();

        let err = Alexa::load(&file).unwrap().sync(|_| Ok(())).unwrap_err();
        assert!(matches!(err, AlexaError::Auth(_)), "{err}");
    }

    #[test]
    fn languages() {
        assert_eq!(language_for("it"), "it-IT");
        assert_eq!(language_for("co.uk"), "en-GB");
        assert_eq!(language_for("xyz"), "en-US");
    }
}
