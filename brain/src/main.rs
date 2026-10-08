use kin_brain::{api, database::Database};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = dotenvy::from_filename(".env.local");
    let _ = dotenvy::dotenv();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "kin_brain=info,tower_http=info".into()),
        )
        .init();
    let db = Database::from_env()?;
    match std::env::args().nth(1).as_deref() {
        Some("download-models") => {
            kin_brain::faces::download_models(&db).await?;
            return Ok(());
        }
        Some("verify-face-models") => {
            let path = std::env::args().nth(2).ok_or("An image path is required")?;
            let detections = kin_brain::faces::infer_native(&std::fs::read(path)?)?;
            println!(
                "{} faces detected; descriptor lengths: {:?}",
                detections.len(),
                detections
                    .iter()
                    .map(|face| face.descriptor.len())
                    .collect::<Vec<_>>()
            );
            return Ok(());
        }
        Some("seed") => {
            println!(
                "{}",
                kin_brain::admin::seed(&db, kin_brain::admin::DEMO_FAMILY).await?
            );
            return Ok(());
        }
        Some("provision-demo") => {
            let _ = dotenvy::from_filename(".env.demo.local");
            kin_brain::admin::provision_demo(&db, &std::env::var("KIN_DEMO_PASSWORD")?).await?;
            return Ok(());
        }
        Some("demo-audio") => {
            kin_brain::admin::demo_audio(
                &db,
                std::env::args().any(|argument| argument == "--apply"),
            )
            .await?;
            return Ok(());
        }
        Some(command) => return Err(format!("Unknown backend command: {command}").into()),
        None => {}
    }
    let bind = std::env::var("KIN_BACKEND_BIND").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("KIN_BRAIN_PORT")
        .unwrap_or_else(|_| "8787".into())
        .parse()?;
    let listener = tokio::net::TcpListener::bind((bind.as_str(), port)).await?;
    tracing::info!(address=%listener.local_addr()?,"Kin Rust backend listening");
    axum::serve(listener, api::router(db))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
}
