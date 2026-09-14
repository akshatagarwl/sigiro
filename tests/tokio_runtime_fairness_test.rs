use std::time::Duration;

use sigiro::workload::{WorkloadBudget, WorkloadClass};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mixed_blocking_load_is_bounded_without_starving_tokio() {
    let budget = WorkloadBudget::new();
    let mut permits = (0..budget.global_capacity())
        .map(|_| budget.try_acquire(WorkloadClass::Transform).unwrap())
        .collect::<Vec<_>>();
    let active_permit = permits.pop().expect("global capacity is at least one");
    let (started_tx, started_rx) = tokio::sync::oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(0);

    let task = tokio::task::spawn_blocking(move || {
        let _permit = active_permit;
        started_tx.send(()).expect("test is still running");
        release_rx.recv().expect("test releases blocking work");
    });
    started_rx.await.unwrap();

    assert!(budget.try_acquire(WorkloadClass::Query).is_err());
    assert!(budget.try_acquire(WorkloadClass::Transform).is_err());

    tokio::time::timeout(Duration::from_secs(1), async {
        for _ in 0..10 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("Tokio timers must keep advancing while admitted blocking work is saturated");

    release_tx.send(()).unwrap();
    task.await.unwrap();
    drop(permits);
    assert_eq!(budget.in_use(WorkloadClass::Transform), 0);
}
