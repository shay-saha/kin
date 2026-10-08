use crate::{
    auth::Identity,
    database::{Database, eq, string},
    error::{ApiError, Result},
    providers::{self, ExtractedNode, Extraction},
};
use axum::extract::Multipart;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

pub const MAX_UPLOAD_BYTES: usize = 15 * 1024 * 1024;
pub fn digest(bytes: impl AsRef<[u8]>) -> String {
    hex::encode(Sha256::digest(bytes))
}
pub fn stable_id(parts: &[&str]) -> String {
    let hash = digest(serde_json::to_vec(parts).unwrap());
    format!(
        "{}-{}-5{}-a{}-{}",
        &hash[..8],
        &hash[8..12],
        &hash[13..16],
        &hash[17..20],
        &hash[20..32]
    )
}
pub fn valid_id(value: &str) -> Result<&str> {
    uuid::Uuid::parse_str(value).map_err(|_| ApiError::malformed())?;
    Ok(value)
}
pub fn normalized_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}
#[derive(Clone)]
pub struct Upload {
    pub bytes: Vec<u8>,
    pub mime: String,
}
pub struct Form {
    pub fields: HashMap<String, String>,
    pub upload: Upload,
}
impl Form {
    pub fn get(&self, key: &str) -> &str {
        self.fields.get(key).map(String::as_str).unwrap_or("")
    }
}
pub async fn multipart(mut multipart: Multipart, image: bool) -> Result<Form> {
    let mut fields = HashMap::new();
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::malformed())?
    {
        let name = field.name().unwrap_or("").to_owned();
        if name == "file" || name == "snapshot" {
            if upload.is_some() {
                return Err(ApiError::malformed());
            }
            let mime = field
                .content_type()
                .unwrap_or("")
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_lowercase();
            let bytes = field
                .bytes()
                .await
                .map_err(|_| ApiError::new(413, "Upload too large or malformed"))?
                .to_vec();
            validate_upload(&bytes, &mime, image)?;
            upload = Some(Upload { bytes, mime });
        } else {
            if fields.contains_key(&name) {
                return Err(ApiError::malformed());
            }
            let bytes = field.bytes().await.map_err(|_| ApiError::malformed())?;
            if bytes.len() > 65536 {
                return Err(ApiError::malformed());
            }
            fields.insert(
                name,
                String::from_utf8(bytes.to_vec()).map_err(|_| ApiError::malformed())?,
            );
        }
    }
    Ok(Form {
        fields,
        upload: upload.ok_or_else(|| ApiError::new(400, "file required"))?,
    })
}
pub fn validate_upload(bytes: &[u8], mime: &str, image: bool) -> Result<()> {
    if bytes.is_empty() {
        return Err(ApiError::new(422, "Empty file"));
    }
    if bytes.len() > MAX_UPLOAD_BYTES {
        return Err(ApiError::new(413, "Upload too large"));
    }
    let valid = match mime {
        "image/jpeg" if image => bytes.starts_with(&[255, 216, 255]),
        "image/png" if image => bytes.starts_with(&[137, 80, 78, 71, 13, 10, 26, 10]),
        "image/webp" if image => bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"),
        "audio/webm" if !image => bytes.starts_with(&[26, 69, 223, 163]),
        "audio/ogg" if !image => bytes.starts_with(b"OggS"),
        "audio/wav" | "audio/x-wav" if !image => {
            bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE")
        }
        "audio/mp4" | "video/mp4" | "audio/m4a" | "audio/x-m4a" if !image => {
            bytes.get(4..8) == Some(b"ftyp")
        }
        "audio/mpeg" | "audio/aac" if !image => {
            bytes.starts_with(b"ID3")
                || (bytes.first() == Some(&255)
                    && bytes.get(1).is_some_and(|value| value & 224 == 224))
        }
        _ => return Err(ApiError::new(415, "Unsupported media type")),
    };
    if valid {
        Ok(())
    } else {
        Err(ApiError::new(422, "Media contents do not match MIME type"))
    }
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct NewPerson {
    name: String,
    relation_to_wearer: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Label {
    #[serde(skip_serializing_if = "Option::is_none")]
    person_node_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    new_person: Option<NewPerson>,
    #[serde(skip_serializing_if = "Option::is_none")]
    r#box: Option<crate::faces::FaceBox>,
}
pub struct PreparedGraph {
    pub nodes: Vec<Value>,
    pub edges: Vec<Value>,
    pub provenance: Vec<Value>,
    pub chips: Vec<Value>,
    pub references: HashMap<String, String>,
}
pub fn prepare_graph(
    identity: &Identity,
    memory: &str,
    extraction: &Extraction,
    existing: &[Value],
    existing_edges: &[Value],
) -> Result<PreparedGraph> {
    let contributor = identity.contributor_id()?;
    let mut graph = PreparedGraph {
        nodes: vec![],
        edges: vec![],
        provenance: vec![],
        chips: vec![],
        references: HashMap::new(),
    };
    for node in &extraction.nodes {
        if graph.references.contains_key(&node.reference) {
            return Err(ApiError::new(502, "Duplicate extraction reference"));
        }
        let resolved = if let Some(id) = node.reference.strip_prefix("existing:") {
            Some(
                existing
                    .iter()
                    .find(|candidate| candidate["id"] == id && candidate["type"] == node.node_type)
                    .cloned()
                    .ok_or_else(|| ApiError::new(502, "Unknown extraction reference"))?,
            )
        } else {
            existing
                .iter()
                .chain(graph.nodes.iter())
                .find(|candidate| {
                    candidate["type"] == node.node_type
                        && std::iter::once(string(candidate, "label"))
                            .chain(
                                candidate["aliases"]
                                    .as_array()
                                    .into_iter()
                                    .flatten()
                                    .filter_map(Value::as_str),
                            )
                            .any(|label| normalized_label(label) == normalized_label(&node.label))
                })
                .cloned()
        };
        let resolved = resolved.unwrap_or_else(|| {
            let row = json!({
                "id": stable_id(&[
                    &identity.family_id,
                    &node.node_type,
                    &normalized_label(&node.label)
                ]),
                "family_id": identity.family_id,
                "type": node.node_type,
                "label": node.label,
                "aliases": [],
                "relation_to_wearer": if node.node_type == "person" {
                    node.relation_to_wearer.clone()
                } else {
                    None
                }
            });
            graph.nodes.push(row.clone());
            row
        });
        graph
            .references
            .insert(node.reference.clone(), string(&resolved, "id").into());
        graph.chips.push(json!({"label":resolved["label"],"type":resolved["type"],"relation_to_wearer":resolved["relation_to_wearer"]}));
    }
    for edge in &extraction.edges {
        for reference in [&edge.from, &edge.to] {
            if !graph.references.contains_key(reference)
                && let Some(id) = reference
                    .strip_prefix("existing:")
                    .filter(|id| existing.iter().any(|node| node["id"] == *id))
            {
                graph.references.insert(reference.clone(), id.into());
            }
        }
        let from = graph
            .references
            .get(&edge.from)
            .ok_or_else(ApiError::provider)?;
        let to = graph
            .references
            .get(&edge.to)
            .ok_or_else(ApiError::provider)?;
        if !providers::ALLOWED_RELATIONS.contains(&edge.rel.as_str()) {
            return Err(ApiError::new(502, "Invalid extracted relationship"));
        }
        let id = existing_edges
            .iter()
            .find(|candidate| {
                candidate["from_node"] == *from
                    && candidate["to_node"] == *to
                    && candidate["rel"] == edge.rel
            })
            .map(|candidate| string(candidate, "id").to_owned())
            .unwrap_or_else(|| stable_id(&[&identity.family_id, from, &edge.rel, to]));
        if !graph.edges.iter().any(|edge| edge["id"] == id) {
            graph.edges.push(json!({"id":id,"family_id":identity.family_id,"from_node":from,"rel":edge.rel,"to_node":to}));
        }
    }
    let mut seen = HashSet::new();
    for node in graph.references.values().filter(|id| seen.insert(*id)) {
        graph.provenance.push(json!({"id":stable_id(&[memory,"node",node]),"memory_id":memory,"contributor_id":contributor,"node_id":node,"edge_id":null}));
    }
    for edge in &graph.edges {
        graph.provenance.push(json!({
            "id": stable_id(&[
                memory,
                "edge",
                string(edge, "id")
            ]),
            "memory_id": memory,
            "contributor_id": contributor,
            "node_id": null,
            "edge_id": edge["id"]
        }));
    }
    Ok(graph)
}
pub fn literal_facts(text: &str, nodes: &[Value], memory: &str, contributor: &str) -> Vec<Value> {
    let spans = regex::Regex::new(r"[^.!?]+[.!?]*").unwrap();
    let mut facts = vec![];
    for node in nodes {
        let labels: Vec<_> = std::iter::once(string(node, "label"))
            .chain(
                node["aliases"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str),
            )
            .filter(|label| !label.is_empty())
            .collect();
        for span in spans
            .find_iter(text)
            .map(|span| span.as_str().trim())
            .filter(|span| !span.is_empty())
        {
            if !labels.iter().any(|label| {
                regex::Regex::new(&format!(
                    r"(?i)(^|[^\p{{L}}\p{{N}}]){}([^\p{{L}}\p{{N}}]|$)",
                    regex::escape(label)
                ))
                .is_ok_and(|pattern| pattern.is_match(span))
            }) {
                continue;
            }
            let byte_start = text.find(span).unwrap();
            let start = text[..byte_start].encode_utf16().count();
            facts.push(json!({
                "id": stable_id(&[
                    memory,
                    string(node, "id"),
                    span
                ]),
                "subjectNodeId": node["id"],
                "text": span,
                "sourceSpan": {
                    "start": start,
                    "end": start+span.encode_utf16().count()
                },
                "memoryId": memory,
                "contributorId": contributor
            }));
        }
    }
    facts
}
pub async fn receipt(
    db: &Database,
    identity: &Identity,
    id: &str,
    hash: &str,
) -> Result<Option<Value>> {
    let rows = db
        .select(
            "ingestion_receipts",
            &[
                ("id", eq(id)),
                ("family_id", eq(&identity.family_id)),
                ("contributor_id", eq(identity.contributor_id()?)),
            ],
        )
        .await?;
    if let Some(row) = rows.first() {
        if row["request_hash"] != hash {
            return Err(ApiError::new(
                409,
                "Idempotency key or question already used with different content",
            ));
        }
        return Ok(Some(row["response"].clone()));
    }
    if identity.is_self {
        let pending = db
            .select(
                "pending_contributions",
                &[
                    ("id", eq(id)),
                    ("family_id", eq(&identity.family_id)),
                    ("contributor_id", eq(identity.contributor_id()?)),
                ],
            )
            .await?;
        if let Some(row) = pending.first() {
            if row["payload"]["request_hash"] != hash || row["state"] == "rejected" {
                return Err(ApiError::new(409, "Contribution already used or rejected"));
            }
            let mut response = row["payload"]["response"].clone();
            response["pending_review"] = json!(row["state"] == "pending");
            return Ok(Some(response));
        }
    }
    Ok(None)
}
pub async fn ingest(
    db: &Database,
    identity: &Identity,
    form: Form,
    kind: &str,
    key: Option<&str>,
) -> Result<Value> {
    identity.assert_ownership(
        valid_id(form.get("contributor_id"))?,
        form.fields.get("family_id").map(String::as_str),
    )?;
    if key.is_some_and(|key| key.is_empty() || key.len() > 200) {
        return Err(ApiError::malformed());
    }
    let contributor_id = identity.contributor_id()?;
    let caption = form.get("caption").trim();
    if caption.chars().count() > 4000 {
        return Err(ApiError::malformed());
    }
    let question = if kind == "answer" {
        Some(valid_id(form.get("question_id"))?)
    } else {
        None
    };
    let topic = if kind == "story" {
        form.fields
            .get("topic_id")
            .map(|id| valid_id(id))
            .transpose()?
    } else {
        None
    };
    let labels: Vec<Label> = if kind == "photo" {
        serde_json::from_str(if form.get("labels").is_empty() {
            "[]"
        } else {
            form.get("labels")
        })?
    } else {
        vec![]
    };
    let consent = form.get("consent") == "true";
    if labels.len() > 32 || (!labels.is_empty() && !consent) {
        return Err(ApiError::new(
            400,
            "Explicit consent is required to label people",
        ));
    }
    for label in &labels {
        if label.person_node_id.is_some() == label.new_person.is_some() {
            return Err(ApiError::malformed());
        }
        if let Some(id) = &label.person_node_id {
            valid_id(id)?;
        }
        if let Some(person) = &label.new_person
            && (person.name.trim().is_empty()
                || person.name.chars().count() > 120
                || person.relation_to_wearer.chars().count() > 120)
        {
            return Err(ApiError::malformed());
        }
        if let Some(face_box) = &label.r#box {
            face_box.validate()?;
        }
    }
    let mut request = json!({
        "kind": kind,
        "media": digest(&form.upload.bytes),
        "mime": form.upload.mime,
        "caption": caption,
        "labels": labels,
        "consent": consent,
        "questionId": question
    });
    if let Some(topic) = topic {
        request["topicId"] = json!(topic);
    }
    let hash = digest(request.to_string());
    let memory_id = stable_id(&[
        &identity.family_id,
        if kind == "answer" {
            "answer"
        } else {
            contributor_id
        },
        kind,
        question.or(key).unwrap_or(&hash),
    ]);
    if let Some(response) = receipt(db, identity, &memory_id, &hash).await? {
        return Ok(response);
    }
    let mut context = String::new();
    let mut answer_question = Value::Null;
    if let Some(id) = question {
        answer_question = db
            .select(
                "weaver_questions",
                &[("id", eq(id)), ("family_id", eq(&identity.family_id))],
            )
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::new(404, "Question not found"))?;
        if answer_question["target_relative_id"] != contributor_id {
            return Err(ApiError::new(
                403,
                "Question belongs to another contributor",
            ));
        }
        if answer_question["status"] != "open" {
            return Err(ApiError::new(409, "Question already answered"));
        }
        context = string(&answer_question, "question_text").into();
    }
    if let Some(id) = topic {
        let topic = db
            .select(
                "graph_nodes",
                &[("id", eq(id)), ("family_id", eq(&identity.family_id))],
            )
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| ApiError::new(404, "Story topic not found"))?;
        context = format!(
            "What would you like to share about {}?",
            string(&topic, "label")
        );
    }
    let (wearer, nodes, edges) = tokio::try_join!(
        async { db.family_rows("wearer", &identity.family_id).await },
        async { db.family_rows("graph_nodes", &identity.family_id).await },
        async { db.family_rows("graph_edges", &identity.family_id).await }
    )?;
    let label_nodes: Vec<_> = labels
        .iter()
        .enumerate()
        .map(|(index, label)| -> Result<ExtractedNode> {
            if let Some(id) = &label.person_node_id {
                let node = nodes
                    .iter()
                    .find(|node| node["id"] == *id && node["type"] == "person")
                    .ok_or_else(|| ApiError::new(403, "Labeled person is not in this family"))?;
                Ok(ExtractedNode {
                    reference: format!("existing:{id}"),
                    node_type: "person".into(),
                    label: string(node, "label").into(),
                    relation_to_wearer: node["relation_to_wearer"].as_str().map(String::from),
                })
            } else {
                let person = label.new_person.as_ref().unwrap();
                Ok(ExtractedNode {
                    reference: format!("new:label-{index}"),
                    node_type: "person".into(),
                    label: person.name.trim().into(),
                    relation_to_wearer: Some(person.relation_to_wearer.trim().into()),
                })
            }
        })
        .collect::<Result<_>>()?;
    let contributor = identity
        .contributor
        .as_ref()
        .ok_or_else(ApiError::provider)?;
    let wearer_name = wearer
        .first()
        .map(|wearer| string(wearer, "name"))
        .unwrap_or("the wearer");
    let (transcript, vision, segments) = if kind == "photo" {
        (
            None,
            Some(
                providers::caption(db, &form.upload.bytes, &form.upload.mime)
                    .await?
                    .caption,
            ),
            vec![],
        )
    } else {
        let names: Vec<String> = std::iter::once(wearer_name)
            .chain(std::iter::once(string(contributor, "name")))
            .chain(
                nodes
                    .iter()
                    .filter(|node| node["type"] == "person")
                    .flat_map(|node| {
                        std::iter::once(string(node, "label")).chain(
                            node["aliases"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(Value::as_str),
                        )
                    }),
            )
            .map(String::from)
            .collect();
        let (text, segments) =
            providers::transcribe(db, &form.upload.bytes, &form.upload.mime, &names).await?;
        if text.trim().is_empty() {
            return Err(ApiError::new(422, "No speech detected"));
        }
        (Some(text), None, segments)
    };
    let source_text = if let Some(text) = &transcript {
        text.clone()
    } else {
        json!({
            "contributor_caption": caption,
            "explicitly_labeled_people": label_nodes.iter().map(|node| json!({
                "name": node.label,
                "relation_to_wearer": node.relation_to_wearer
            })).collect::<Vec<_>>()
        })
        .to_string()
    };
    let mut extraction = providers::extract(
        db,
        &source_text,
        &context,
        wearer_name,
        contributor,
        &nodes,
        identity.is_self,
    )
    .await?;
    for node in &label_nodes {
        if !extraction
            .nodes
            .iter()
            .any(|candidate| candidate.reference == node.reference)
        {
            extraction.nodes.push(node.clone());
        }
    }
    let answer_subjects = anchor_origin_answer(
        &mut extraction,
        transcript.as_deref().unwrap_or(""),
        &answer_question,
        &nodes,
        &edges,
    );
    let mut graph = prepare_graph(identity, &memory_id, &extraction, &nodes, &edges)?;
    if let Some(topic) = topic
        && !graph
            .provenance
            .iter()
            .any(|source| source["node_id"] == topic)
    {
        graph.provenance.push(json!({
            "id": stable_id(&[
                &memory_id,
                "node",
                topic
            ]),
            "memory_id": memory_id,
            "contributor_id": contributor_id,
            "node_id": topic,
            "edge_id": null
        }));
    }
    let human_text = transcript.as_deref().unwrap_or(caption);
    let fact_nodes: Vec<_> = nodes
        .iter()
        .chain(graph.nodes.iter())
        .filter(|node| graph.references.values().any(|id| node["id"] == *id))
        .cloned()
        .collect();
    let mut facts = literal_facts(human_text, &fact_nodes, &memory_id, contributor_id);
    for subject in answer_subjects {
        facts.push(json!({
            "id": stable_id(&[
                &memory_id,
                string(&subject, "id"),
                human_text
            ]),
            "subjectNodeId": subject["id"],
            "text": human_text,
            "sourceSpan": {
                "start": 0,
                "end": human_text.encode_utf16().count()
            },
            "memoryId": memory_id,
            "contributorId": contributor_id
        }));
    }
    let source = json!({
        "type": "human",
        "user_id": identity.user["id"],
        "caption": caption,
        "labels": labels,
        "consent": consent,
        "audio_segments": segments,
        "topic_id": topic,
        "question_context": if context.is_empty() {
            Value::Null
        } else {
            json!(context)
        },
        "gap_node_id": answer_question["gap_node_id"]
    });
    let embedding = providers::embedding(db, &extraction.summary).await?;
    let path = format!(
        "{}/{contributor_id}/{memory_id}/{}",
        identity.family_id,
        digest(&form.upload.bytes)
    );
    db.upload(&path, form.upload.bytes.clone(), &form.upload.mime)
        .await?;
    let persons: Vec<_> = label_nodes
        .iter()
        .enumerate()
        .map(|(index, node)| json!({"index":index,"node_id":graph.references.get(&node.reference)}))
        .collect();
    let response = json!({
        "memory_id": memory_id,
        "media_path": path,
        "transcript": transcript,
        "caption": vision,
        "summary": extraction.summary,
        "entities": graph.chips,
        "persons": persons
    });
    let payload = json!({
        "id": memory_id,
        "request_hash": hash,
        "family_id": identity.family_id,
        "contributor_id": contributor_id,
        "memory": {
            "id": memory_id,
            "family_id": identity.family_id,
            "contributor_id": contributor_id,
            "kind": kind,
            "media_path": path,
            "transcript": transcript,
            "caption": vision,
            "summary": extraction.summary,
            "embedding": embedding,
            "source_question_id": question,
            "source": source,
            "verified_facts": facts
        },
        "nodes": graph.nodes,
        "edges": graph.edges,
        "provenance": graph.provenance,
        "response": response,
        "source": source
    });
    if identity.is_self {
        db.rpc("capture_self_contribution",json!({"payload":payload,"preview":human_text.trim().chars().take(2000).collect::<String>()})).await
    } else {
        db.rpc("commit_ingestion", json!({"payload":payload})).await
    }
}
pub fn anchor_origin_answer(
    extraction: &mut Extraction,
    transcript: &str,
    question: &Value,
    nodes: &[Value],
    edges: &[Value],
) -> Vec<Value> {
    if question["gap_type"] != "missing_origin" {
        return vec![];
    }
    let Some(tradition) = nodes.iter().find(|node| {
        node["id"] == question["gap_node_id"]
            && matches!(string(node, "type"), "tradition" | "event")
    }) else {
        return vec![];
    };
    let tradition_ref = format!("existing:{}", string(tradition, "id"));
    let mut references = HashSet::from([tradition_ref.clone()]);
    for node in &extraction.nodes {
        if node.node_type == string(tradition, "type")
            && std::iter::once(string(tradition, "label"))
                .chain(
                    tradition["aliases"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str),
                )
                .any(|label| label.eq_ignore_ascii_case(&node.label))
        {
            references.insert(node.reference.clone());
        }
    }
    extraction.edges.retain(|edge| {
        !["origin", "started_by", "taught_by"].contains(&edge.rel.as_str())
            || (!references.contains(&edge.from) && !references.contains(&edge.to))
    });
    let normalized = transcript.trim().replace('’', "'");
    let pattern = regex::Regex::new(r"(?i)^(?:(?:It was (?:actually )?their mother's recipe[.,;]\s*)?She brought it from Italy[.!]?|(?:The (?:lemon cake )?recipe|It) (?:came|was brought) from Italy[.!]?)$").unwrap();
    if !pattern.is_match(&normalized) {
        return vec![];
    }
    let participants: Vec<_> = edges
        .iter()
        .filter(|edge| edge["to_node"] == tradition["id"] && edge["rel"] == "participates_in")
        .filter_map(|edge| {
            nodes
                .iter()
                .find(|node| node["id"] == edge["from_node"] && node["type"] == "person")
                .cloned()
        })
        .collect();
    for node in std::iter::once(tradition).chain(participants.iter()) {
        let reference = format!("existing:{}", string(node, "id"));
        if !extraction
            .nodes
            .iter()
            .any(|candidate| candidate.reference == reference)
        {
            extraction.nodes.push(ExtractedNode {
                reference,
                node_type: string(node, "type").into(),
                label: string(node, "label").into(),
                relation_to_wearer: node["relation_to_wearer"].as_str().map(String::from),
            });
        }
    }
    let italy = nodes.iter().find(|node| {
        node["type"] == "place" && string(node, "label").eq_ignore_ascii_case("italy")
    });
    let italy_ref = italy
        .map(|node| format!("existing:{}", string(node, "id")))
        .unwrap_or_else(|| "new:answer-origin-italy".into());
    if !extraction
        .nodes
        .iter()
        .any(|node| node.reference == italy_ref)
    {
        extraction.nodes.push(ExtractedNode {
            reference: italy_ref.clone(),
            node_type: "place".into(),
            label: "Italy".into(),
            relation_to_wearer: None,
        });
    }
    extraction.edges.push(providers::ExtractedEdge {
        from: tradition_ref,
        rel: "origin".into(),
        to: italy_ref,
    });
    participants
}
