use crate::{
    database::{Database, eq, string},
    error::{ApiError, Result},
};
use axum::{
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use reqwest::Method;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Session {
    pub user: Value,
    pub access_token: String,
}
#[derive(Clone)]
pub struct Identity {
    pub user: Value,
    pub family_id: String,
    pub contributor: Option<Value>,
    pub membership: Value,
    pub is_self: bool,
    pub is_admin: bool,
}
impl Identity {
    pub fn contributor_id(&self) -> Result<&str> {
        self.contributor
            .as_ref()
            .map(|value| string(value, "id"))
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ApiError::new(403, "Contributor membership required"))
    }
    pub fn require_contributor(&self) -> Result<()> {
        self.contributor_id()?;
        if self.is_self {
            Err(ApiError::new(
                403,
                "Only family contributors can change this",
            ))
        } else {
            Ok(())
        }
    }
    pub fn assert_ownership(&self, contributor: &str, family: Option<&str>) -> Result<()> {
        if self.contributor_id()? != contributor
            || family.is_some_and(|value| value != self.family_id)
        {
            Err(ApiError::new(403, "Contributor or family mismatch"))
        } else {
            Ok(())
        }
    }
}

pub fn cookie_name(db: &Database) -> String {
    let project = url::Url::parse(&db.url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host.split('.').next().unwrap_or(host).to_owned())
        })
        .unwrap_or_default();
    format!("sb-{project}-auth-token")
}
pub fn read_cookie(headers: &HeaderMap, name: &str) -> Option<String> {
    let cookies: Vec<(String, String)> = headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|part| {
            let (key, value) = part.trim().split_once('=')?;
            Some((
                key.into(),
                url::form_urlencoded::parse(format!("v={value}").as_bytes())
                    .next()?
                    .1
                    .into_owned(),
            ))
        })
        .collect();
    if let Some((_, value)) = cookies.iter().find(|(key, _)| key == name) {
        return Some(value.clone());
    }
    let mut combined = String::new();
    for index in 0..20 {
        let Some((_, value)) = cookies
            .iter()
            .find(|(key, _)| key == &format!("{name}.{index}"))
        else {
            break;
        };
        combined.push_str(value);
    }
    (!combined.is_empty()).then_some(combined)
}
pub fn decode_cookie(value: &str) -> Option<Value> {
    let decoded = if let Some(encoded) = value.strip_prefix("base64-") {
        String::from_utf8(URL_SAFE_NO_PAD.decode(encoded).ok()?).ok()?
    } else {
        value.into()
    };
    serde_json::from_str(&decoded).ok()
}
pub fn session_cookies(db: &Database, session: &Value, headers: &HeaderMap) -> Vec<String> {
    let name = cookie_name(db);
    let mut session = session.clone();
    if session["expires_at"].is_null() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        session["expires_at"] = json!(now + session["expires_in"].as_u64().unwrap_or(3600));
    }
    let encoded = format!("base64-{}", URL_SAFE_NO_PAD.encode(session.to_string()));
    let secure = if headers
        .get("x-forwarded-proto")
        .is_some_and(|value| value == "https")
    {
        "; Secure"
    } else {
        ""
    };
    let suffix = format!("; Path=/; Max-Age=34560000; SameSite=Lax{secure}");
    let mut result = Vec::new();
    for key in std::iter::once(name.clone()).chain((0..20).map(|index| format!("{name}.{index}"))) {
        if read_cookie(headers, &key).is_some() {
            result.push(format!("{key}=; Path=/; Max-Age=0; SameSite=Lax{secure}"));
        }
    }
    let chunks: Vec<_> = encoded.as_bytes().chunks(3180).collect();
    for (index, chunk) in chunks.iter().enumerate() {
        let key = if chunks.len() == 1 {
            name.clone()
        } else {
            format!("{name}.{index}")
        };
        result.push(format!("{key}={}{suffix}", String::from_utf8_lossy(chunk)));
    }
    result
}
async fn verified_user(db: &Database, token: &str) -> Result<Value> {
    let response = db
        .client
        .get(format!("{}/auth/v1/user", db.url))
        .header("apikey", &db.public_key)
        .bearer_auth(token)
        .send()
        .await?;
    if !response.status().is_success() {
        return Err(ApiError::new(401, "Please sign in to continue."));
    }
    let user: Value = response.json().await?;
    if user["id"].as_str().is_none() {
        return Err(ApiError::new(401, "Invalid authentication"));
    }
    Ok(user)
}
async fn authenticate(db: &Database, headers: &HeaderMap) -> Result<(Session, Vec<String>)> {
    if let Some(authorization) = headers.get(header::AUTHORIZATION) {
        let authorization = authorization
            .to_str()
            .map_err(|_| ApiError::new(401, "Invalid authentication"))?;
        let (scheme, token) = authorization
            .split_once(' ')
            .ok_or_else(|| ApiError::new(401, "Invalid authentication"))?;
        if !scheme.eq_ignore_ascii_case("bearer")
            || token.is_empty()
            || token.contains(char::is_whitespace)
        {
            return Err(ApiError::new(401, "Invalid authentication"));
        }
        return Ok((
            Session {
                user: verified_user(db, token).await?,
                access_token: token.into(),
            },
            vec![],
        ));
    }
    let stored = read_cookie(headers, &cookie_name(db))
        .and_then(|value| decode_cookie(&value))
        .ok_or_else(|| ApiError::new(401, "Please sign in to continue."))?;
    let token = stored["access_token"].as_str().unwrap_or("");
    match verified_user(db, token).await {
        Ok(user) => Ok((
            Session {
                user,
                access_token: token.into(),
            },
            vec![],
        )),
        Err(error) if error.status.as_u16() == 401 => {
            let refresh = stored["refresh_token"]
                .as_str()
                .filter(|value| !value.is_empty())
                .ok_or(error)?;
            let response = db
                .client
                .post(format!("{}/auth/v1/token?grant_type=refresh_token", db.url))
                .header("apikey", &db.public_key)
                .json(&json!({"refresh_token":refresh}))
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(ApiError::new(401, "Please sign in to continue."));
            }
            let refreshed: Value = response.json().await?;
            let token = refreshed["access_token"]
                .as_str()
                .ok_or_else(ApiError::provider)?;
            let user = verified_user(db, token).await?;
            Ok((
                Session {
                    user,
                    access_token: token.into(),
                },
                session_cookies(db, &refreshed, headers),
            ))
        }
        Err(error) => Err(error),
    }
}
pub async fn membership(db: &Database, user: &Value) -> Result<Value> {
    let user_id = string(user, "id");
    let members = db
        .select("family_members", &[("user_id", eq(user_id))])
        .await;
    match members {
        Ok(rows) if rows.len() > 1 => {
            return Err(ApiError::new(403, "Ambiguous family membership"));
        }
        Ok(rows) if !rows.is_empty() => return Ok(rows[0].clone()),
        Err(error) if !matches!(error.code.as_deref(), Some("42P01" | "PGRST205")) => {
            return Err(error);
        }
        _ => {}
    }
    let metadata = &user["app_metadata"];
    let family = string(metadata, "kin_family_id");
    if family.is_empty() {
        return Ok(Value::Null);
    }
    if metadata["kin_role"] == "wearer" {
        let rows = db
            .select(
                "wearer_accounts",
                &[("user_id", eq(user_id)), ("family_id", eq(family))],
            )
            .await?;
        return Ok(if rows.len() == 1 {
            json!({"user_id":user_id,"family_id":family,"relative_id":null,"role":"loved_one"})
        } else {
            Value::Null
        });
    }
    let contributor = string(metadata, "kin_contributor_id");
    if contributor.is_empty() {
        return Ok(Value::Null);
    }
    let rows = db
        .select(
            "relatives",
            &[("id", eq(contributor)), ("family_id", eq(family))],
        )
        .await?;
    Ok(if rows.len() == 1 {
        json!({"user_id":user_id,"family_id":family,"relative_id":contributor,"role":"contributor"})
    } else {
        Value::Null
    })
}
pub async fn identity(db: &Database, session: &Session) -> Result<Identity> {
    let member = membership(db, &session.user).await?;
    if member.is_null() {
        return Err(ApiError::new(409, "Finish setting up your family first."));
    }
    let family_id = string(&member, "family_id").to_owned();
    if family_id.is_empty() || family_id.len() > 128 {
        return Err(ApiError::new(403, "Invalid family membership"));
    }
    let is_self = member["role"] == "loved_one";
    let filter = if is_self {
        ("is_self", "eq.true".into())
    } else {
        ("id", eq(string(&member, "relative_id")))
    };
    let contributor = match db
        .select("relatives", &[("family_id", eq(&family_id)), filter])
        .await
    {
        Ok(rows) if rows.len() <= 1 => rows.into_iter().next(),
        Err(error)
            if is_self
                && matches!(
                    error.code.as_deref(),
                    Some("42703" | "PGRST204" | "PGRST205")
                ) =>
        {
            None
        }
        Err(error) => return Err(error),
        _ => return Err(ApiError::new(403, "Invalid contributor membership")),
    };
    if !is_self && contributor.is_none() {
        return Err(ApiError::new(403, "Contributor membership required"));
    }
    let families = match db
        .select(
            "families",
            &[("id", eq(&family_id)), ("select", "owner_id".into())],
        )
        .await
    {
        Ok(rows) => rows,
        Err(error) if matches!(error.code.as_deref(), Some("42P01" | "PGRST205")) => vec![],
        Err(error) => return Err(error),
    };
    let owner = families
        .first()
        .and_then(|family| family["owner_id"].as_str());
    let is_admin = !is_self
        && (owner == session.user["id"].as_str()
            || (owner.is_none()
                && session.user["app_metadata"]["kin_family_id"] == family_id
                && session.user["app_metadata"]["kin_admin"] == true));
    Ok(Identity {
        user: session.user.clone(),
        family_id,
        contributor,
        membership: member,
        is_self,
        is_admin,
    })
}
pub async fn session_middleware(
    State(db): State<Database>,
    mut request: Request,
    next: Next,
) -> Response {
    let path = request.uri().path();
    if matches!(
        path,
        "/api/health" | "/health" | "/auth/callback" | "/gate" | "/weaver"
    ) {
        return next.run(request).await;
    }
    let (session, cookies) = match authenticate(&db, request.headers()).await {
        Ok(value) => value,
        Err(error) => return error.into_response(),
    };
    request.extensions_mut().insert(session);
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("private, no-store"),
    );
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            response.headers_mut().append(header::SET_COOKIE, value);
        }
    }
    response
}
pub async fn callback(
    db: &Database,
    headers: &HeaderMap,
    query: &std::collections::HashMap<String, String>,
) -> Result<(String, Vec<String>)> {
    let auth_type = query.get("type").map(String::as_str).unwrap_or("");
    let (path, body) = if let Some(code) = query.get("code") {
        let verifier_name = format!("{}-code-verifier", cookie_name(db));
        let verifier = read_cookie(headers, &verifier_name)
            .and_then(|value| decode_cookie(&value))
            .and_then(|value| value.as_str().map(String::from))
            .unwrap_or_default();
        (
            "/auth/v1/token?grant_type=pkce",
            json!({"auth_code":code,"code_verifier":verifier.split('/').next().unwrap_or("")}),
        )
    } else if let Some(token) = query
        .get("token_hash")
        .filter(|_| matches!(auth_type, "signup" | "recovery" | "email" | "invite"))
    {
        (
            "/auth/v1/verify",
            json!({"token_hash":token,"type":auth_type}),
        )
    } else {
        return Ok(("/signin?error=expired-link".into(), vec![]));
    };
    let session = match db
        .request(Method::POST, path, &[], Some(body), Some(""), "")
        .await
    {
        Ok(value) if value["access_token"].is_string() => value,
        _ => return Ok(("/signin?error=expired-link".into(), vec![])),
    };
    let mut cookies = session_cookies(db, &session, headers);
    cookies.push(format!(
        "{}-code-verifier=; Path=/; Max-Age=0; SameSite=Lax",
        cookie_name(db)
    ));
    let destination = if auth_type == "recovery"
        || query
            .get("next")
            .is_some_and(|value| value == "/update-password")
    {
        "/update-password".into()
    } else if let Some(invite) = query
        .get("invite")
        .filter(|value| uuid::Uuid::parse_str(value).is_ok())
    {
        format!("/onboarding?invite={invite}")
    } else {
        "/onboarding".into()
    };
    Ok((destination, cookies))
}
