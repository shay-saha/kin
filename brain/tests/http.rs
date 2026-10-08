use axum::{
    Json, Router,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::any,
};
use kin_brain::{admin, database::string, faces::NATIVE_MODEL, ingestion::literal_facts};
use reqwest::{
    Client, Method,
    multipart::{Form, Part},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    process::{Child, Command, Stdio},
    sync::{Arc, Mutex},
    time::Duration,
};

const FAMILY: &str = "670f5075-c286-4b29-8074-86401c18d0c0";
const OWNER: &str = "11111111-1111-4111-8111-111111111111";
const OTHER: &str = "22222222-2222-4222-8222-222222222222";
const SELF: &str = "33333333-3333-4333-8333-333333333333";
const SUBJECT: &str = "44444444-4444-4444-8444-444444444444";
const TOPIC: &str = "55555555-5555-4555-8555-555555555555";
const MEMORY: &str = "66666666-6666-4666-8666-666666666666";
const SECOND_MEMORY: &str = "77777777-7777-4777-8777-777777777777";
#[derive(Default)]
struct MockData {
    tables: HashMap<String, Vec<Value>>,
    storage: HashMap<String, (Vec<u8>, String)>,
    requests: Vec<(String, Value)>,
    fail_terminal: bool,
}
type SharedData = Arc<Mutex<MockData>>;
fn user(token: &str) -> Option<Value> {
    let (id, role, relative) = match token {
        "owner" => ("owner-user", "contributor", Some(OWNER)),
        "other" => ("other-user", "contributor", Some(OTHER)),
        "self" => ("self-user", "loved_one", None),
        "new" => ("new-user", "", None),
        _ => return None,
    };
    Some(
        json!({"id":id,"email":format!("{token}@example.invalid"),"user_metadata":{"full_name":token},"app_metadata":{"kin_family_id":if role.is_empty() { Value::Null } else { json!(FAMILY) },"kin_contributor_id":relative,"kin_role":if role=="loved_one" { "wearer" } else { role },"kin_admin":token=="owner"}}),
    )
}
fn matches(row: &Value, query: &HashMap<String, String>, data: &MockData) -> bool {
    query.iter().all(|(key, filter)| {
        if ["select", "order", "limit", "offset"].contains(&key.as_str()) {
            return true;
        }
        let field = if key == "memories.family_id" {
            data.tables
                .get("memories")
                .into_iter()
                .flatten()
                .find(|memory| memory["id"] == row["memory_id"])
                .map(|memory| &memory["family_id"])
                .unwrap_or(&Value::Null)
        } else {
            &row[key]
        };
        let value = field
            .as_str()
            .map(String::from)
            .unwrap_or_else(|| field.to_string());
        if let Some(expected) = filter.strip_prefix("eq.") {
            value == expected
        } else if let Some(expected) = filter.strip_prefix("gt.") {
            value.as_str() > expected
        } else if let Some(list) = filter
            .strip_prefix("in.(")
            .and_then(|list| list.strip_suffix(')'))
        {
            list.split(',').any(|expected| value == expected)
        } else if filter == "is.null" {
            field.is_null()
        } else {
            true
        }
    })
}
async fn mock(State(data): State<SharedData>, request: Request) -> Response {
    let path = request.uri().path().to_owned();
    let method = request.method().clone();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(request.uri().query().unwrap_or("").as_bytes())
            .into_owned()
            .collect();
    let token = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .strip_prefix("Bearer ")
        .unwrap_or("")
        .to_owned();
    let mime = request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_owned();
    let ignore_duplicates = request
        .headers()
        .get("prefer")
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.contains("resolution=ignore-duplicates"));
    let bytes = axum::body::to_bytes(request.into_body(), 20 * 1024 * 1024)
        .await
        .unwrap()
        .to_vec();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    if path == "/auth/v1/user" {
        return match user(&token) {
            Some(user) => Json(user).into_response(),
            None => (
                StatusCode::UNAUTHORIZED,
                Json(json!({"error":"Invalid token"})),
            )
                .into_response(),
        };
    }
    if path == "/auth/v1/token" {
        let refreshed = user("owner").unwrap();
        return Json(json!({"access_token":"owner","refresh_token":"refreshed","expires_in":3600,"user":refreshed})).into_response();
    }
    if path == "/auth/v1/verify" {
        return Json(json!({"access_token":"owner","refresh_token":"refreshed","expires_in":3600,"user":user("owner")})).into_response();
    }
    if path == "/detect" {
        return Json(json!({"model":NATIVE_MODEL,"width":100,"height":100,"faces":[{"box":{"x":0,"y":0,"width":50,"height":50},"descriptor":vec![0.1;128]}]})).into_response();
    }
    if path == "/v1/embeddings" {
        return Json(json!({"data":[{"embedding":vec![0.1;1536]}]})).into_response();
    }
    if path == "/v1/chat/completions" {
        let name = body["response_format"]["json_schema"]["name"]
            .as_str()
            .unwrap_or("");
        let generated = match name {
            "scene_caption" => {
                json!({"caption":"A person in the kitchen","objects":["cake"],"setting":"kitchen"})
            }
            "memory_extraction" => {
                json!({"summary":"Nora shares lemon cake with Rosa on Sundays.","nodes":[{"ref":format!("existing:{SUBJECT}"),"type":"person","label":"Nora","relation_to_wearer":"sister"}],"edges":[]})
            }
            "grounded_cue" => json!({"factIds":["invented"],"cue":"A fabricated sentence"}),
            "weaver_question" => {
                json!({"question":"Do you remember where the lemon cake recipe came from?"})
            }
            _ => Value::Null,
        };
        return Json(json!({"choices":[{"message":{"content":generated.to_string()}}]}))
            .into_response();
    }
    if path == "/listen" {
        return Json(json!({"results":{"channels":[{"alternatives":[{"transcript":"Nora shares lemon cake with Rosa on Sundays."}]}],"utterances":[{"start":0,"end":2,"transcript":"Nora shares lemon cake with Rosa on Sundays."}]}})).into_response();
    }
    if path.starts_with("/v1/text-to-speech/") {
        return ([("content-type", "audio/mpeg")], vec![1u8; 1024]).into_response();
    }
    let mut data = data.lock().unwrap();
    data.requests.push((path.clone(), body.clone()));
    if path.starts_with("/rest/v1/rpc/") {
        let rpc = path.trim_start_matches("/rest/v1/rpc/");
        if rpc == "match_subject_memories" {
            let contributor = string(&body, "contributor");
            let rows: Vec<_> = data
                .tables
                .get("memories")
                .into_iter()
                .flatten()
                .filter(|memory| memory["contributor_id"] == contributor)
                .map(|memory| json!({"id":memory["id"],"similarity":0.6}))
                .collect();
            return Json(json!(rows)).into_response();
        }
        if rpc == "commit_ingestion" || rpc == "capture_self_contribution" {
            let payload = &body["payload"];
            if rpc == "capture_self_contribution" {
                let closed = data
                    .tables
                    .get("relatives")
                    .into_iter()
                    .flatten()
                    .any(|relative| {
                        relative["id"] == SELF && relative["self_capture_open"] == false
                    });
                if closed {
                    data.tables.entry("pending_contributions".into()).or_default().push(json!({"id":payload["id"],"family_id":FAMILY,"contributor_id":SELF,"state":"pending","payload":payload,"preview":body["preview"],"kind":payload["memory"]["kind"]}));
                    let mut response = payload["response"].clone();
                    response["pending_review"] = json!(true);
                    return Json(response).into_response();
                }
            }
            if payload["memory"].is_object() {
                data.tables
                    .entry("memories".into())
                    .or_default()
                    .push(payload["memory"].clone());
            }
            if payload["face"].is_object() {
                data.tables
                    .entry("face_embeddings".into())
                    .or_default()
                    .push(payload["face"].clone());
            }
            data.tables.entry("ingestion_receipts".into()).or_default().push(json!({"id":payload["id"],"family_id":payload["family_id"],"contributor_id":payload["contributor_id"],"request_hash":payload["request_hash"],"response":payload["response"]}));
            data.tables.entry("provenance".into()).or_default().extend(
                payload["provenance"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            );
            return Json(payload["response"].clone()).into_response();
        }
        if rpc == "review_self_contribution" {
            let pending = data
                .tables
                .entry("pending_contributions".into())
                .or_default();
            let Some(row) = pending
                .iter_mut()
                .find(|row| row["id"] == body["contribution"])
            else {
                return (StatusCode::NOT_FOUND, Json(json!({"code":"P0002"}))).into_response();
            };
            row["state"] = json!(if body["decision"] == "approve" {
                "approved"
            } else {
                "rejected"
            });
            return Json(json!({"ok":true})).into_response();
        }
        if rpc == "create_kin_family" {
            data.tables.entry("family_members".into()).or_default().push(json!({"user_id":"new-user","family_id":FAMILY,"relative_id":OWNER,"role":"contributor"}));
            return Json(json!(FAMILY)).into_response();
        }
        return Json(json!([])).into_response();
    }
    if let Some(table) = path.strip_prefix("/rest/v1/") {
        let selected: Vec<_> = data
            .tables
            .get(table)
            .into_iter()
            .flatten()
            .filter(|row| matches(row, &query, &data))
            .cloned()
            .collect();
        let mut selected = selected;
        if method == axum::http::Method::POST {
            selected = body
                .as_array()
                .cloned()
                .unwrap_or_else(|| vec![body.clone()]);
            if ignore_duplicates {
                let unique_key = if table == "wearer" { "family_id" } else { "id" };
                selected.retain(|row| {
                    !data
                        .tables
                        .get(table)
                        .into_iter()
                        .flatten()
                        .any(|existing| {
                            !row[unique_key].is_null() && existing[unique_key] == row[unique_key]
                        })
                });
            }
            for row in &mut selected {
                if table == "weaver_questions" && row["status"].is_null() {
                    row["status"] = json!("open");
                }
                if row["id"].is_null() {
                    row["id"] = json!(uuid::Uuid::new_v4());
                }
                if row["created_at"].is_null() {
                    row["created_at"] = json!("2026-10-08T00:00:00Z");
                }
                if table == "family_invites" {
                    row["token"] = json!(uuid::Uuid::new_v4());
                    row["expires_at"] = json!("2027-01-01T00:00:00Z");
                }
            }
            data.tables
                .entry(table.into())
                .or_default()
                .extend(selected.clone());
        }
        if method == axum::http::Method::PATCH {
            if table == "recall_events" && data.fail_terminal && body["status"].is_string() {
                return (
                    StatusCode::SERVICE_UNAVAILABLE,
                    Json(json!({"error":"offline"})),
                )
                    .into_response();
            }
            for row in data.tables.entry(table.into()).or_default().iter_mut() {
                if selected.iter().any(|selected| selected["id"] == row["id"]) {
                    for (key, value) in body.as_object().unwrap() {
                        row[key] = value.clone();
                    }
                }
            }
            for row in &mut selected {
                for (key, value) in body.as_object().unwrap() {
                    row[key] = value.clone();
                }
            }
        }
        if method == axum::http::Method::DELETE {
            data.tables
                .entry(table.into())
                .or_default()
                .retain(|row| !selected.iter().any(|selected| selected["id"] == row["id"]));
        }
        if let Some(order) = query.get("order") {
            let (field, direction) = order.split_once('.').unwrap_or((order, "asc"));
            selected.sort_by(|a, b| a[field].to_string().cmp(&b[field].to_string()));
            if direction == "desc" {
                selected.reverse();
            }
        }
        let offset = query
            .get("offset")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(0);
        let limit = query
            .get("limit")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(1000);
        selected = selected.into_iter().skip(offset).take(limit).collect();
        if let Some(columns) = query
            .get("select")
            .filter(|columns| columns.as_str() != "*" && !columns.contains('('))
        {
            selected = selected
                .into_iter()
                .map(|row| {
                    Value::Object(
                        columns
                            .split(',')
                            .map(|key| (key.into(), row[key].clone()))
                            .collect(),
                    )
                })
                .collect();
        }
        return Json(json!(selected)).into_response();
    }
    if let Some(path) = path.strip_prefix("/storage/v1/object/authenticated/media/") {
        return data
            .storage
            .get(path)
            .map(|(bytes, mime)| ([("content-type", mime.clone())], bytes.clone()).into_response())
            .unwrap_or_else(|| StatusCode::NOT_FOUND.into_response());
    }
    if let Some(path) = path.strip_prefix("/storage/v1/object/media/") {
        data.storage.insert(path.into(), (bytes, mime));
        return Json(json!({"Key":path})).into_response();
    }
    if path == "/storage/v1/object/media" {
        for path in body["prefixes"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            data.storage.remove(path);
        }
        return Json(json!([])).into_response();
    }
    if path == "/storage/v1/object/list/media" {
        return Json(json!([])).into_response();
    }
    if path.starts_with("/storage/v1/object/sign/media/") {
        return Json(json!({"signedURL":"/object/sign/media/test?token=private"})).into_response();
    }
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error":"Unknown mock endpoint"})),
    )
        .into_response()
}
struct Harness {
    child: Child,
    server: tokio::task::JoinHandle<()>,
    directory: std::path::PathBuf,
    client: Client,
    endpoint: String,
    data: SharedData,
}
impl Drop for Harness {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.server.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl Harness {
    async fn new() -> Self {
        let dataset = admin::demo_dataset();
        let mut data = MockData::default();
        for (table, key) in [("graph_nodes", "nodes"), ("graph_edges", "edges")] {
            data.tables
                .insert(table.into(), dataset[key].as_array().unwrap().clone());
        }
        data.tables.insert(
            "wearer".into(),
            vec![json!({"family_id":FAMILY,"name":"Rosa"})],
        );
        data.tables.insert(
            "families".into(),
            vec![json!({"id":FAMILY,"owner_id":"owner-user"})],
        );
        data.tables.insert("family_members".into(),vec![json!({"user_id":"owner-user","family_id":FAMILY,"relative_id":OWNER,"role":"contributor"}),json!({"user_id":"other-user","family_id":FAMILY,"relative_id":OTHER,"role":"contributor"}),json!({"user_id":"self-user","family_id":FAMILY,"relative_id":null,"role":"loved_one"})]);
        data.tables.insert("relatives".into(),vec![json!({"id":OWNER,"family_id":FAMILY,"name":"Maya","relation_to_wearer":"granddaughter","color":"#fff"}),json!({"id":OTHER,"family_id":FAMILY,"name":"Elena","relation_to_wearer":"daughter","color":"#fff"}),json!({"id":SELF,"family_id":FAMILY,"name":"Rosa","relation_to_wearer":"self","color":"#fff","is_self":true,"self_capture_open":false})]);
        let nodes = data.tables.get_mut("graph_nodes").unwrap();
        nodes.push(json!({"id":SUBJECT,"family_id":FAMILY,"type":"person","label":"Nora","aliases":[],"relation_to_wearer":"sister"}));
        nodes.push(json!({"id":TOPIC,"family_id":FAMILY,"type":"tradition","label":"lemon cake","aliases":[],"relation_to_wearer":null}));
        let subject = nodes
            .iter()
            .find(|node| node["id"] == SUBJECT)
            .unwrap()
            .clone();
        for (id, contributor, text) in [
            (
                MEMORY,
                OWNER,
                "Nora shares lemon cake with Rosa on Sundays.",
            ),
            (
                SECOND_MEMORY,
                OTHER,
                "Nora wears the yellow apron when she bakes.",
            ),
        ] {
            data.tables.entry("memories".into()).or_default().push(json!({"id":id,"family_id":FAMILY,"contributor_id":contributor,"kind":"story","transcript":text,"summary":text,"source":{"type":"human"},"verified_facts":literal_facts(text,std::slice::from_ref(&subject),id,contributor),"created_at":"2026-10-08T00:00:00Z"}));
            data.tables.entry("provenance".into()).or_default().push(json!({"id":uuid::Uuid::new_v4(),"memory_id":id,"contributor_id":contributor,"node_id":SUBJECT,"edge_id":null}));
        }
        data.tables.insert("face_embeddings".into(),vec![json!({"id":uuid::Uuid::new_v4(),"family_id":FAMILY,"person_node_id":SUBJECT,"descriptor":vec![0.1;128],"model":NATIVE_MODEL,"contributor_id":OWNER,"memory_id":MEMORY})]);
        let data = Arc::new(Mutex::new(data));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mock_url = format!("http://{}", listener.local_addr().unwrap());
        let app = Router::new().fallback(any(mock)).with_state(data.clone());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = reserved.local_addr().unwrap().port();
        drop(reserved);
        let directory =
            std::env::temp_dir().join(format!("kin-backend-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&directory).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_kin-brain"))
            .current_dir(&directory)
            .env_clear()
            .env("KIN_BRAIN_PORT", port.to_string())
            .env("NEXT_PUBLIC_SUPABASE_URL", &mock_url)
            .env("NEXT_PUBLIC_SUPABASE_PUBLISHABLE_KEY", "offline-public")
            .env("SUPABASE_SECRET_KEY", "offline-service")
            .env("OPENAI_API_KEY", "offline-openai")
            .env("OPENAI_BASE_URL", format!("{mock_url}/v1"))
            .env("DEEPGRAM_API_KEY", "offline-deepgram")
            .env("DEEPGRAM_LISTEN_URL", format!("{mock_url}/listen"))
            .env("ELEVENLABS_API_KEY", "offline-elevenlabs")
            .env("ELEVENLABS_VOICE_ID", "offline-voice")
            .env("ELEVENLABS_BASE_URL", format!("{mock_url}/v1"))
            .env("KIN_FACE_SERVICE_URL", format!("{mock_url}/detect"))
            .env("KIN_FACE_SERVICE_TOKEN", "offline-face")
            .env("KIN_FACE_TOKEN_KEY", "ab".repeat(32))
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let harness = Self {
            child,
            server,
            directory,
            client: Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            endpoint: format!("http://127.0.0.1:{port}"),
            data,
        };
        for _ in 0..200 {
            if harness
                .client
                .get(format!("{}/health", harness.endpoint))
                .send()
                .await
                .is_ok()
            {
                return harness;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("backend did not start");
    }
    async fn request(
        &self,
        method: Method,
        path: &str,
        token: &str,
        body: Option<Value>,
    ) -> (u16, Value) {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.endpoint));
        if !token.is_empty() {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        let body = response.json().await.unwrap();
        (status, body)
    }
    async fn photo(
        &self,
        token: &str,
        contributor: &str,
        caption: &str,
        labels: &str,
        consent: bool,
        key: &str,
    ) -> (u16, Value) {
        let file = Part::bytes(vec![255, 216, 255, 224, 1, 2, 3, 4])
            .file_name("photo.jpg")
            .mime_str("image/jpeg")
            .unwrap();
        let form = Form::new()
            .part("file", file)
            .text("contributor_id", contributor.to_owned())
            .text("caption", caption.to_owned())
            .text("labels", labels.to_owned())
            .text("consent", consent.to_string());
        let response = self
            .client
            .post(format!("{}/api/memories/photo", self.endpoint))
            .bearer_auth(token)
            .header("idempotency-key", key)
            .multipart(form)
            .send()
            .await
            .unwrap();
        (response.status().as_u16(), response.json().await.unwrap())
    }
}
#[tokio::test]
async fn verifies_authentication_membership_and_ownership() {
    let h = Harness::new().await;
    for token in ["", "invalid"] {
        assert_eq!(
            h.request(Method::GET, "/api/family", token, None).await.0,
            401
        );
    }
    let (status, account) = h.request(Method::GET, "/api/account", "owner", None).await;
    assert_eq!(status, 200);
    assert_eq!(account["membership"]["family_id"], FAMILY);
    assert_eq!(
        h.request(Method::GET, "/api/family", "new", None).await.0,
        409
    );
    for (token, owner) in [("owner", true), ("other", false), ("self", false)] {
        let (status, family) = h.request(Method::GET, "/api/family", token, None).await;
        assert_eq!(status, 200);
        assert_eq!(family["isOwner"], owner);
        assert!(family["faces"][0].get("descriptor").is_none());
    }
    assert_eq!(
        h.request(
            Method::DELETE,
            &format!("/api/memories/{MEMORY}?contributor_id={OWNER}"),
            "other",
            None
        )
        .await
        .0,
        403
    );
    assert_eq!(
        h.request(Method::POST, "/api/invite", "other", None)
            .await
            .0,
        403
    );
    assert_eq!(
        h.request(Method::POST, "/api/admin/reset", "self", None)
            .await
            .0,
        403
    );
    let (_, session) = h
        .request(Method::GET, "/api/session?path=%2Ffamily", "self", None)
        .await;
    assert_eq!(session["destination"], "/wearer");
    let response = h
        .client
        .get(format!("{}/api/account", h.endpoint))
        .header(
            "cookie",
            format!(
                "sb-127-auth-token={}",
                json!({"access_token":"expired","refresh_token":"refresh"})
            ),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(response.headers().get_all("set-cookie").iter().count() > 0);
    let (status,_)=h.request(Method::POST,"/api/onboarding","new",Some(json!({"mode":"create","name":"Maya","relationship":"granddaughter","lovedOne":"Rosa"}))).await;
    assert_eq!(status, 200);
}
#[tokio::test]
async fn ingests_idempotently_and_rejects_untrusted_input() {
    let h = Harness::new().await;
    let labels = json!([{"person_node_id":SUBJECT}]).to_string();
    assert_eq!(
        h.photo(
            "owner",
            OTHER,
            "Nora shares cake on Sundays.",
            &labels,
            true,
            "wrong-owner"
        )
        .await
        .0,
        403
    );
    assert_eq!(
        h.photo(
            "owner",
            OWNER,
            "Nora shares cake on Sundays.",
            &labels,
            false,
            "no-consent"
        )
        .await
        .0,
        400
    );
    assert_eq!(
        h.photo(
            "owner",
            OWNER,
            "Nora shares cake on Sundays.",
            &json!([{"person_node_id":SUBJECT,"descriptor":vec![0.1;128]}]).to_string(),
            true,
            "untrusted-descriptor"
        )
        .await
        .0,
        400
    );
    let (status, first) = h
        .photo(
            "owner",
            OWNER,
            "Nora shares cake on Sundays.",
            &labels,
            true,
            "photo-retry",
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(first["persons"][0]["node_id"], SUBJECT);
    let (status, retried) = h
        .photo(
            "owner",
            OWNER,
            "Nora shares cake on Sundays.",
            &labels,
            true,
            "photo-retry",
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(retried, first);
    assert_eq!(
        h.photo(
            "owner",
            OWNER,
            "Nora has a different story.",
            &labels,
            true,
            "photo-retry"
        )
        .await
        .0,
        409
    );
    assert_eq!(h.data.lock().unwrap().tables["ingestion_receipts"].len(), 1);
    let facts = h.data.lock().unwrap().tables["memories"].last().unwrap()["verified_facts"].clone();
    assert_eq!(facts[0]["text"], "Nora shares cake on Sundays.");
    assert!(!facts.to_string().contains("kitchen"));
}
#[tokio::test]
async fn seals_face_enrollment_and_rejects_cross_scope_tokens() {
    let h = Harness::new().await;
    let image = vec![255, 216, 255, 224, 1, 2, 3, 4];
    let form = Form::new().part(
        "file",
        Part::bytes(image.clone()).mime_str("image/jpeg").unwrap(),
    );
    let response = h
        .client
        .post(format!("{}/api/faces/detect", h.endpoint))
        .bearer_auth("owner")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let detected: Value = response.json().await.unwrap();
    assert!(detected["faces"][0].get("descriptor").is_none());
    let (_, memory) = h
        .photo(
            "owner",
            OWNER,
            "Nora shares cake on Sundays.",
            &json!([{"person_node_id":SUBJECT}]).to_string(),
            true,
            "enrollment",
        )
        .await;
    let body = json!({"person_node_id":SUBJECT,"contributor_id":OWNER,"memory_id":memory["memory_id"],"temporaryFaceId":detected["faces"][0]["temporaryFaceId"],"consent":true});
    let (status, enrolled) = h
        .request(
            Method::POST,
            "/api/faces/enroll",
            "owner",
            Some(body.clone()),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(enrolled["model"], NATIVE_MODEL);
    assert_eq!(
        h.request(
            Method::POST,
            "/api/faces/enroll",
            "owner",
            Some(body.clone())
        )
        .await
        .1,
        enrolled
    );
    let mut tampered = body.clone();
    tampered["temporaryFaceId"] = json!("tampered");
    assert_eq!(
        h.request(Method::POST, "/api/faces/enroll", "owner", Some(tampered))
            .await
            .0,
        422
    );
    let mut foreign = body;
    foreign["contributor_id"] = json!(OTHER);
    assert_eq!(
        h.request(Method::POST, "/api/faces/enroll", "other", Some(foreign))
            .await
            .0,
        403
    );
}
#[tokio::test]
async fn holds_self_recordings_for_review_and_validates_segments() {
    let h = Harness::new().await;
    let form = Form::new()
        .part(
            "file",
            Part::bytes(vec![26, 69, 223, 163, 1, 2])
                .mime_str("audio/webm")
                .unwrap(),
        )
        .text("contributor_id", SELF)
        .text("topic_id", TOPIC);
    let response = h
        .client
        .post(format!("{}/api/memories/story", h.endpoint))
        .bearer_auth("self")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let captured: Value = response.json().await.unwrap();
    assert_eq!(captured["pending_review"], true);
    assert_eq!(h.data.lock().unwrap().tables["memories"].len(), 2);
    assert_eq!(
        h.request(Method::GET, "/api/self/review", "self", None)
            .await
            .0,
        403
    );
    let (status, pending) = h
        .request(Method::GET, "/api/self/review", "owner", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(pending["pending"].as_array().unwrap().len(), 1);
    assert!(pending["pending"][0].get("payload").is_none());
    assert_eq!(
        h.request(
            Method::POST,
            "/api/self/review",
            "owner",
            Some(json!({"id":captured["memory_id"],"action":"approve"}))
        )
        .await
        .0,
        200
    );
    let (status, window) = h
        .request(
            Method::PATCH,
            "/api/self/window",
            "other",
            Some(json!({"captureOpen":true})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(window["captureOpen"], true);
    assert_eq!(
        h.request(
            Method::PATCH,
            "/api/self/window",
            "self",
            Some(json!({"captureOpen":false}))
        )
        .await
        .0,
        403
    );
    h.data.lock().unwrap().tables.get_mut("memories").unwrap()[0]["source"]["audio_segments"] =
        json!([{"start":0,"end":2,"text":"Nora shares lemon cake with Rosa on Sundays."}]);
    let (status, media) = h
        .request(Method::GET, &format!("/api/stories/{MEMORY}"), "self", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(media["segments"].as_array().unwrap().len(), 1);
    assert_eq!(
        h.request(Method::GET, "/api/stories/not-a-uuid", "owner", None)
            .await
            .0,
        400
    );
}
#[tokio::test]
async fn recalls_only_grounded_independent_evidence_and_invalidates_deleted_sources() {
    let h = Harness::new().await;
    let form = Form::new().part(
        "snapshot",
        Part::bytes(vec![255, 216, 255, 224, 1, 2, 3, 4])
            .mime_str("image/jpeg")
            .unwrap(),
    );
    let response = h
        .client
        .post(format!("{}/api/recall", h.endpoint))
        .bearer_auth("self")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let recalled: Value = response.json().await.unwrap();
    assert_eq!(recalled["decision"], "speak");
    assert_eq!(
        recalled["cueText"],
        "A relative said: “Nora shares lemon cake with Rosa on Sundays.”"
    );
    assert_eq!(
        recalled["scores"]["agreeingKeeperIds"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let event = recalled["eventId"].as_str().unwrap();
    let (status, replay) = h
        .request(
            Method::POST,
            "/api/recall",
            "self",
            Some(json!({"replayEventId":event})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(replay["decision"], "speak");
    let speech = h
        .client
        .post(format!("{}/api/tts", h.endpoint))
        .bearer_auth("self")
        .json(&json!({"eventId":event}))
        .send()
        .await
        .unwrap();
    assert_eq!(speech.status(), 200);
    assert_eq!(speech.headers()["content-type"], "audio/mpeg");
    let (status, _) = h
        .request(
            Method::DELETE,
            &format!("/api/memories/{MEMORY}?contributor_id={OWNER}"),
            "owner",
            None,
        )
        .await;
    assert_eq!(status, 200);
    {
        let data = h.data.lock().unwrap();
        assert!(
            data.tables["memories"]
                .iter()
                .all(|memory| memory["id"] != MEMORY)
        );
        assert!(
            data.tables["recall_events"]
                .iter()
                .all(|event| event["status"] == "silent")
        );
        assert!(
            data.tables["graph_nodes"]
                .iter()
                .any(|node| node["id"] == SUBJECT)
        );
    }
    assert_eq!(
        h.request(
            Method::POST,
            "/api/tts",
            "self",
            Some(json!({"eventId":event}))
        )
        .await
        .0,
        404
    );
}
#[tokio::test]
async fn fails_closed_when_terminal_recall_persistence_fails() {
    let h = Harness::new().await;
    h.data.lock().unwrap().fail_terminal = true;
    let form = Form::new().part(
        "snapshot",
        Part::bytes(vec![255, 216, 255, 224, 1, 2, 3, 4])
            .mime_str("image/jpeg")
            .unwrap(),
    );
    let response = h
        .client
        .post(format!("{}/api/recall", h.endpoint))
        .bearer_auth("self")
        .multipart(form)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    let body: Value = response.json().await.unwrap();
    assert!(body["error"].as_str().unwrap().contains("terminal state"));
    assert!(body.get("cueText").is_none());
    assert_eq!(
        h.data.lock().unwrap().tables["recall_events"][0]["status"],
        "running"
    );
}

#[tokio::test]
async fn creates_topic_weaver_questions_once_and_excludes_self_routing() {
    let h = Harness::new().await;
    {
        let mut data = h.data.lock().unwrap();
        let wearer = data.tables["graph_nodes"]
            .iter()
            .find(|node| node["relation_to_wearer"] == "self")
            .unwrap()["id"]
            .clone();
        data.tables.get_mut("graph_edges").unwrap().extend([json!({"id":uuid::Uuid::new_v4(),"family_id":FAMILY,"from_node":SUBJECT,"to_node":TOPIC,"rel":"participates_in"}),json!({"id":uuid::Uuid::new_v4(),"family_id":FAMILY,"from_node":wearer,"to_node":TOPIC,"rel":"participates_in"})]);
        data.tables.get_mut("provenance").unwrap().push(json!({"id":uuid::Uuid::new_v4(),"memory_id":MEMORY,"node_id":TOPIC,"edge_id":null,"contributor_id":OWNER}));
    }
    let (status, created) = h
        .request(
            Method::POST,
            "/api/weaver/run",
            "owner",
            Some(json!({"topicId":TOPIC})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(created["reason"], "created");
    assert_eq!(created["question"]["target_relative_id"], OTHER);
    assert_eq!(created["question"]["gap_node_id"], TOPIC);
    assert!(
        created["question"]["evidence"]
            .as_array()
            .unwrap()
            .iter()
            .any(|source| source["contributor_id"] == OTHER)
    );
    let (status, repeated) = h
        .request(
            Method::POST,
            "/api/weaver/run",
            "owner",
            Some(json!({"topicId":TOPIC})),
        )
        .await;
    assert_eq!(status, 200);
    assert_eq!(repeated["reason"], "existing_open");
    assert_eq!(repeated["question"]["id"], created["question"]["id"]);
    assert_eq!(h.data.lock().unwrap().tables["weaver_questions"].len(), 1);
}
#[tokio::test]
async fn supports_owner_demo_administration_and_preserves_memberships() {
    let h = Harness::new().await;
    assert_eq!(
        h.request(Method::POST, "/api/admin/seed", "other", None)
            .await
            .0,
        403
    );
    let (status, seeded) = h
        .request(Method::POST, "/api/admin/seed", "owner", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(seeded["ok"], true);
    assert!(seeded["addedMemoryCount"].as_u64().unwrap() > 0);
    let (status, repeated) = h
        .request(Method::POST, "/api/admin/seed", "owner", None)
        .await;
    assert_eq!(status, 200);
    assert_eq!(repeated["addedMemoryCount"], 0);
    assert_eq!(
        h.request(Method::POST, "/api/admin/reset", "owner", None)
            .await
            .0,
        200
    );
    let data = h.data.lock().unwrap();
    assert!(data.tables["memories"].is_empty());
    assert!(data.tables["graph_nodes"].is_empty());
    assert!(
        data.tables["relatives"]
            .iter()
            .any(|relative| relative["id"] == OWNER)
    );
    assert_eq!(data.tables["family_members"].len(), 3);
    assert_eq!(data.tables["wearer"].len(), 1);
}
#[tokio::test]
async fn exchanges_auth_callbacks_without_allowing_external_redirects() {
    let h = Harness::new().await;
    let response = h
        .client
        .get(format!(
            "{}/auth/callback?token_hash=verification&type=signup&next=https://example.invalid",
            h.endpoint
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 303);
    assert_eq!(response.headers()["location"], "/onboarding");
    assert!(response.headers().get_all("set-cookie").iter().count() > 0);
    let recovery = h
        .client
        .get(format!(
            "{}/auth/callback?token_hash=verification&type=recovery",
            h.endpoint
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(recovery.headers()["location"], "/update-password");
    let malformed = h
        .client
        .get(format!(
            "{}/auth/callback?next=https://example.invalid",
            h.endpoint
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(
        malformed.headers()["location"],
        "/signin?error=expired-link"
    );
}
