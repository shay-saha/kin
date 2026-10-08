use crate::config::{FACE, RETRIEVAL};
use crate::gate::{Claim, Evidence, FaceOutcome, KeeperResult};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Keeper {
    pub relative_id: String,
    pub name: String,
    pub color: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Source {
    #[serde(rename = "type")]
    pub kind: String,
    pub caption: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Span {
    pub start: usize,
    pub end: usize,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Fact {
    pub id: String,
    pub subject_node_id: String,
    pub text: String,
    pub contributor_id: String,
    pub memory_id: String,
    pub source_span: Option<Span>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredMemory {
    pub id: String,
    pub contributor_id: String,
    pub transcript: Option<String>,
    pub source: Option<Source>,
    #[serde(default)]
    pub verified_facts: Vec<Fact>,
    pub similarity: f64,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeeperInput {
    pub face: FaceOutcome,
    pub subject_label: String,
    pub memories: Vec<ScoredMemory>,
}
pub fn visual_confidence(distance: f64) -> f64 {
    ((FACE.zero_confidence_distance - distance) / FACE.confidence_window).clamp(0.0, 1.0)
}
pub fn retrieval_confidence(similarity: f64) -> f64 {
    ((similarity - RETRIEVAL.similarity_floor) / RETRIEVAL.similarity_window).clamp(0.0, 1.0)
}
fn word_boundary(character: char) -> bool {
    !(character.is_ascii_alphanumeric() || character == '_')
}
fn contains_phrase(text: &str, needle: &str) -> bool {
    text.match_indices(needle).any(|(index, _)| {
        (index == 0 || text[..index].chars().next_back().is_some_and(word_boundary))
            && (index + needle.len() == text.len()
                || text[index + needle.len()..]
                    .chars()
                    .next()
                    .is_some_and(word_boundary))
    })
}
fn identity_only(text: &str) -> bool {
    let lower = text.trim().to_lowercase();
    let tokens: Vec<_> = lower.split_whitespace().collect();
    let starts = |a: &str, b: &str| {
        tokens.first() == Some(&a)
            && tokens.get(1).is_some_and(|w| {
                w.strip_prefix(b).is_some_and(|tail| {
                    tail.is_empty() || tail.chars().next().is_some_and(word_boundary)
                })
            })
    };
    ["this", "that", "here"]
        .iter()
        .any(|a| starts(a, "is") || starts(a, "was"))
        || starts("photo", "of")
        || starts("picture", "of")
        || (tokens.first() == Some(&"a") && identity_only(&tokens[1..].join(" ")))
}
pub fn human_facts<'a>(memory: &'a ScoredMemory, subject: &str) -> Vec<&'a Fact> {
    let Some(source) = &memory.source else {
        return vec![];
    };
    if source.kind != "human" {
        return vec![];
    }
    let literal = memory
        .transcript
        .as_deref()
        .filter(|text| !text.is_empty())
        .or(source.caption.as_deref())
        .unwrap_or("");
    memory
        .verified_facts
        .iter()
        .filter(|fact| {
            fact.subject_node_id == subject
                && fact.memory_id == memory.id
                && fact.contributor_id == memory.contributor_id
                && fact.text.split_whitespace().count() >= 4
                && !identity_only(&fact.text)
                && literal.contains(&fact.text)
                && fact.source_span.as_ref().is_none_or(|span| {
                    let units: Vec<u16> = literal.encode_utf16().collect();
                    units
                        .get(span.start..span.end)
                        .and_then(|span_text| String::from_utf16(span_text).ok())
                        .as_deref()
                        == Some(&fact.text)
                })
        })
        .collect()
}
pub fn build_keeper_result(keeper: &Keeper, input: &KeeperInput) -> KeeperResult {
    let mut result = KeeperResult {
        keeper_id: keeper.relative_id.clone(),
        claim: None,
        memory_ids: vec![],
        visual_confidence: 0.0,
        retrieval_confidence: 0.0,
        reason: "No meaningful human story linked to this subject".into(),
        support: "abstains".into(),
        evidence: vec![],
    };
    let FaceOutcome::Matched {
        subject_node_id: subject,
        visual_confidence,
        ..
    } = &input.face
    else {
        return result;
    };
    let mut own: Vec<_> = input
        .memories
        .iter()
        .filter(|memory| {
            memory.contributor_id == keeper.relative_id
                && memory.similarity.is_finite()
                && !human_facts(memory, subject).is_empty()
        })
        .collect();
    own.sort_by(|a, b| b.similarity.total_cmp(&a.similarity));
    own.truncate(RETRIEVAL.max_subject_memories);
    if own.is_empty() {
        return result;
    }
    let disputed = own.iter().any(|memory| {
        human_facts(memory, subject).iter().any(|fact| {
            let lower = fact.text.to_lowercase();
            [
                "never",
                "incorrect",
                "mistaken",
                "did not",
                "didn't",
                "was not",
                "wasn't",
                "not true",
                "not actually",
            ]
            .iter()
            .any(|phrase| contains_phrase(&lower, phrase))
        })
    });
    result.claim = Some(Claim {
        subject_node_id: subject.clone(),
        label: input.subject_label.clone(),
    });
    result.memory_ids = own.iter().map(|memory| memory.id.clone()).collect();
    result.visual_confidence = *visual_confidence;
    result.retrieval_confidence = retrieval_confidence(own[0].similarity);
    result.support = if disputed { "contradicts" } else { "supports" }.into();
    result.reason = if disputed {
        "Human evidence contains an explicit denial; review before recall"
    } else {
        "Contributor's human story supports the matched subject"
    }
    .into();
    result.evidence = own
        .iter()
        .map(|memory| Evidence {
            memory_id: memory.id.clone(),
            contributor_id: memory.contributor_id.clone(),
            subject_node_id: subject.clone(),
            source: "human".into(),
            supported_facts: human_facts(memory, subject)
                .iter()
                .map(|fact| fact.text.clone())
                .collect(),
        })
        .collect();
    result
}
