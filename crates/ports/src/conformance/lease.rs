use std::time::Duration;

use crate::{LeaseError, Leases, MAX_LEASE_TTL};

const TTL: Duration = Duration::from_millis(100);

/// Run in `#[tokio::test(start_paused = true)]` for in-process implementations.
///
/// - One holder per name; other names are independent; release frees the name at once.
/// - Names and ttls are validated.
/// - A lease expires after its ttl; the old holder can neither renew nor release it, and releasing a lost
///   lease never frees the new holder's.
/// - `renew` extends; dropping a guard without releasing keeps the lease until it expires.
pub async fn leases<L: Leases + ?Sized>(l: &L) {
    let long = Duration::from_secs(60);

    let a = l.try_acquire("job.a", long).await.unwrap().expect("free name");
    assert_eq!(a.name(), "job.a");
    assert!(a.is_valid());
    assert!(
        l.try_acquire("job.a", long).await.unwrap().is_none(),
        "second holder is refused"
    );
    let b = l.try_acquire("job.b", long).await.unwrap();
    assert!(b.is_some(), "other names are independent");

    a.release().await.unwrap();
    let a2 = l.try_acquire("job.a", long).await.unwrap();
    assert!(a2.is_some(), "released names are free at once");

    assert_eq!(
        l.try_acquire("", long).await.unwrap_err(),
        LeaseError::InvalidName
    );
    assert_eq!(
        l.try_acquire("Bad Name", long).await.unwrap_err(),
        LeaseError::InvalidName
    );
    assert_eq!(
        l.try_acquire(&"x".repeat(129), long).await.unwrap_err(),
        LeaseError::InvalidName
    );
    assert_eq!(
        l.try_acquire("job.t", Duration::ZERO).await.unwrap_err(),
        LeaseError::InvalidTtl
    );
    assert_eq!(
        l.try_acquire("job.t", MAX_LEASE_TTL + Duration::from_secs(1))
            .await
            .unwrap_err(),
        LeaseError::InvalidTtl
    );

    // Expiry and loss.
    let mut old = l.try_acquire("job.c", TTL).await.unwrap().expect("free name");
    tokio::time::sleep(TTL * 2).await;
    assert!(!old.is_valid(), "the guard knows it expired");
    let new = l
        .try_acquire("job.c", long)
        .await
        .unwrap()
        .expect("an expired lease can be taken");
    assert_eq!(
        old.renew(long).await,
        Err(LeaseError::Lost),
        "the old holder cannot renew"
    );
    assert_eq!(
        old.release().await,
        Err(LeaseError::Lost),
        "the old holder cannot release"
    );
    assert!(
        l.try_acquire("job.c", long).await.unwrap().is_none(),
        "releasing a lost lease does not free the new holder's"
    );
    drop(new);

    // Renewal.
    let mut r = l.try_acquire("job.r", TTL).await.unwrap().expect("free name");
    tokio::time::sleep(TTL * 6 / 10).await;
    r.renew(TTL).await.unwrap();
    tokio::time::sleep(TTL * 6 / 10).await;
    assert!(r.is_valid(), "renewal pushed the expiry out");
    assert!(
        l.try_acquire("job.r", long).await.unwrap().is_none(),
        "still held after the original ttl"
    );

    // Dropping does not release.
    let d = l.try_acquire("job.d", TTL).await.unwrap().expect("free name");
    drop(d);
    assert!(
        l.try_acquire("job.d", long).await.unwrap().is_none(),
        "a dropped guard keeps the lease"
    );
    tokio::time::sleep(TTL * 2).await;
    assert!(
        l.try_acquire("job.d", long).await.unwrap().is_some(),
        "until it expires"
    );
}
