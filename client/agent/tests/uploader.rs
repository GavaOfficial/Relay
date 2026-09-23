use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicU32, AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::put,
    Router,
};
use relay_agent::uploader::{UploadConfig, Uploader};
use tokio::{net::TcpListener, sync::watch};

#[derive(Clone, Default)]
struct Counters {
    inflight: Arc<AtomicU32>,
    max_inflight: Arc<AtomicU32>,
    received: Arc<AtomicU64>,
}

async fn put_segment(
    State(c): State<Counters>,
    Path((_id, _pid, _file)): Path<(String, String, String)>,
) -> StatusCode {
    let now = c.inflight.fetch_add(1, Ordering::SeqCst) + 1;
    c.max_inflight.fetch_max(now, Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(40)).await;
    c.inflight.fetch_sub(1, Ordering::SeqCst);
    c.received.fetch_add(1, Ordering::SeqCst);
    StatusCode::NO_CONTENT
}

async fn spawn_mock() -> (String, Counters) {
    let counters = Counters::default();
    let app = Router::new()
        .route(
            "/api/matches/{id}/players/{pid}/segments/{file}",
            put(put_segment),
        )
        .with_state(counters.clone());
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (url, counters)
}

#[tokio::test]
async fn once_recording_is_done_the_backlog_uploads_in_parallel_not_one_by_one() {
    let (base_url, counters) = spawn_mock().await;
    let dir = tempfile::tempdir().unwrap();

    const N: u64 = 6;
    for n in 0..N {
        std::fs::write(dir.path().join(format!("seg_{n}.ts")), vec![b'x'; 128]).unwrap();
    }

    let cfg = UploadConfig {
        base_url,
        token: "t".into(),
        match_id: "m1".into(),
        player: "alice".into(),
        dir: PathBuf::from(dir.path()),
        limit_bytes_per_sec: None,
    };
    let uploader = Uploader::new(cfg).unwrap();

    // La registrazione e' gia' finita: niente da proteggere, la coda deve svuotarsi
    // con piu' richieste in volo insieme, non una alla volta.
    let (_tx, rx) = watch::channel(true);
    uploader.run(rx).await.unwrap();

    assert_eq!(
        counters.received.load(Ordering::SeqCst),
        N,
        "tutti i segmenti devono essere caricati"
    );
    assert!(
        counters.max_inflight.load(Ordering::SeqCst) > 1,
        "a registrazione finita gli upload devono sovrapporsi, non uno alla volta"
    );
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "i segmenti caricati vanno cancellati dal disco"
    );
}

#[tokio::test]
async fn the_queue_shrinks_as_each_upload_finishes_not_all_at_once_at_the_end() {
    let (base_url, _counters) = spawn_mock().await;
    let dir = tempfile::tempdir().unwrap();
    let dir_path: PathBuf = dir.path().to_path_buf();

    const N: u64 = 12;
    for n in 0..N {
        std::fs::write(dir_path.join(format!("seg_{n}.ts")), vec![b'x'; 128]).unwrap();
    }

    let cfg = UploadConfig {
        base_url,
        token: "t".into(),
        match_id: "m1".into(),
        player: "alice".into(),
        dir: dir_path.clone(),
        limit_bytes_per_sec: None,
    };
    let uploader = Uploader::new(cfg).unwrap();
    let (_tx, rx) = watch::channel(true);

    let handle = tokio::spawn(async move { uploader.run(rx).await });

    let mut saw_partial_progress = false;
    for _ in 0..100 {
        let remaining = std::fs::read_dir(&dir_path).unwrap().count();
        if remaining > 0 && (remaining as u64) < N {
            saw_partial_progress = true;
            break;
        }
        if handle.is_finished() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    handle.await.unwrap().unwrap();

    assert!(
        saw_partial_progress,
        "la coda deve svuotarsi mano a mano che ogni upload finisce, non tutta insieme alla fine"
    );
    assert_eq!(std::fs::read_dir(&dir_path).unwrap().count(), 0);
}
