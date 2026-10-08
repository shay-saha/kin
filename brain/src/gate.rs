use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Claim {
    pub subject_node_id: String,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Evidence {
    pub memory_id: String,
    pub contributor_id: String,
    pub subject_node_id: String,
    pub source: String,
    pub supported_facts: Vec<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeeperResult {
    pub keeper_id: String,
    pub claim: Option<Claim>,
    pub memory_ids: Vec<String>,
    #[serde(rename = "v")]
    pub visual_confidence: f64,
    #[serde(rename = "r")]
    pub retrieval_confidence: f64,
    pub reason: String,
    pub support: String,
    pub evidence: Vec<Evidence>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum FaceOutcome {
    Matched {
        #[serde(rename = "subjectNodeId")]
        subject_node_id: String,
        model: String,
        #[serde(rename = "enrollmentIds")]
        enrollment_ids: Vec<String>,
        distance: f64,
        #[serde(rename = "v")]
        visual_confidence: f64,
    },
    NoFace {
        model: String,
    },
    Unknown {
        model: String,
    },
    Ambiguous {
        model: String,
    },
    Unavailable {
        model: String,
    },
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateInfo {
    pub face: FaceOutcome,
    #[serde(default)]
    pub provider_failure: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GateResult {
    #[serde(rename = "V")]
    pub visual_confidence: f64,
    #[serde(rename = "R")]
    pub retrieval_confidence: f64,
    #[serde(rename = "A")]
    pub agreement: f64,
    #[serde(rename = "S")]
    pub independent_support: f64,
    #[serde(rename = "X")]
    pub contradiction: f64,
    #[serde(rename = "C")]
    pub confidence: f64,
    pub threshold: f64,
    pub decision: String,
    pub reason: String,
    pub subject_node_id: Option<String>,
    pub agreeing_keeper_ids: Vec<String>,
    pub cited_memory_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason_code: Option<String>,
}
impl GateResult {
    fn silent(mut self, code: &str, reason: &str) -> Self {
        self.decision = "silent".into();
        self.reason_code = Some(code.into());
        self.reason = reason.into();
        self
    }
}
pub fn evaluate_gate(results: &[KeeperResult], info: &GateInfo) -> GateResult {
    let configured = std::env::var("KIN_GATE_THRESHOLD").unwrap_or_default();
    let threshold = if configured.is_empty() {
        Some(crate::config::GATE.threshold)
    } else {
        configured.parse::<f64>().ok().filter(|visual_confidence| {
            visual_confidence.is_finite() && (0.85..=1.0).contains(visual_confidence)
        })
    };
    let mut gate = GateResult {
        visual_confidence: 0.0,
        retrieval_confidence: 0.0,
        agreement: 0.0,
        independent_support: 0.0,
        contradiction: 0.0,
        confidence: 0.0,
        threshold: threshold.unwrap_or(1.0),
        decision: "silent".into(),
        reason: String::new(),
        subject_node_id: None,
        agreeing_keeper_ids: vec![],
        cited_memory_ids: vec![],
        reason_code: None,
    };
    if threshold.is_none() {
        return gate.silent("provider_failure", "Invalid evidence scores");
    }
    if info.provider_failure || matches!(info.face, FaceOutcome::Unavailable { .. }) {
        return gate.silent("provider_failure", "Required service unavailable");
    }
    let (subject, visual_confidence) = match &info.face {
        FaceOutcome::NoFace { .. } => return gate.silent("no_face", "No face detected"),
        FaceOutcome::Unknown { .. } => {
            return gate.silent("unknown_face", "No eligible enrolled face matched");
        }
        FaceOutcome::Ambiguous { .. } => {
            return gate.silent("ambiguous_face", "Face match is ambiguous");
        }
        FaceOutcome::Unavailable { .. } => {
            return gate.silent("provider_failure", "Required service unavailable");
        }
        FaceOutcome::Matched {
            subject_node_id,
            visual_confidence,
            ..
        } => (subject_node_id, *visual_confidence),
    };
    gate.subject_node_id = Some(subject.clone());
    gate.visual_confidence = visual_confidence;
    let has_subject = |result: &&KeeperResult| {
        result
            .claim
            .as_ref()
            .is_some_and(|claim| &claim.subject_node_id == subject)
    };
    let supporting: Vec<_> = results
        .iter()
        .filter(|result| result.support == "supports" && has_subject(result))
        .collect();
    let contradicting = results
        .iter()
        .filter(|result| {
            result.support == "contradicts"
                || (result.support == "supports" && !has_subject(result))
        })
        .count();
    gate.contradiction = contradicting as f64 / (contradicting + supporting.len()).max(1) as f64;
    if contradicting > 0 {
        return gate.silent("contradiction", "Keepers disagree");
    }
    if supporting.iter().any(|result| {
        result.evidence.is_empty()
            || result.memory_ids.is_empty()
            || result.memory_ids.iter().any(|id| {
                !result
                    .evidence
                    .iter()
                    .any(|evidence| &evidence.memory_id == id)
            })
            || result.evidence.iter().any(|evidence| {
                evidence.source != "human"
                    || evidence.contributor_id != result.keeper_id
                    || &evidence.subject_node_id != subject
                    || !result.memory_ids.contains(&evidence.memory_id)
                    || evidence.supported_facts.is_empty()
                    || evidence
                        .supported_facts
                        .iter()
                        .any(|fact| fact.trim().is_empty())
            })
    }) {
        return gate.silent(
            "no_provenance",
            "Supporting claim has no valid human provenance",
        );
    }
    let mut unique: Vec<&KeeperResult> = vec![];
    for result in supporting {
        if let Some(index) = unique
            .iter()
            .position(|other| other.keeper_id == result.keeper_id)
        {
            unique[index] = result;
        } else {
            unique.push(result);
        }
    }
    gate.agreeing_keeper_ids = unique
        .iter()
        .map(|result| result.keeper_id.clone())
        .collect();
    for id in unique.iter().flat_map(|result| &result.memory_ids) {
        if !gate.cited_memory_ids.contains(id) {
            gate.cited_memory_ids.push(id.clone());
        }
    }
    if !unique.is_empty() {
        gate.retrieval_confidence = unique
            .iter()
            .map(|result| result.retrieval_confidence)
            .sum::<f64>()
            / unique.len() as f64;
        gate.independent_support = 1.0;
    }
    if unique.len() >= 2 {
        gate.agreement = 1.0;
    }
    let weights = &crate::config::GATE;
    gate.confidence = weights.visual_weight * gate.visual_confidence
        + weights.retrieval_weight * gate.retrieval_confidence
        + weights.agreement_weight * gate.agreement
        + weights.support_weight * gate.independent_support
        - weights.contradiction_weight * gate.contradiction;
    if unique.len() < 2 {
        return gate.silent(
            "insufficient_evidence",
            "Two distinct contributors with human stories are required",
        );
    }
    if ![
        gate.visual_confidence,
        gate.retrieval_confidence,
        gate.agreement,
        gate.independent_support,
        gate.contradiction,
        gate.confidence,
    ]
    .iter()
    .all(|n| n.is_finite())
        || unique
            .iter()
            .any(|result| !(0.0..=1.0).contains(&result.retrieval_confidence))
        || !(0.0..=1.0).contains(&gate.visual_confidence)
    {
        return gate.silent("provider_failure", "Invalid evidence scores");
    }
    if gate.confidence < gate.threshold {
        return gate.silent("below_threshold", "Evidence is below the recall threshold");
    }
    gate.decision = "speak".into();
    gate.reason = "Verified independent human evidence".into();
    gate
}
