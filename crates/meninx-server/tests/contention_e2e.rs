//! Lock contention under load: `repo.list` runs git for every registered repository. While
//! clients keep asking for it, terminal input / resize and the session list must stay fast
//! (they used to wait for the session store lock that `repo.list` held across its git runs).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::*;
use meninx_client::{Client, ClientOptions, NewSession};

const REPOS: usize = 12;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn terminal_and_sessions_stay_fast_while_repos_are_listed() {
    let dir = tmp_dir("contention-e2e");
    let server = server(&dir.join("data"));
    let c = Client::connect(memory(&server), ClientOptions::default())
        .await
        .unwrap();
    let mut ids = Vec::new();
    for i in 0..REPOS {
        let r = dir.join("src").join(format!("r{i}"));
        init_repo(&r);
        ids.push(c.register_repo(&r.display().to_string()).await.unwrap().id);
    }
    let (s, _) = c
        .create_session(NewSession {
            provider: "shell".into(),
            account: None,
            configuration: "default".into(),
            repos: ids[..1].to_vec(),
            original_repo: None,
            refresh_base: false,
            background: false,
            bases: Default::default(),
            title: String::new(),
            start: true,
            cols: 80,
            rows: 24,
        })
        .await
        .unwrap();

    let t = Instant::now();
    let listed = c.repos().await.unwrap();
    assert_eq!(listed.len(), REPOS);
    assert!(listed.iter().all(|r| r.base == "main"), "{listed:?}");
    eprintln!("repo.list alone: {:?}", t.elapsed());

    // Three other clients keep listing repositories (an app reacting to ReposChanged).
    let stop = Arc::new(AtomicBool::new(false));
    let mut busy = Vec::new();
    for _ in 0..3 {
        let other = Client::connect(memory(&server), ClientOptions::default())
            .await
            .unwrap();
        let stop = stop.clone();
        busy.push(tokio::spawn(async move {
            let mut n = 0u32;
            while !stop.load(Ordering::Relaxed) {
                other.repos().await.unwrap();
                n += 1;
            }
            n
        }));
    }
    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut worst = (Duration::ZERO, Duration::ZERO, Duration::ZERO);
    for i in 0..20u16 {
        let t = Instant::now();
        c.resize(&s.id, 80 + (i % 2), 24).await.unwrap();
        worst.0 = worst.0.max(t.elapsed());
        let t = Instant::now();
        c.write(&s.id, b" ".to_vec()).await.unwrap();
        worst.1 = worst.1.max(t.elapsed());
        let t = Instant::now();
        c.sessions().await.unwrap();
        worst.2 = worst.2.max(t.elapsed());
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    stop.store(true, Ordering::Relaxed);
    let mut lists = 0;
    for b in busy {
        lists += b.await.unwrap();
    }
    eprintln!(
        "under {lists} repo.list calls: worst resize {:?}, write {:?}, session.list {:?}",
        worst.0, worst.1, worst.2
    );
    let limit = Duration::from_millis(500);
    assert!(worst.0 < limit, "term.resize took {:?}", worst.0);
    assert!(worst.1 < limit, "term.write took {:?}", worst.1);
    assert!(worst.2 < limit, "session.list took {:?}", worst.2);
    let _ = c.stop_session(&s.id).await;
}
