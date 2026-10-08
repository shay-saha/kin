pub struct GateConfig {
    pub visual_weight: f64,
    pub retrieval_weight: f64,
    pub agreement_weight: f64,
    pub support_weight: f64,
    pub contradiction_weight: f64,
    pub threshold: f64,
}

pub const GATE: GateConfig = GateConfig {
    visual_weight: 0.35,
    retrieval_weight: 0.25,
    agreement_weight: 0.20,
    support_weight: 0.15,
    contradiction_weight: 0.25,
    threshold: 0.85,
};

pub struct FaceConfig {
    pub zero_confidence_distance: f64,
    pub confidence_window: f64,
}

pub const FACE: FaceConfig = FaceConfig {
    zero_confidence_distance: 0.6,
    confidence_window: 0.25,
};

pub struct RetrievalConfig {
    pub similarity_floor: f64,
    pub similarity_window: f64,
    pub max_subject_memories: usize,
}

pub const RETRIEVAL: RetrievalConfig = RetrievalConfig {
    similarity_floor: 0.2,
    similarity_window: 0.4,
    max_subject_memories: 3,
};
