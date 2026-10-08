use kin_brain::{
    admin::{demo_dataset, narration_segments},
    auth::{Identity, decode_cookie, read_cookie},
    ingestion::{
        Upload, anchor_origin_answer, literal_facts, prepare_graph, stable_id, validate_upload,
    },
    providers::{ExtractedEdge, Extraction, valid_segments},
    recall::{CueDraft, CueFact, render_facts, validate_grounding},
};
use serde_json::{Value, json};

fn identity() -> Identity {
    Identity {
        user: json!({"id":"user"}),
        family_id: "family".into(),
        contributor: Some(json!({"id":"contributor"})),
        membership: Value::Null,
        is_self: false,
        is_admin: false,
    }
}
#[test]
fn rejects_spoofed_media_containers_and_oversized_uploads() {
    for (bytes, mime, image) in [
        (b"not jpeg".as_slice(), "image/jpeg", true),
        (b"RIFF0000AVI ".as_slice(), "audio/wav", false),
        (b"<svg/>".as_slice(), "image/svg+xml", true),
        (b"not audio".as_slice(), "audio/mp4", false),
    ] {
        assert!(validate_upload(bytes, mime, image).is_err());
    }
    assert_eq!(
        validate_upload(&vec![0; 15 * 1024 * 1024 + 1], "image/png", true)
            .unwrap_err()
            .status
            .as_u16(),
        413
    );
    assert_eq!(
        validate_upload(&[], "audio/webm", false)
            .unwrap_err()
            .status
            .as_u16(),
        422
    );
    assert!(validate_upload(&[137, 80, 78, 71, 13, 10, 26, 10], "image/png", true).is_ok());
}
#[test]
fn preserves_utf16_human_spans_and_word_boundaries() {
    let text =
        "😀 Nora shares lemon cake on Sundays. Eleanor shares tea. NORA wears a yellow apron.";
    let nodes = vec![json!({"id":"subject","label":"Nora","aliases":[]})];
    let facts = literal_facts(text, &nodes, "memory", "contributor");
    assert_eq!(facts.len(), 2);
    assert_eq!(facts[0]["text"], "😀 Nora shares lemon cake on Sundays.");
    assert_eq!(facts[0]["sourceSpan"]["start"], 0);
    let second = text.find("NORA").unwrap();
    assert_eq!(
        facts[1]["sourceSpan"]["start"],
        text[..second].encode_utf16().count()
    );
    assert!(
        facts
            .iter()
            .all(|fact| !fact["text"].as_str().unwrap().contains("Eleanor"))
    );
}
#[test]
fn preserves_stable_ids_from_the_original_backend() {
    let dataset = demo_dataset();
    let id = stable_id(&["670f5075-c286-4b29-8074-86401c18d0c0", "demo-memory", "0"]);
    assert!(
        dataset["memories"]
            .as_array()
            .unwrap()
            .iter()
            .any(|memory| memory["id"] == id)
    );
    assert_ne!(stable_id(&["ab", "c"]), stable_id(&["a", "bc"]));
}
#[test]
fn resolves_graph_aliases_and_rejects_foreign_references() {
    let nodes = vec![
        json!({"id":"subject","family_id":"family","type":"person","label":"Nora","aliases":["Aunt Nora"],"relation_to_wearer":"sister"}),
    ];
    let extraction:Extraction=serde_json::from_value(json!({"summary":"Nora shares cake.","nodes":[{"ref":"new:nora","type":"person","label":" Aunt  Nora ","relation_to_wearer":null}],"edges":[]})).unwrap();
    let graph = prepare_graph(&identity(), "memory", &extraction, &nodes, &[]).unwrap();
    assert!(graph.nodes.is_empty());
    assert_eq!(graph.references["new:nora"], "subject");
    assert_eq!(graph.provenance.len(), 1);
    let mut foreign = extraction.clone();
    foreign.nodes[0].reference = "existing:foreign".into();
    assert!(prepare_graph(&identity(), "memory", &foreign, &nodes, &[]).is_err());
    let mut duplicate = extraction.clone();
    duplicate.nodes.push(extraction.nodes[0].clone());
    assert!(prepare_graph(&identity(), "memory", &duplicate, &nodes, &[]).is_err());
    let mut fabricated = extraction;
    fabricated.edges.push(ExtractedEdge {
        from: "new:nora".into(),
        to: "new:nora".into(),
        rel: "invented".into(),
    });
    assert!(prepare_graph(&identity(), "memory", &fabricated, &nodes, &[]).is_err());
}
#[test]
fn closes_origin_gaps_only_for_an_explicit_assertion() {
    let nodes = vec![
        json!({"id":"cake","type":"tradition","label":"lemon cake","aliases":[]}),
        json!({"id":"nora","type":"person","label":"Nora"}),
    ];
    let edges = vec![
        json!({"id":"participation","from_node":"nora","to_node":"cake","rel":"participates_in"}),
    ];
    let question = json!({"gap_node_id":"cake","gap_type":"missing_origin"});
    let extraction = Extraction {
        summary: "A recipe memory".into(),
        nodes: vec![],
        edges: vec![ExtractedEdge {
            from: "existing:cake".into(),
            rel: "origin".into(),
            to: "new:invented".into(),
        }],
    };
    for uncertain in [
        "Maybe she brought it from Italy.",
        "She did not bring it from Italy.",
        "I don't remember where it came from.",
    ] {
        let mut candidate = extraction.clone();
        assert!(
            anchor_origin_answer(&mut candidate, uncertain, &question, &nodes, &edges).is_empty()
        );
        assert!(candidate.edges.is_empty());
    }
    let mut asserted = extraction;
    assert_eq!(
        anchor_origin_answer(
            &mut asserted,
            "It was actually their mother's recipe. She brought it from Italy.",
            &question,
            &nodes,
            &edges
        )
        .len(),
        1
    );
    assert_eq!(asserted.edges[0].rel, "origin");
}
#[test]
fn rejects_rewritten_cues_unknown_facts_duplicate_ids_and_long_quotes() {
    let fact = CueFact {
        id: "fact".into(),
        text: "Nora shares lemon cake on Sundays.".into(),
        subject_node_id: "subject".into(),
        contributor_id: "contributor".into(),
        memory_id: "memory".into(),
        speaker: "You".into(),
    };
    let facts = vec![fact];
    let exact = render_facts(&facts);
    assert!(validate_grounding(
        &CueDraft {
            fact_ids: vec!["fact".into()],
            cue: exact.clone()
        },
        &facts
    ));
    for (ids, text) in [
        (vec!["fact".into()], "Nora baked cakes every Sunday.".into()),
        (vec!["unknown".into()], exact.clone()),
        (vec!["fact".into(), "fact".into()], exact.clone()),
        (vec![], exact),
    ] {
        assert!(!validate_grounding(
            &CueDraft {
                fact_ids: ids,
                cue: text
            },
            &facts
        ));
    }
    let mut long = facts[0].clone();
    long.text = "word ".repeat(31);
    let facts = vec![long];
    assert!(!validate_grounding(
        &CueDraft {
            fact_ids: vec!["fact".into()],
            cue: render_facts(&facts)
        },
        &facts
    ));
}
#[test]
fn rejects_unordered_audio_and_nonliteral_transcription_passages() {
    let transcript = "Nora shares cake. Rosa wears an apron.";
    assert_eq!(valid_segments(&json!([{"start":0,"end":2,"text":"Nora shares cake."},{"start":2,"end":4,"text":"Rosa wears an apron."}]),transcript).len(),2);
    for segments in [
        json!([{"start":2,"end":4,"text":"Nora shares cake."},{"start":1,"end":2,"text":"Rosa wears an apron."}]),
        json!([{"start":0,"end":0,"text":"Nora shares cake."}]),
        json!([{"start":0,"end":2,"text":"Invented memory"}]),
    ] {
        assert!(valid_segments(&segments, transcript).is_empty());
    }
}
#[test]
fn maps_narration_alignment_with_unicode_characters() {
    let transcript = "😀 Nora shares cake. Rosa smiles.";
    let characters: Vec<_> = transcript
        .chars()
        .map(|character| character.to_string())
        .collect();
    let starts: Vec<_> = (0..characters.len())
        .map(|index| index as f64 / 10.0)
        .collect();
    let ends: Vec<_> = (1..=characters.len())
        .map(|index| index as f64 / 10.0)
        .collect();
    let segments = narration_segments(
        transcript,
        &json!({"characters":characters,"character_start_times_seconds":starts,"character_end_times_seconds":ends}),
    );
    assert_eq!(segments.as_array().unwrap().len(), 2);
    assert_eq!(segments[0]["text"], "😀 Nora shares cake.");
    assert!(narration_segments("different text",&json!({"characters":[],"character_start_times_seconds":[],"character_end_times_seconds":[]})).as_array().unwrap().is_empty());
}
#[test]
fn reconstructs_chunked_cookie_sessions_without_trusting_their_claims() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    let encoded = format!(
        "base64-{}",
        URL_SAFE_NO_PAD.encode(
            json!({"access_token":"session-token","user":{"app_metadata":{"kin_admin":true}}})
                .to_string()
        )
    );
    let mut headers = axum::http::HeaderMap::new();
    let middle = encoded.len() / 2;
    headers.insert(
        "cookie",
        format!(
            "sb-test-auth-token.1={}; sb-test-auth-token.0={}",
            &encoded[middle..],
            &encoded[..middle]
        )
        .parse()
        .unwrap(),
    );
    let cookie = read_cookie(&headers, "sb-test-auth-token").unwrap();
    assert_eq!(
        decode_cookie(&cookie).unwrap()["access_token"],
        "session-token"
    );
    assert!(decode_cookie("invalid").is_none());
    let image = Upload {
        bytes: vec![255, 216, 255],
        mime: "image/jpeg".into(),
    };
    assert!(validate_upload(&image.bytes, &image.mime, true).is_ok());
}
