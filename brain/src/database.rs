use crate::error::{ApiError, Result};
use reqwest::{Client, Method};
use serde_json::{Value, json};
use std::time::Duration;

#[derive(Clone)]
pub struct Database {
    pub client: Client,
    pub url: String,
    pub public_key: String,
    pub service_key: String,
}

impl Database {
    pub fn from_env() -> Result<Self> {
        let env = |names: &[&str]| {
            names
                .iter()
                .find_map(|name| std::env::var(name).ok().filter(|value| !value.is_empty()))
                .unwrap_or_default()
        };
        Ok(Self {
            client: Client::builder()
                .timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?,
            url: env(&["NEXT_PUBLIC_SUPABASE_URL", "SUPABASE_URL"])
                .trim_end_matches('/')
                .into(),
            public_key: env(&[
                "NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY",
                "NEXT_PUBLIC_SUPABASE_ANON_KEY",
            ]),
            service_key: env(&["SUPABASE_SECRET_KEY", "SUPABASE_SERVICE_ROLE_KEY"]),
        })
    }
    pub fn configured(&self) -> bool {
        !self.url.is_empty() && !self.service_key.is_empty()
    }
    pub async fn request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
        token: Option<&str>,
        preference: &str,
    ) -> Result<Value> {
        if !self.configured() {
            return Err(ApiError::new(503, "Supabase is not configured"));
        }
        let key = if token.is_some() {
            &self.public_key
        } else {
            &self.service_key
        };
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.url))
            .header("apikey", key)
            .bearer_auth(token.unwrap_or(&self.service_key))
            .query(query);
        if !preference.is_empty() {
            request = request.header("prefer", preference);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await?;
        let status = response.status();
        let bytes = response.bytes().await?;
        let value: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if !status.is_success() {
            let code = value["code"].as_str().map(String::from);
            let status = match code.as_deref() {
                Some("23505") => 409,
                Some("42501") => 403,
                Some("P0002") => 404,
                Some("PGRST202" | "42883") => 503,
                _ => 502,
            };
            return Err(ApiError {
                status: axum::http::StatusCode::from_u16(status).unwrap(),
                message: if code.as_deref() == Some("P0001") {
                    value["message"]
                        .as_str()
                        .unwrap_or("Request rejected")
                        .into()
                } else {
                    "Database request failed".into()
                },
                code,
            });
        }
        Ok(value)
    }
    pub async fn select(&self, table: &str, query: &[(&str, String)]) -> Result<Vec<Value>> {
        if query
            .iter()
            .any(|(key, _)| *key == "limit" || *key == "offset")
        {
            let value = self
                .request(
                    Method::GET,
                    &format!("/rest/v1/{table}"),
                    query,
                    None,
                    None,
                    "",
                )
                .await?;
            return value.as_array().cloned().ok_or_else(ApiError::provider);
        }
        let mut rows = Vec::new();
        loop {
            let mut page_query = query.to_vec();
            page_query.push(("limit", "1000".into()));
            page_query.push(("offset", rows.len().to_string()));
            if !page_query.iter().any(|(key, _)| *key == "order") {
                let order = match table {
                    "family_members" | "wearer_accounts" => "user_id.asc",
                    "wearer" => "family_id.asc",
                    _ => "id.asc",
                };
                page_query.push(("order", order.into()));
            }
            let value = self
                .request(
                    Method::GET,
                    &format!("/rest/v1/{table}"),
                    &page_query,
                    None,
                    None,
                    "",
                )
                .await?;
            let page = value.as_array().ok_or_else(ApiError::provider)?;
            let complete = page.len() < 1000;
            rows.extend(page.iter().cloned());
            if complete {
                return Ok(rows);
            }
            if rows.len() >= 100_000 {
                return Err(ApiError::new(
                    503,
                    "Family dataset exceeds the service read limit",
                ));
            }
        }
    }
    pub async fn family_rows(&self, table: &str, family: &str) -> Result<Vec<Value>> {
        self.select(table, &[("select", "*".into()), ("family_id", eq(family))])
            .await
    }
    pub async fn write(
        &self,
        method: Method,
        table: &str,
        query: &[(&str, String)],
        body: Value,
    ) -> Result<Vec<Value>> {
        let value = self
            .request(
                method,
                &format!("/rest/v1/{table}"),
                query,
                Some(body),
                None,
                "return=representation",
            )
            .await?;
        value.as_array().cloned().ok_or_else(ApiError::provider)
    }
    pub async fn rpc(&self, name: &str, args: Value) -> Result<Value> {
        self.request(
            Method::POST,
            &format!("/rest/v1/rpc/{name}"),
            &[],
            Some(args),
            None,
            "",
        )
        .await
    }
    pub async fn delete(&self, table: &str, query: &[(&str, String)]) -> Result<()> {
        self.request(
            Method::DELETE,
            &format!("/rest/v1/{table}"),
            query,
            None,
            None,
            "",
        )
        .await?;
        Ok(())
    }
    pub async fn upload(&self, path: &str, bytes: Vec<u8>, mime: &str) -> Result<()> {
        let response = self
            .client
            .post(format!(
                "{}/storage/v1/object/media/{}",
                self.url,
                encode_path(path)
            ))
            .header("apikey", &self.service_key)
            .bearer_auth(&self.service_key)
            .header("content-type", mime)
            .body(bytes)
            .send()
            .await?;
        if response.status().is_success() || response.status().as_u16() == 409 {
            Ok(())
        } else {
            Err(ApiError::provider())
        }
    }
    pub async fn download(&self, path: &str) -> Result<(Vec<u8>, String)> {
        let response = self
            .client
            .get(format!(
                "{}/storage/v1/object/authenticated/media/{}",
                self.url,
                encode_path(path)
            ))
            .header("apikey", &self.service_key)
            .bearer_auth(&self.service_key)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(ApiError::provider());
        }
        let mime = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .into();
        Ok((response.bytes().await?.to_vec(), mime))
    }
    pub async fn signed_url(&self, path: &str) -> Result<String> {
        let value = self
            .request(
                Method::POST,
                &format!("/storage/v1/object/sign/media/{}", encode_path(path)),
                &[],
                Some(json!({"expiresIn":3600})),
                None,
                "",
            )
            .await?;
        let signed = value["signedURL"]
            .as_str()
            .or(value["signedUrl"].as_str())
            .ok_or_else(ApiError::provider)?;
        Ok(if signed.starts_with("http") {
            signed.into()
        } else {
            format!("{}/storage/v1{signed}", self.url)
        })
    }
    pub async fn remove_media(&self, paths: &[String]) -> Result<()> {
        if paths.is_empty() {
            return Ok(());
        }
        self.request(
            Method::DELETE,
            "/storage/v1/object/media",
            &[],
            Some(json!({"prefixes":paths})),
            None,
            "",
        )
        .await?;
        Ok(())
    }
}
pub fn eq(value: &str) -> String {
    format!("eq.{value}")
}
pub fn in_list(ids: &[String]) -> String {
    format!("in.({})", ids.join(","))
}
pub fn encode_path(path: &str) -> String {
    path.split('/')
        .map(|part| url::form_urlencoded::byte_serialize(part.as_bytes()).collect::<String>())
        .collect::<Vec<_>>()
        .join("/")
}
pub fn string<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key].as_str().unwrap_or("")
}
pub fn one(rows: Vec<Value>) -> Result<Value> {
    if rows.len() == 1 {
        Ok(rows.into_iter().next().unwrap())
    } else {
        Err(ApiError::provider())
    }
}
