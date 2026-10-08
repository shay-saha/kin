use crate::{
    database::{Database, eq, in_list, string},
    error::{ApiError, Result},
    ingestion::{digest, stable_id},
    providers,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Method;
use serde_json::{Value, json};
use std::collections::HashSet;

pub const DEMO_FAMILY: &str = "670f5075-c286-4b29-8074-86401c18d0c0";
pub fn demo_dataset() -> Value {
    serde_json::from_str(include_str!("../fixtures/demo.json")).expect("valid bundled demo dataset")
}
async fn insert_missing(db: &Database, table: &str, rows: Value) -> Result<()> {
    if rows.as_array().is_some_and(Vec::is_empty) {
        return Ok(());
    }
    db.request(
        Method::POST,
        &format!("/rest/v1/{table}"),
        &[],
        Some(rows),
        None,
        "resolution=ignore-duplicates",
    )
    .await?;
    Ok(())
}
pub async fn seed(db: &Database, family: &str) -> Result<Value> {
    if family != DEMO_FAMILY {
        return Err(ApiError::new(
            403,
            "Seed requires the canonical shared demo family",
        ));
    }
    if !db
        .select("wearer", &[("family_id", eq("demo"))])
        .await?
        .is_empty()
    {
        return Err(ApiError::new(
            409,
            "Apply consolidation migration 005 before seeding",
        ));
    }
    let dataset = demo_dataset();
    let existing = db.family_rows("memories", family).await?;
    let present: HashSet<_> = existing.iter().map(|row| string(row, "id")).collect();
    let has_key = std::env::var("OPENAI_API_KEY").is_ok_and(|value| !value.is_empty());
    let mut memories = vec![];
    for mut memory in dataset["memories"]
        .as_array()
        .ok_or_else(ApiError::provider)?
        .iter()
        .filter(|memory| !present.contains(string(memory, "id")))
        .cloned()
    {
        memory["embedding"] = if has_key {
            json!(providers::embedding(db, string(&memory, "summary")).await?)
        } else {
            json!(vec![0.0; 1536])
        };
        memory["source"]["embedding_status"] = json!(if has_key { "ready" } else { "missing_key" });
        memories.push(memory);
    }
    insert_missing(db, "wearer", json!([{"family_id":family,"name":"Rosa"}])).await?;
    for table in ["relatives", "graph_nodes", "graph_edges"] {
        let key = match table {
            "graph_nodes" => "nodes",
            "graph_edges" => "edges",
            _ => table,
        };
        insert_missing(db, table, dataset[key].clone()).await?;
    }
    insert_missing(db, "memories", json!(memories)).await?;
    insert_missing(db, "provenance", dataset["provenance"].clone()).await?;
    let nodes = dataset["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|node| (string(node, "label").into(), node["id"].clone()))
        .collect::<serde_json::Map<_, _>>();
    Ok(json!({
        "relatives": dataset["ids"],
        "nodes": nodes,
        "memoryCount": dataset["memories"].as_array().unwrap().len(),
        "addedMemoryCount": memories.len(),
        "embeddingStatus": if has_key {
            "ready"
        } else {
            "missing_key"
        }
    }))
}
pub async fn reset(db: &Database, family: &str) -> Result<()> {
    if family.is_empty() || family.contains('/') || family.contains("..") {
        return Err(ApiError::new(400, "Invalid family storage prefix"));
    }
    let mut directories = vec![family.to_owned()];
    let mut paths = vec![];
    while let Some(prefix) = directories.pop() {
        let mut offset = 0;
        loop {
            let entries = db
                .request(
                    Method::POST,
                    "/storage/v1/object/list/media",
                    &[],
                    Some(json!({
                        "prefix": prefix,
                        "limit": 100,
                        "offset": offset,
                        "sortBy": {"column": "name", "order": "asc"}
                    })),
                    None,
                    "",
                )
                .await?;
            let entries = entries.as_array().ok_or_else(ApiError::provider)?;
            for entry in entries {
                let name = string(entry, "name");
                if name.is_empty() || name.contains('/') || name == ".." {
                    return Err(ApiError::provider());
                }
                let path = format!("{prefix}/{name}");
                if entry["id"].is_null() {
                    directories.push(path);
                } else {
                    paths.push(path);
                }
            }
            if entries.len() < 100 {
                break;
            }
            offset += 100;
        }
    }
    for paths in paths.chunks(100) {
        db.remove_media(paths).await?;
    }
    if let Err(error) = db
        .delete("pending_contributions", &[("family_id", eq(family))])
        .await
        && !matches!(error.code.as_deref(), Some("42P01" | "PGRST205"))
    {
        return Err(error);
    }
    if let Err(error) = db
        .write(
            Method::PATCH,
            "relatives",
            &[("family_id", eq(family)), ("is_self", "eq.true".into())],
            json!({"self_capture_open":true}),
        )
        .await
        && !matches!(error.code.as_deref(), Some("42703" | "PGRST204"))
    {
        return Err(error);
    }
    for table in [
        "weaver_questions",
        "recall_events",
        "ingestion_receipts",
        "face_embeddings",
        "memories",
        "graph_edges",
        "graph_nodes",
    ] {
        db.delete(table, &[("family_id", eq(family))]).await?;
    }
    Ok(())
}
pub async fn delete_memory(
    db: &Database,
    family: &str,
    contributor: &str,
    memory: &str,
) -> Result<bool> {
    let found = db
        .select(
            "memories",
            &[
                ("family_id", eq(family)),
                ("contributor_id", eq(contributor)),
                ("id", eq(memory)),
            ],
        )
        .await?;
    let Some(found) = found.first() else {
        return Ok(false);
    };
    let (sources, nodes, edges, faces, questions, events) = tokio::try_join!(
        async {
            db.select(
                "provenance",
                &[
                    (
                        "select",
                        "memory_id,node_id,edge_id,memories!inner(family_id)".into(),
                    ),
                    ("memories.family_id", eq(family)),
                ],
            )
            .await
        },
        async { db.family_rows("graph_nodes", family).await },
        async { db.family_rows("graph_edges", family).await },
        async { db.family_rows("face_embeddings", family).await },
        async { db.family_rows("weaver_questions", family).await },
        async { db.family_rows("recall_events", family).await }
    )?;
    let own: Vec<_> = sources
        .iter()
        .filter(|source| source["memory_id"] == memory)
        .collect();
    let remaining: Vec<_> = sources
        .iter()
        .filter(|source| source["memory_id"] != memory)
        .collect();
    let edge_ids: Vec<String> = edges
        .iter()
        .filter(|edge| {
            own.iter().any(|source| source["edge_id"] == edge["id"])
                && !remaining
                    .iter()
                    .any(|source| source["edge_id"] == edge["id"])
        })
        .map(|edge| string(edge, "id").into())
        .collect();
    let retained: Vec<_> = edges
        .iter()
        .filter(|edge| !edge_ids.iter().any(|id| edge["id"] == *id))
        .collect();
    let touches = |edge: &Value, node: &Value| {
        edge["from_node"] == node["id"] || edge["to_node"] == node["id"]
    };
    let node_ids: Vec<String> = nodes
        .iter()
        .filter(|node| {
            node["relation_to_wearer"] != "self"
                && (own.iter().any(|source| source["node_id"] == node["id"])
                    || edges.iter().any(|edge| {
                        edge_ids.iter().any(|id| edge["id"] == *id) && touches(edge, node)
                    }))
                && !remaining
                    .iter()
                    .any(|source| source["node_id"] == node["id"])
                && !retained.iter().any(|edge| touches(edge, node))
                && !faces
                    .iter()
                    .any(|face| face["person_node_id"] == node["id"] && face["memory_id"] != memory)
        })
        .map(|node| string(node, "id").into())
        .collect();
    let question_ids: Vec<String> = questions
        .iter()
        .filter(|question| {
            question["answer_memory_id"] == memory
                || node_ids.iter().any(|id| question["gap_node_id"] == *id)
                || question["evidence"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|source| source["memory_id"] == memory)
        })
        .map(|question| string(question, "id").into())
        .collect();
    let event_ids: Vec<String> = events
        .iter()
        .filter(|event| {
            event["evidence"]
                .as_array()
                .into_iter()
                .flatten()
                .any(|source| source["memoryId"] == memory)
                || event["gate"]["citedMemoryIds"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|id| id == memory)
                || event["keeper_results"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .any(|keeper| {
                        keeper["memoryIds"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .any(|id| id == memory)
                    })
        })
        .map(|event| string(event, "id").into())
        .collect();
    if let Some(path) = found["media_path"].as_str() {
        db.remove_media(&[path.into()]).await?;
    }
    if !event_ids.is_empty() {
        db.write(
            Method::PATCH,
            "recall_events",
            &[("family_id", eq(family)), ("id", in_list(&event_ids))],
            json!({
                "status": "silent",
                "cue_text": null,
                "evidence": [],
                "selected_fact_ids": [],
                "face_outcome": null,
                "reason_code": "insufficient_evidence",
                "keeper_results": [],
                "gate": null,
                "silence_reason": "A source memory was removed. Tap again."
            }),
        )
        .await?;
    }
    for (table, ids) in [
        ("weaver_questions", question_ids),
        ("graph_edges", edge_ids),
        ("graph_nodes", node_ids),
    ] {
        if !ids.is_empty() {
            db.delete(table, &[("family_id", eq(family)), ("id", in_list(&ids))])
                .await?;
        }
    }
    let receipt_ids = std::iter::once(memory.to_owned())
        .chain(
            faces
                .iter()
                .filter(|face| face["memory_id"] == memory)
                .map(|face| string(face, "id").into()),
        )
        .collect::<Vec<_>>();
    db.delete(
        "ingestion_receipts",
        &[
            ("family_id", eq(family)),
            ("contributor_id", eq(contributor)),
            ("id", in_list(&receipt_ids)),
        ],
    )
    .await?;
    db.delete(
        "memories",
        &[
            ("family_id", eq(family)),
            ("contributor_id", eq(contributor)),
            ("id", eq(memory)),
        ],
    )
    .await?;
    Ok(true)
}
pub async fn provision_demo(db: &Database, password: &str) -> Result<()> {
    if password.is_empty() {
        return Err(ApiError::new(
            400,
            "The demo password needs to be supplied.",
        ));
    }
    for (table, columns) in [
        ("memories", "id,source,verified_facts"),
        ("ingestion_receipts", "id"),
        ("recall_events", "id,face_outcome,evidence,reason_code"),
        ("wearer_accounts", "user_id,family_id"),
        ("relatives", "id,is_self,self_capture_open"),
        ("pending_contributions", "id,payload,state"),
    ] {
        db.select(table, &[("select", columns.into()), ("limit", "1".into())])
            .await?;
    }
    if !db
        .select("wearer", &[("family_id", eq("demo"))])
        .await?
        .is_empty()
    {
        return Err(ApiError::new(
            409,
            "Apply consolidation migration 005 first",
        ));
    }
    for (name, args) in [
        (
            "capture_self_contribution",
            json!({"payload":{},"preview":""}),
        ),
        (
            "review_self_contribution",
            json!({"family":DEMO_FAMILY,"reviewer":null,"contribution":null,"decision":"invalid"}),
        ),
    ] {
        if !db
            .rpc(name, args)
            .await
            .is_err_and(|error| error.code.as_deref() == Some("22023"))
        {
            return Err(ApiError::new(
                503,
                "Apply the complete self-contribution migration 007 first",
            ));
        }
    }
    let mut users = vec![];
    for page in 1.. {
        let result = db
            .request(
                Method::GET,
                "/auth/v1/admin/users",
                &[("page", page.to_string()), ("per_page", "100".into())],
                None,
                None,
                "",
            )
            .await?;
        let rows = result["users"].as_array().ok_or_else(ApiError::provider)?;
        users.extend(rows.clone());
        if rows.len() < 100 {
            break;
        }
    }
    let dataset = demo_dataset();
    let accounts = [
        (
            "Maya",
            "maya@demo.kin.test",
            "organizer",
            Some("6ba867ac-36ef-432a-8306-1ce7f5df5289"),
        ),
        (
            "David",
            "david@demo.kin.test",
            "contributor",
            Some("adc50d52-40d5-48d6-8ce5-804724c71c49"),
        ),
        (
            "Elena",
            "elena@demo.kin.test",
            "contributor",
            Some("54d2a843-200b-4a8f-b1d0-6fe280d1e527"),
        ),
        ("Rosa", "rosa@demo.kin.test", "wearer", None),
    ];
    let mut mapped = vec![];
    for (name, email, role, contributor) in accounts {
        let candidates: Vec<_> = users
            .iter()
            .filter(|user| {
                string(user, "email").eq_ignore_ascii_case(email)
                    || (user["app_metadata"]["kin_family_id"] == "demo"
                        && string(user, "email")
                            .split('@')
                            .next()
                            .is_some_and(|local| local.eq_ignore_ascii_case(name)))
            })
            .collect();
        if candidates.len() > 1 {
            return Err(ApiError::new(
                409,
                "Conflicting duplicate demo accounts require reconciliation",
            ));
        }
        let existing = candidates.first().copied();
        if existing.is_some_and(|user| {
            user["app_metadata"]["kin_family_id"]
                .as_str()
                .is_some_and(|family| !["demo", DEMO_FAMILY].contains(&family))
        }) {
            return Err(ApiError::new(409, "Account belongs to another family"));
        }
        mapped.push((name, email, role, contributor, existing.cloned()));
    }
    insert_missing(
        db,
        "wearer",
        json!([{"family_id":DEMO_FAMILY,"name":"Rosa"}]),
    )
    .await?;
    insert_missing(db, "relatives", dataset["relatives"].clone()).await?;
    let relatives = db.family_rows("relatives", DEMO_FAMILY).await?;
    for (name, _, _, contributor, _) in &mapped {
        if contributor.is_some_and(|id| {
            !relatives
                .iter()
                .any(|relative| relative["id"] == id && relative["name"] == *name)
        }) {
            return Err(ApiError::new(409, "Canonical contributor mapping missing"));
        }
    }
    if !relatives.iter().any(|relative| relative["is_self"] == true) {
        insert_missing(
            db,
            "relatives",
            json!([
                {
                    "id": stable_id(&[
                        DEMO_FAMILY,
                        "self-keeper"
                    ]),
                    "family_id": DEMO_FAMILY,
                    "name": "Rosa",
                    "relation_to_wearer": "self",
                    "color": "#8a7fd1",
                    "is_self": true
                }
            ]),
        )
        .await?;
    }
    for (_, email, role, contributor, existing) in mapped {
        let mut metadata = existing
            .as_ref()
            .map(|user| user["app_metadata"].clone())
            .filter(Value::is_object)
            .unwrap_or(json!({}));
        metadata["kin_family_id"] = json!(DEMO_FAMILY);
        metadata["kin_contributor_id"] = json!(contributor);
        metadata["kin_role"] = json!(role);
        metadata["kin_admin"] = json!(role == "organizer");
        let result = if let Some(user) = existing {
            let mut body = json!({"app_metadata":metadata});
            if user["email"] != email {
                body["email"] = json!(email);
                body["email_confirm"] = json!(true);
            }
            db.request(
                Method::PUT,
                &format!("/auth/v1/admin/users/{}", string(&user, "id")),
                &[],
                Some(body),
                None,
                "",
            )
            .await?
        } else {
            db.request(Method::POST,"/auth/v1/admin/users",&[],Some(json!({"email":email,"password":password,"email_confirm":true,"app_metadata":metadata})),None,"").await?
        };
        let user = if result["user"].is_object() {
            &result["user"]
        } else {
            &result
        };
        if user["id"].as_str().is_none() {
            return Err(ApiError::provider());
        }
        if role == "wearer" {
            db.request(
                Method::POST,
                "/rest/v1/wearer_accounts",
                &[],
                Some(json!({"user_id":user["id"],"family_id":DEMO_FAMILY})),
                None,
                "resolution=merge-duplicates",
            )
            .await?;
        }
        let login = db
            .request(
                Method::POST,
                "/auth/v1/token?grant_type=password",
                &[],
                Some(json!({"email":email,"password":password})),
                Some(""),
                "",
            )
            .await?;
        let token = login["access_token"]
            .as_str()
            .ok_or_else(ApiError::provider)?;
        let claims = &login["user"]["app_metadata"];
        if claims["kin_family_id"] != DEMO_FAMILY
            || claims["kin_contributor_id"] != json!(contributor)
            || claims["kin_role"] != role
        {
            return Err(ApiError::new(409, "Demo claims mismatch"));
        }
        let own = db
            .request(
                Method::GET,
                "/rest/v1/wearer",
                &[("select", "family_id".into())],
                None,
                Some(token),
                "",
            )
            .await?;
        if own
            .as_array()
            .is_none_or(|rows| rows.len() != 1 || rows[0]["family_id"] != DEMO_FAMILY)
        {
            return Err(ApiError::new(403, "Family membership RLS failed"));
        }
        let foreign = db
            .request(
                Method::GET,
                "/rest/v1/graph_nodes",
                &[
                    ("select", "id".into()),
                    ("family_id", format!("neq.{DEMO_FAMILY}")),
                ],
                None,
                Some(token),
                "",
            )
            .await?;
        if foreign.as_array().is_none_or(|rows| !rows.is_empty()) {
            return Err(ApiError::new(403, "Family isolation failed"));
        }
        db.request(Method::POST, "/auth/v1/logout", &[], None, Some(token), "")
            .await?;
        println!("{email}: membership and sign-in verified");
    }
    Ok(())
}
pub fn narration_segments(transcript: &str, alignment: &Value) -> Value {
    let Some(characters) = alignment["characters"].as_array() else {
        return json!([]);
    };
    let Some(starts) = alignment["character_start_times_seconds"].as_array() else {
        return json!([]);
    };
    let Some(ends) = alignment["character_end_times_seconds"].as_array() else {
        return json!([]);
    };
    if characters.len() != starts.len() || characters.len() != ends.len() {
        return json!([]);
    }
    let text: String = characters.iter().filter_map(Value::as_str).collect();
    if text != transcript {
        return json!([]);
    }
    let mut offsets = std::collections::HashMap::new();
    let mut offset = 0;
    for (index, character) in characters.iter().enumerate() {
        offsets.insert(offset, index);
        offset += character.as_str().unwrap_or("").len();
    }
    offsets.insert(offset, characters.len());
    let mut segments = vec![];
    for matched in regex::Regex::new(r"[^.!?]+[.!?]*")
        .unwrap()
        .find_iter(transcript)
    {
        let text = matched.as_str().trim();
        if text.is_empty() {
            continue;
        }
        let start = matched.start() + matched.as_str().find(text).unwrap();
        let (Some(first), Some(after)) = (offsets.get(&start), offsets.get(&(start + text.len())))
        else {
            return json!([]);
        };
        if *after == 0 {
            return json!([]);
        }
        segments.push(json!({"start":starts[*first],"end":ends[*after-1],"text":text}));
    }
    json!(providers::valid_segments(&json!(segments), transcript))
}
pub async fn demo_audio(db: &Database, apply: bool) -> Result<()> {
    let baseline: Value = serde_json::from_str(include_str!("../../demo/shared-baseline.json"))?;
    let rows = db
        .select(
            "memories",
            &[("family_id", eq(DEMO_FAMILY)), ("kind", "eq.story".into())],
        )
        .await?;
    let candidates: Vec<_> = rows
        .iter()
        .filter(|memory| {
            memory["media_path"].is_null()
                && (memory["source"].is_null() || memory["source"]["reenacted"] == true)
                && baseline["memories"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|script| {
                        script["id"] == memory["id"]
                            && script["contributor_id"] == memory["contributor_id"]
                            && script["transcript"] == memory["transcript"]
                            && script["kind"] == "story"
                    })
        })
        .collect();
    println!(
        "{} demo stories need narration ({} characters).",
        candidates.len(),
        candidates
            .iter()
            .map(|memory| string(memory, "transcript").chars().count())
            .sum::<usize>()
    );
    if !apply {
        println!("Preview only. Add --apply to generate and attach audio.");
        return Ok(());
    }
    let key = std::env::var("ELEVENLABS_API_KEY")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(ApiError::provider)?;
    let cache = "demo/fixtures/private/narration";
    std::fs::create_dir_all(cache).map_err(|_| ApiError::provider())?;
    for memory in candidates {
        let voice = match string(memory, "contributor_id") {
            "6ba867ac-36ef-432a-8306-1ce7f5df5289" => "cgSgspJ2msm6clMCkdW9",
            "adc50d52-40d5-48d6-8ce5-804724c71c49" => "JBFqnCBsd6RMkjVDRZzb",
            "54d2a843-200b-4a8f-b1d0-6fe280d1e527" => "pFZP5JQG7iQjIQuC4Bku",
            _ => return Err(ApiError::provider()),
        };
        let model = "eleven_multilingual_v2";
        let transcript = string(memory, "transcript");
        let hash = digest(json!([transcript, voice, model]).to_string());
        let cached = format!("{cache}/{}-{hash}.json", string(memory, "id"));
        let generated: Value = match std::fs::read(&cached) {
            Ok(bytes) => serde_json::from_slice(&bytes)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let value = db
                    .client
                    .post(format!(
                        "https://api.elevenlabs.io/v1/text-to-speech/{voice}/with-timestamps?output_format=mp3_44100_128"
                    ))
                    .header("xi-api-key", &key)
                    .timeout(std::time::Duration::from_secs(60))
                    .json(&json!({
                        "text": transcript,
                        "model_id": model,
                        "voice_settings": {"stability": 0.55, "similarity_boost": 0.7},
                        "seed": 42
                    }))
                    .send()
                    .await?
                    .error_for_status()?
                    .json::<Value>()
                    .await?;
                std::fs::write(&cached, serde_json::to_vec(&value)?)
                    .map_err(|_| ApiError::provider())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&cached, std::fs::Permissions::from_mode(0o600))
                        .map_err(|_| ApiError::provider())?;
                }
                value
            }
            Err(_) => return Err(ApiError::provider()),
        };
        let audio = STANDARD
            .decode(string(&generated, "audio_base64"))
            .map_err(|_| ApiError::provider())?;
        if audio.len() < 1000 {
            return Err(ApiError::provider());
        }
        let segments = narration_segments(transcript, &generated["alignment"]);
        let path = format!(
            "{DEMO_FAMILY}/{}/{}/demo-narration/{hash}.mp3",
            string(memory, "contributor_id"),
            string(memory, "id")
        );
        db.upload(&path, audio, "audio/mpeg").await?;
        let mut source = memory["source"].clone();
        if !source.is_object() {
            source = json!({});
        }
        let fields = json!({
            "type": "synthetic",
            "demo": true,
            "generated_audio": true,
            "origin": "Fictional HackMIT demo narration",
            "audio_provider": "elevenlabs",
            "audio_model": model,
            "audio_voice_id": voice,
            "audio_segments": segments
        });
        for (key, value) in fields.as_object().unwrap() {
            source[key] = value.clone();
        }
        let query = [
            ("id", eq(string(memory, "id"))),
            ("family_id", eq(DEMO_FAMILY)),
            ("contributor_id", eq(string(memory, "contributor_id"))),
            ("transcript", eq(transcript)),
            ("media_path", "is.null".into()),
            (
                "source",
                if memory["source"].is_null() {
                    "is.null".into()
                } else {
                    eq(&memory["source"].to_string())
                },
            ),
        ];
        let attached = db
            .write(
                Method::PATCH,
                "memories",
                &query,
                json!({"media_path":path,"source":source}),
            )
            .await?;
        println!(
            "{}: {}",
            string(memory, "id"),
            if attached.is_empty() {
                "skipped changed memory"
            } else {
                "attached narration"
            }
        );
    }
    Ok(())
}
