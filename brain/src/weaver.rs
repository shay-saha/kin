use std::collections::{BTreeSet, HashMap, HashSet};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GapType {
    MissingOrigin,
    OrphanObject,
    UnrelatedPerson,
}

const GAP_ORDER: [GapType; 3] = [
    GapType::MissingOrigin,
    GapType::OrphanObject,
    GapType::UnrelatedPerson,
];

const ORIGIN_RELS: [&str; 3] = ["origin", "started_by", "taught_by"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gap {
    #[serde(rename = "type")]
    pub gap_type: GapType,
    #[serde(rename = "nodeId")]
    pub node_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphNode {
    pub id: String,
    #[serde(rename = "type")]
    pub node_type: String,
    pub label: String,
    pub relation_to_wearer: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphEdge {
    pub id: String,
    pub from_node: String,
    pub rel: String,
    pub to_node: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Provenance {
    pub id: String,
    pub memory_id: String,
    pub node_id: Option<String>,
    pub edge_id: Option<String>,
    pub contributor_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeaverMemory {
    pub id: String,
    pub contributor_id: String,
    pub kind: String,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Relative {
    pub id: String,
    pub name: String,
    pub relation_to_wearer: String,
    pub color: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeaverData {
    pub nodes: Vec<GraphNode>,
    pub edges: Vec<GraphEdge>,
    pub provenance: Vec<Provenance>,
    pub memories: Vec<WeaverMemory>,
    #[serde(rename = "facePersonIds")]
    pub face_person_ids: Vec<String>,
    #[serde(rename = "wearerNodeId")]
    pub wearer_node_id: Option<String>,
    pub relatives: Vec<Relative>,
    #[serde(rename = "openQuestionRelativeIds")]
    pub open_question_relative_ids: Vec<String>,
}

fn edges_touching<'a>(node_id: &str, edges: &'a [GraphEdge]) -> Vec<&'a GraphEdge> {
    edges
        .iter()
        .filter(|e| e.from_node == node_id || e.to_node == node_id)
        .collect()
}

fn neighbor_ids(node_id: &str, edges: &[GraphEdge]) -> HashSet<String> {
    let mut out = HashSet::new();
    for e in edges {
        if e.from_node == node_id {
            out.insert(e.to_node.clone());
        }
        if e.to_node == node_id {
            out.insert(e.from_node.clone());
        }
    }
    out
}

fn contributors_of(d: &WeaverData, node_id: &str) -> BTreeSet<String> {
    let edge_ids: HashSet<&str> = edges_touching(node_id, &d.edges)
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    d.provenance
        .iter()
        .filter(|p| {
            p.node_id.as_deref() == Some(node_id)
                || p.edge_id
                    .as_deref()
                    .is_some_and(|eid| edge_ids.contains(eid))
        })
        .map(|p| p.contributor_id.clone())
        .collect()
}

fn memory_ids_touching_node(d: &WeaverData, node_id: &str) -> HashSet<String> {
    let edge_ids: HashSet<&str> = edges_touching(node_id, &d.edges)
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    d.provenance
        .iter()
        .filter(|p| {
            p.node_id.as_deref() == Some(node_id)
                || p.edge_id
                    .as_deref()
                    .is_some_and(|eid| edge_ids.contains(eid))
        })
        .map(|p| p.memory_id.clone())
        .collect()
}

pub fn find_gaps(d: &WeaverData) -> Vec<Gap> {
    let mut gaps = Vec::new();
    let by_id: HashMap<&str, &GraphNode> = d.nodes.iter().map(|n| (n.id.as_str(), n)).collect();

    for n in &d.nodes {
        if n.node_type != "tradition" && n.node_type != "event" {
            continue;
        }
        let incoming = d
            .edges
            .iter()
            .filter(|e| e.to_node == n.id && e.rel == "participates_in")
            .count();
        let has_origin = edges_touching(&n.id, &d.edges)
            .iter()
            .any(|e| ORIGIN_RELS.contains(&e.rel.as_str()));
        if incoming >= 2 && !has_origin {
            gaps.push(Gap {
                gap_type: GapType::MissingOrigin,
                node_id: n.id.clone(),
            });
        }
    }

    for n in &d.nodes {
        if n.node_type != "object" {
            continue;
        }
        let memory_count: HashSet<&str> = d
            .provenance
            .iter()
            .filter(|p| p.node_id.as_deref() == Some(n.id.as_str()))
            .map(|p| p.memory_id.as_str())
            .collect();
        if memory_count.len() != 1 {
            continue;
        }
        let linked = edges_touching(&n.id, &d.edges).iter().any(|e| {
            let other_id = if e.from_node == n.id {
                &e.to_node
            } else {
                &e.from_node
            };
            by_id
                .get(other_id.as_str())
                .is_some_and(|o| matches!(o.node_type.as_str(), "tradition" | "event" | "place"))
        });
        if !linked {
            gaps.push(Gap {
                gap_type: GapType::OrphanObject,
                node_id: n.id.clone(),
            });
        }
    }

    for n in &d.nodes {
        if n.node_type != "person" || !d.face_person_ids.contains(&n.id) {
            continue;
        }
        if n.relation_to_wearer.is_some() {
            continue;
        }
        let touches_wearer = d.wearer_node_id.as_deref().is_some_and(|w| {
            d.edges.iter().any(|e| {
                (e.from_node == n.id && e.to_node == w) || (e.to_node == n.id && e.from_node == w)
            })
        });
        if !touches_wearer {
            gaps.push(Gap {
                gap_type: GapType::UnrelatedPerson,
                node_id: n.id.clone(),
            });
        }
    }

    gaps
}

pub fn gap_score(d: &WeaverData, gap: &Gap) -> usize {
    edges_touching(&gap.node_id, &d.edges).len() + 2 * contributors_of(d, &gap.node_id).len()
}

pub fn pick_top_gap(d: &WeaverData) -> Option<Gap> {
    let mut gaps = find_gaps(d);
    if gaps.is_empty() {
        return None;
    }
    let order = |t: GapType| GAP_ORDER.iter().position(|g| *g == t).unwrap_or(usize::MAX);
    gaps.sort_by(|a, b| {
        gap_score(d, b)
            .cmp(&gap_score(d, a))
            .then_with(|| order(a.gap_type).cmp(&order(b.gap_type)))
            .then_with(|| a.node_id.cmp(&b.node_id))
    });
    gaps.into_iter().next()
}

pub fn route_question(d: &WeaverData, gap: &Gap) -> Option<String> {
    let neighbors = neighbor_ids(&gap.node_id, &d.edges);
    let touching = memory_ids_touching_node(d, &gap.node_id);
    let describers: HashSet<String> = d
        .memories
        .iter()
        .filter(|m| m.kind != "photo" && touching.contains(&m.id))
        .map(|m| m.contributor_id.clone())
        .collect();
    let open: HashSet<&str> = d
        .open_question_relative_ids
        .iter()
        .map(String::as_str)
        .collect();

    let mut candidates: Vec<&Relative> = d
        .relatives
        .iter()
        .filter(|r| !open.contains(r.id.as_str()) && !describers.contains(&r.id))
        .collect();
    if candidates.is_empty() {
        candidates = d
            .relatives
            .iter()
            .filter(|r| !open.contains(r.id.as_str()))
            .collect();
    }
    if candidates.is_empty() {
        return None;
    }

    let mut neighbor_edge_ids: HashSet<&str> = HashSet::new();
    for nid in &neighbors {
        for e in edges_touching(nid, &d.edges) {
            neighbor_edge_ids.insert(e.id.as_str());
        }
    }
    let touches_neighbor = |memory_id: &str| -> bool {
        d.provenance.iter().any(|p| {
            p.memory_id == memory_id
                && (p.node_id.as_deref().is_some_and(|n| neighbors.contains(n))
                    || p.edge_id
                        .as_deref()
                        .is_some_and(|e| neighbor_edge_ids.contains(e)))
        })
    };
    let gap_memories = memory_ids_touching_node(d, &gap.node_id);

    struct Scored<'a> {
        relative: &'a Relative,
        score: usize,
        has_related_photo: bool,
    }

    let mut scored: Vec<Scored> = candidates
        .into_iter()
        .map(|r| {
            let mems: Vec<&WeaverMemory> = d
                .memories
                .iter()
                .filter(|m| m.contributor_id == r.id)
                .collect();
            let neighbor_mems: Vec<&WeaverMemory> = mems
                .iter()
                .copied()
                .filter(|m| touches_neighbor(&m.id))
                .collect();
            let gap_mems: Vec<&WeaverMemory> = mems
                .iter()
                .copied()
                .filter(|m| gap_memories.contains(&m.id))
                .collect();
            let has_related_photo = neighbor_mems
                .iter()
                .chain(gap_mems.iter())
                .any(|m| m.kind == "photo");
            Scored {
                relative: r,
                score: 2 * neighbor_mems.len() + gap_mems.len(),
                has_related_photo,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| b.has_related_photo.cmp(&a.has_related_photo))
            .then_with(|| a.relative.id.cmp(&b.relative.id))
    });
    scored.first().map(|s| s.relative.id.clone())
}
