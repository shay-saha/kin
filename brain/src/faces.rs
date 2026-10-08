use crate::{
    auth::Identity,
    database::{Database, eq, string},
    error::{ApiError, Result},
    gate::FaceOutcome,
    ingestion::{Upload, digest, receipt, stable_id},
};
use aes_gcm::{Aes256Gcm, KeyInit, Nonce, aead::Aead};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use image::{ImageReader, RgbImage, imageops::FilterType};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
};
use tract_onnx::prelude::*;

pub const NATIVE_MODEL: &str = "kin-yunet-2023mar:sface-2021dec:recognition128:rgb-exif-v1";
pub const LEGACY_MODEL: &str =
    "face-api-1.7.15:ssd-mobilenetv1:landmark68:recognition128:rgb-exif-v1";
pub fn model() -> String {
    std::env::var("KIN_FACE_MODEL")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| NATIVE_MODEL.into())
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FaceBox {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}
impl FaceBox {
    pub fn validate(&self) -> Result<()> {
        if [self.x, self.y, self.width, self.height]
            .iter()
            .all(|value| value.is_finite())
            && self.x >= 0.0
            && self.y >= 0.0
            && self.width > 0.0
            && self.height > 0.0
        {
            Ok(())
        } else {
            Err(ApiError::malformed())
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Detection {
    #[serde(rename = "box")]
    pub face_box: FaceBox,
    pub descriptor: Vec<f64>,
}
#[derive(Deserialize)]
struct Inference {
    model: String,
    width: u32,
    height: u32,
    faces: Vec<Detection>,
}
type OnnxPlan = SimplePlan<TypedFact, Box<dyn TypedOp>, TypedModel>;
struct Models {
    detector: OnnxPlan,
    recognizer: OnnxPlan,
    output_names: Vec<String>,
}
static MODELS: OnceLock<Arc<Mutex<Models>>> = OnceLock::new();
static MODEL_INITIALIZATION: Mutex<()> = Mutex::new(());
const MODEL_ARTIFACTS: [(&str, &str, &str); 2] = [
    (
        "face_detection_yunet",
        "face_detection_yunet_2023mar.onnx",
        "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4",
    ),
    (
        "face_recognition_sface",
        "face_recognition_sface_2021dec.onnx",
        "0ba9fbfa01b5270c96627c4ef784da859931e02f04419c829e83484087c34e79",
    ),
];
fn native_models() -> Result<Arc<Mutex<Models>>> {
    if let Some(models) = MODELS.get() {
        return Ok(models.clone());
    }
    let _initialization = MODEL_INITIALIZATION
        .lock()
        .map_err(|_| ApiError::provider())?;
    if let Some(models) = MODELS.get() {
        return Ok(models.clone());
    }
    let directory = std::env::var("KIN_FACE_WEIGHTS").unwrap_or_else(|_| "brain/models".into());
    for (_, file, expected_hash) in MODEL_ARTIFACTS {
        let bytes = std::fs::read(format!("{directory}/{file}")).map_err(|_| {
            ApiError::new(
                503,
                "Rust face models unavailable; run the download-models command",
            )
        })?;
        if digest(&bytes) != expected_hash {
            return Err(ApiError::new(503, "Face model checksum mismatch"));
        }
    }
    let load = || -> TractResult<Models> {
        let detector = tract_onnx::onnx()
            .model_for_path(format!("{directory}/face_detection_yunet_2023mar.onnx"))?
            .with_input_fact(0, f32::fact([1, 3, 640, 640]).into())?;
        let output_names = detector
            .output_outlets()?
            .iter()
            .map(|outlet| {
                detector
                    .outlet_label(*outlet)
                    .unwrap_or(&detector.node(outlet.node).name)
                    .to_owned()
            })
            .collect();
        let recognizer = tract_onnx::onnx()
            .model_for_path(format!("{directory}/face_recognition_sface_2021dec.onnx"))?
            .with_input_fact(0, f32::fact([1, 3, 112, 112]).into())?
            .into_optimized()?
            .into_runnable()?;
        Ok(Models {
            detector: detector.into_optimized()?.into_runnable()?,
            recognizer,
            output_names,
        })
    };
    let models = Arc::new(Mutex::new(load().map_err(|error| {
        tracing::error!(error=%error,"Face model loading failed");
        ApiError::new(
            503,
            "Rust face models unavailable; run the download-models command",
        )
    })?));
    let _ = MODELS.set(models.clone());
    Ok(MODELS.get().cloned().unwrap_or(models))
}
fn image_tensor(image: &RgbImage, bgr: bool) -> Tensor {
    let (width, height) = image.dimensions();
    let array = tract_ndarray::Array4::from_shape_fn(
        (1, 3, height as usize, width as usize),
        |(_, channel, y, x)| {
            image.get_pixel(x as u32, y as u32)[if bgr { 2 - channel } else { channel }] as f32
        },
    );
    array.into_tensor()
}
#[derive(Clone)]
struct Candidate {
    face_box: FaceBox,
    landmarks: [[f64; 2]; 5],
    score: f32,
}
fn overlap(a: &FaceBox, b: &FaceBox) -> f64 {
    let width = ((a.x + a.width).min(b.x + b.width) - a.x.max(b.x)).max(0.0);
    let height = ((a.y + a.height).min(b.y + b.height) - a.y.max(b.y)).max(0.0);
    let intersection = width * height;
    intersection / (a.width * a.height + b.width * b.height - intersection).max(f64::EPSILON)
}
fn aligned_face(image: &RgbImage, landmarks: &[[f64; 2]; 5]) -> Result<RgbImage> {
    let target = [
        [38.2946, 51.6963],
        [73.5318, 51.5014],
        [56.0252, 71.7366],
        [41.5493, 92.3655],
        [70.7299, 92.2041],
    ];
    let mean = |points: &[[f64; 2]; 5]| {
        [
            points.iter().map(|point| point[0]).sum::<f64>() / 5.0,
            points.iter().map(|point| point[1]).sum::<f64>() / 5.0,
        ]
    };
    let source_mean = mean(landmarks);
    let target_mean = mean(&target);
    let mut denominator = 0.0;
    let mut real = 0.0;
    let mut imaginary = 0.0;
    for (source, target) in landmarks.iter().zip(target) {
        let x = source[0] - source_mean[0];
        let y = source[1] - source_mean[1];
        let u = target[0] - target_mean[0];
        let v = target[1] - target_mean[1];
        denominator += x * x + y * y;
        real += x * u + y * v;
        imaginary += x * v - y * u;
    }
    if denominator <= f64::EPSILON {
        return Err(ApiError::provider());
    }
    let a = real / denominator;
    let b = imaginary / denominator;
    let tx = target_mean[0] - a * source_mean[0] + b * source_mean[1];
    let ty = target_mean[1] - b * source_mean[0] - a * source_mean[1];
    let determinant = a * a + b * b;
    if !determinant.is_finite() || determinant <= f64::EPSILON {
        return Err(ApiError::provider());
    }
    Ok(RgbImage::from_fn(112, 112, |u, v| {
        let source_x = (a * (u as f64 - tx) + b * (v as f64 - ty)) / determinant;
        let source_y = (-b * (u as f64 - tx) + a * (v as f64 - ty)) / determinant;
        let x = source_x.floor() as i64;
        let y = source_y.floor() as i64;
        let dx = source_x - x as f64;
        let dy = source_y - y as f64;
        let mut pixel = [0; 3];
        for (channel, value) in pixel.iter_mut().enumerate() {
            let sample = |x: i64, y: i64| {
                if x >= 0 && y >= 0 && x < image.width() as i64 && y < image.height() as i64 {
                    image.get_pixel(x as u32, y as u32)[channel] as f64
                } else {
                    0.0
                }
            };
            *value = ((1.0 - dx) * (1.0 - dy) * sample(x, y)
                + dx * (1.0 - dy) * sample(x + 1, y)
                + (1.0 - dx) * dy * sample(x, y + 1)
                + dx * dy * sample(x + 1, y + 1))
            .round()
            .clamp(0.0, 255.0) as u8;
        }
        image::Rgb(pixel)
    }))
}
pub fn infer_native(bytes: &[u8]) -> Result<Vec<Detection>> {
    let mut reader = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| ApiError::new(422, "Invalid image"))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(12000);
    limits.max_image_height = Some(12000);
    limits.max_alloc = Some(128 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader
        .into_decoder()
        .map_err(|_| ApiError::new(422, "Invalid image"))?;
    use image::ImageDecoder;
    let dimensions = decoder.dimensions();
    if dimensions.0 as u64 * dimensions.1 as u64 > 24_000_000 {
        return Err(ApiError::new(413, "Image too large"));
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    let mut image = image::DynamicImage::from_decoder(decoder)
        .map_err(|_| ApiError::new(422, "Invalid image"))?;
    image.apply_orientation(orientation);
    let image = image.to_rgb8();
    let scale = 640.0 / image.width().max(image.height()) as f64;
    let resized_width = (image.width() as f64 * scale).round().max(1.0) as u32;
    let resized_height = (image.height() as f64 * scale).round().max(1.0) as u32;
    let resized =
        image::imageops::resize(&image, resized_width, resized_height, FilterType::Triangle);
    let mut padded = RgbImage::new(640, 640);
    image::imageops::replace(&mut padded, &resized, 0, 0);
    let models = native_models()?;
    let models = models.lock().map_err(|_| ApiError::provider())?;
    let outputs = models
        .detector
        .run(tvec!(image_tensor(&padded, true).into()))
        .map_err(|_| ApiError::provider())?;
    tracing::debug!(output_names=?models.output_names,"Face detector outputs");
    let tensors: Vec<Vec<f32>> = outputs
        .iter()
        .map(|output| {
            output
                .to_array_view::<f32>()
                .map(|array| array.iter().copied().collect())
        })
        .collect::<TractResult<_>>()
        .map_err(|_| ApiError::provider())?;
    let output = |name: &str| -> Result<&[f32]> {
        models
            .output_names
            .iter()
            .position(|candidate| candidate == name)
            .and_then(|index| tensors.get(index))
            .map(Vec::as_slice)
            .ok_or_else(ApiError::provider)
    };
    let mut candidates = vec![];
    let sx = image.width() as f64 / resized_width as f64;
    let sy = image.height() as f64 / resized_height as f64;
    for stride in [8, 16, 32] {
        let classes = output(&format!("cls_{stride}"))?;
        let objects = output(&format!("obj_{stride}"))?;
        let boxes = output(&format!("bbox_{stride}"))?;
        let keypoints = output(&format!("kps_{stride}"))?;
        tracing::debug!(
            stride,
            maximum_score = classes
                .iter()
                .zip(objects)
                .map(|(class, object)| (class.clamp(0.0, 1.0) * object.clamp(0.0, 1.0)).sqrt())
                .fold(0.0f32, f32::max),
            "Face detector confidence"
        );
        let columns = 640 / stride;
        if classes.len() != columns * columns
            || objects.len() != classes.len()
            || boxes.len() != classes.len() * 4
            || keypoints.len() != classes.len() * 10
        {
            return Err(ApiError::provider());
        }
        for index in 0..classes.len() {
            let score = (classes[index].clamp(0.0, 1.0) * objects[index].clamp(0.0, 1.0)).sqrt();
            if score < 0.8 {
                continue;
            }
            let column = (index % columns) as f64;
            let row = (index / columns) as f64;
            let center_x = (column + boxes[index * 4] as f64) * stride as f64;
            let center_y = (row + boxes[index * 4 + 1] as f64) * stride as f64;
            let width = (boxes[index * 4 + 2] as f64).exp() * stride as f64;
            let height = (boxes[index * 4 + 3] as f64).exp() * stride as f64;
            let x = ((center_x - width / 2.0) * sx).max(0.0);
            let y = ((center_y - height / 2.0) * sy).max(0.0);
            let right = ((center_x + width / 2.0) * sx).min(image.width() as f64);
            let bottom = ((center_y + height / 2.0) * sy).min(image.height() as f64);
            let face_box = FaceBox {
                x,
                y,
                width: right - x,
                height: bottom - y,
            };
            if face_box.validate().is_err() {
                continue;
            }
            let landmarks = std::array::from_fn(|point| {
                [
                    (column + keypoints[index * 10 + point * 2] as f64) * stride as f64 * sx,
                    (row + keypoints[index * 10 + point * 2 + 1] as f64) * stride as f64 * sy,
                ]
            });
            candidates.push(Candidate {
                face_box,
                landmarks,
                score,
            });
        }
    }
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut selected: Vec<Candidate> = vec![];
    for candidate in candidates {
        if !selected
            .iter()
            .any(|other| overlap(&candidate.face_box, &other.face_box) > 0.3)
        {
            selected.push(candidate);
        }
    }
    if selected.len() > 32 {
        return Err(ApiError::new(422, "Too many faces"));
    }
    tracing::debug!(count = selected.len(), "Face candidates after suppression");
    let mut detections = vec![];
    for candidate in selected {
        let aligned = aligned_face(&image, &candidate.landmarks)?;
        let output = models
            .recognizer
            .run(tvec!(image_tensor(&aligned, false).into()))
            .map_err(|error| {
                tracing::error!(error=%error,"Face recognizer execution failed");
                ApiError::provider()
            })?;
        let mut descriptor: Vec<f64> = output[0]
            .to_array_view::<f32>()
            .map_err(|_| ApiError::provider())?
            .iter()
            .map(|value| *value as f64)
            .collect();
        let norm = descriptor
            .iter()
            .map(|value| value * value)
            .sum::<f64>()
            .sqrt();
        if norm <= f64::EPSILON || !norm.is_finite() {
            return Err(ApiError::provider());
        }
        descriptor.iter_mut().for_each(|value| *value /= norm);
        validate_descriptor(&descriptor)?;
        detections.push(Detection {
            face_box: candidate.face_box,
            descriptor,
        });
    }
    Ok(detections)
}
pub fn validate_descriptor(descriptor: &[f64]) -> Result<()> {
    if descriptor.len() == 128
        && descriptor.iter().all(|value| value.is_finite())
        && descriptor.iter().any(|value| *value != 0.0)
    {
        Ok(())
    } else {
        Err(ApiError::provider())
    }
}
pub async fn detect(db: &Database, image: &Upload) -> Result<Vec<Detection>> {
    let endpoint = std::env::var("KIN_FACE_SERVICE_URL").unwrap_or_default();
    if endpoint.is_empty() {
        if model() != NATIVE_MODEL {
            return Err(ApiError::new(
                503,
                "Configured face model requires its inference endpoint",
            ));
        }
        let bytes = image.bytes.clone();
        return tokio::task::spawn_blocking(move || infer_native(&bytes))
            .await
            .map_err(|_| ApiError::provider())?;
    }
    let token = std::env::var("KIN_FACE_SERVICE_TOKEN")
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| ApiError::new(503, "Face inference token required"))?;
    let response: Inference = db
        .client
        .post(endpoint)
        .bearer_auth(token)
        .header("content-type", &image.mime)
        .timeout(std::time::Duration::from_secs(20))
        .body(image.bytes.clone())
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if response.model != model()
        || response.width == 0
        || response.height == 0
        || response.width > 12000
        || response.height > 12000
        || response.faces.len() > 32
    {
        return Err(ApiError::provider());
    }
    for face in &response.faces {
        face.face_box.validate()?;
        validate_descriptor(&face.descriptor)?;
        if face.face_box.x + face.face_box.width > response.width as f64 + 1.0
            || face.face_box.y + face.face_box.height > response.height as f64 + 1.0
        {
            return Err(ApiError::provider());
        }
    }
    Ok(response.faces)
}
fn token_cipher() -> Result<Aes256Gcm> {
    let key = std::env::var("KIN_FACE_TOKEN_KEY")
        .ok()
        .and_then(|value| hex::decode(value).ok())
        .filter(|key| key.len() == 32)
        .ok_or_else(|| ApiError::new(503, "Face selection signing is not configured"))?;
    Aes256Gcm::new_from_slice(&key).map_err(|_| ApiError::provider())
}
fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}
pub fn seal_face(image: &Upload, detection: &Detection, identity: &Identity) -> Result<String> {
    let payload = json!({
        "familyId": identity.family_id,
        "contributorId": identity.contributor_id()?,
        "hash": digest(&image.bytes),
        "model": model(),
        "expires": now_millis()+900000,
        "box": detection.face_box,
        "descriptor": detection.descriptor
    });
    let mut nonce = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce);
    let encrypted = token_cipher()?
        .encrypt(Nonce::from_slice(&nonce), payload.to_string().as_bytes())
        .map_err(|_| ApiError::provider())?;
    let (ciphertext, tag) = encrypted.split_at(encrypted.len() - 16);
    let mut token = nonce.to_vec();
    token.extend_from_slice(tag);
    token.extend_from_slice(ciphertext);
    Ok(URL_SAFE_NO_PAD.encode(token))
}
pub fn unseal_face(image: &Upload, token: &str, identity: &Identity) -> Result<Vec<f64>> {
    let cipher = token_cipher()?;
    let validate = || -> Option<Vec<f64>> {
        if token.len() > 16000 {
            return None;
        }
        let token = URL_SAFE_NO_PAD.decode(token).ok()?;
        if token.len() < 29 {
            return None;
        }
        let mut encrypted = token[28..].to_vec();
        encrypted.extend_from_slice(&token[12..28]);
        let bytes = cipher
            .decrypt(Nonce::from_slice(&token[..12]), encrypted.as_slice())
            .ok()?;
        let payload: Value = serde_json::from_slice(&bytes).ok()?;
        if payload["hash"] != digest(&image.bytes)
            || payload["model"] != model()
            || payload["familyId"] != identity.family_id
            || payload["contributorId"] != identity.contributor_id().ok()?
            || payload["expires"].as_u64()? < now_millis() as u64
        {
            return None;
        }
        let descriptor: Vec<f64> = serde_json::from_value(payload["descriptor"].clone()).ok()?;
        validate_descriptor(&descriptor).ok()?;
        Some(descriptor)
    };
    validate()
        .ok_or_else(|| ApiError::new(422, "Invalid or expired face selection; detect faces again"))
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Enrollment {
    pub person_node_id: String,
    pub contributor_id: String,
    pub memory_id: String,
    pub family_id: Option<String>,
    #[serde(rename = "temporaryFaceId")]
    pub temporary_face_id: String,
    pub consent: bool,
}
pub async fn enroll(db: &Database, identity: &Identity, input: Enrollment) -> Result<Value> {
    crate::ingestion::valid_id(&input.person_node_id)?;
    crate::ingestion::valid_id(&input.memory_id)?;
    identity.assert_ownership(&input.contributor_id, input.family_id.as_deref())?;
    if !input.consent {
        return Err(ApiError::new(400, "Explicit consent required"));
    }
    let memories = db
        .select(
            "memories",
            &[
                ("id", eq(&input.memory_id)),
                ("family_id", eq(&identity.family_id)),
                ("contributor_id", eq(&input.contributor_id)),
                ("kind", "eq.photo".into()),
            ],
        )
        .await?;
    let persons = db
        .select(
            "graph_nodes",
            &[
                ("id", eq(&input.person_node_id)),
                ("family_id", eq(&identity.family_id)),
                ("type", "eq.person".into()),
            ],
        )
        .await?;
    let memory = memories
        .first()
        .filter(|memory| memory["media_path"].is_string() && persons.len() == 1)
        .ok_or_else(|| {
            ApiError::new(
                403,
                "Photo or person does not belong to this family and contributor",
            )
        })?;
    let (bytes, mime) = db.download(string(memory, "media_path")).await?;
    let descriptor = unseal_face(&Upload { bytes, mime }, &input.temporary_face_id, identity)?;
    let model = model();
    let id = stable_id(&[
        &identity.family_id,
        &input.contributor_id,
        &input.memory_id,
        "face",
        &model,
        &digest(json!(descriptor).to_string()),
    ]);
    let hash = digest(json!({"memory":input.memory_id,"person":input.person_node_id,"descriptor":descriptor,"consent":true,"model":model}).to_string());
    if let Some(response) = receipt(db, identity, &id, &hash).await? {
        return Ok(response);
    }
    db.rpc(
        "commit_ingestion",
        json!({
            "payload": {
                "id": id,
                "request_hash": hash,
                "family_id": identity.family_id,
                "contributor_id": input.contributor_id,
                "face": {
                    "id": id,
                    "family_id": identity.family_id,
                    "person_node_id": input.person_node_id,
                    "contributor_id": input.contributor_id,
                    "memory_id": input.memory_id,
                    "descriptor": descriptor,
                    "model": model
                },
                "source": {
                    "type": "human",
                    "user_id": identity.user["id"],
                    "consent": true,
                    "model": model
                },
                "response": {
                    "ok": true,
                    "face_id": id,
                    "model": model
                }
            }
        }),
    )
    .await
}
fn numeric_setting(name: &str, default: f64, range: std::ops::RangeInclusive<f64>) -> Result<f64> {
    let value = std::env::var(name)
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| value.parse::<f64>())
        .transpose()
        .map_err(|_| ApiError::provider())?
        .unwrap_or(default);
    if value.is_finite() && range.contains(&value) {
        Ok(value)
    } else {
        Err(ApiError::provider())
    }
}
pub async fn recognize(db: &Database, family: &str, image: &Upload) -> FaceOutcome {
    let model = model();
    let infer = async {
        let detections = detect(db, image).await?;
        if detections.is_empty() {
            return Ok(FaceOutcome::NoFace {
                model: model.clone(),
            });
        }
        if detections.len() != 1 {
            return Ok(FaceOutcome::Ambiguous {
                model: model.clone(),
            });
        }
        let max_distance = numeric_setting("KIN_FACE_MAX_DISTANCE", 0.45, 0.01..=0.6)?;
        let margin = numeric_setting("KIN_FACE_MIN_MARGIN", 0.1, 0.05..=0.5)?;
        let rows = db
            .select(
                "face_embeddings",
                &[("family_id", eq(family)), ("model", eq(&model))],
            )
            .await?;
        if rows.len() > 1000 {
            return Err(ApiError::provider());
        }
        let mut subjects: HashMap<String, (f64, Vec<String>)> = HashMap::new();
        for row in rows {
            if row["family_id"] != family || row["model"] != model {
                return Err(ApiError::provider());
            }
            let descriptor_value = if let Some(encoded) = row["descriptor"].as_str() {
                serde_json::from_str(encoded)?
            } else {
                row["descriptor"].clone()
            };
            let descriptor: Vec<f64> = serde_json::from_value(descriptor_value)?;
            validate_descriptor(&descriptor)?;
            let distance = descriptor
                .iter()
                .zip(&detections[0].descriptor)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                .sqrt();
            let subject = string(&row, "person_node_id");
            let id = string(&row, "id");
            if subject.is_empty() || id.is_empty() {
                return Err(ApiError::provider());
            }
            let entry = subjects
                .entry(subject.into())
                .or_insert((f64::INFINITY, vec![]));
            entry.0 = entry.0.min(distance);
            if distance <= max_distance {
                entry.1.push(id.into());
            }
        }
        let mut ranked: Vec<_> = subjects.into_iter().collect();
        ranked.sort_by(|a, b| a.1.0.total_cmp(&b.1.0));
        let Some((subject, (distance, ids))) = ranked.first() else {
            return Ok(FaceOutcome::Unknown {
                model: model.clone(),
            });
        };
        if *distance > max_distance {
            return Ok(FaceOutcome::Unknown {
                model: model.clone(),
            });
        }
        if ranked
            .get(1)
            .is_some_and(|(_, other)| other.0 - distance < margin)
        {
            return Ok(FaceOutcome::Ambiguous {
                model: model.clone(),
            });
        }
        let mut ids = ids.clone();
        ids.sort();
        Ok::<_, ApiError>(FaceOutcome::Matched {
            subject_node_id: subject.clone(),
            model: model.clone(),
            enrollment_ids: ids,
            distance: *distance,
            visual_confidence: crate::keepers::visual_confidence(*distance),
        })
    };
    infer.await.unwrap_or(FaceOutcome::Unavailable { model })
}
pub async fn download_models(db: &Database) -> Result<()> {
    let directory = std::env::var("KIN_FACE_WEIGHTS").unwrap_or_else(|_| "brain/models".into());
    std::fs::create_dir_all(&directory).map_err(|_| ApiError::provider())?;
    for (folder, file, expected_hash) in MODEL_ARTIFACTS {
        let response = db
            .client
            .get(format!(
                "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/{folder}/{file}"
            ))
            .timeout(std::time::Duration::from_secs(120))
            .send()
            .await?
            .error_for_status()?;
        let bytes = response.bytes().await?;
        if digest(&bytes) != expected_hash {
            return Err(ApiError::provider());
        }
        std::fs::write(format!("{directory}/{file}"), bytes).map_err(|_| ApiError::provider())?;
    }
    Ok(())
}
