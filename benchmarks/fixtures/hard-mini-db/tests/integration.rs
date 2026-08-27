use hard_mini_db::{Db, DbError};

#[test]
fn put_get_basic() {
    let mut db = Db::open();
    db.put(1, "a", "1").unwrap();
    assert_eq!(db.get(1, "a"), Some("1"));
}

#[test]
fn rev_increments_on_each_put() {
    let mut db = Db::open();
    db.put(1, "a", "1").unwrap();
    assert_eq!(db.rev(1), Some(1));
    db.put(1, "a", "2").unwrap();
    assert_eq!(db.rev(1), Some(2));
    db.put(1, "b", "x").unwrap();
    assert_eq!(db.rev(1), Some(3));
}

#[test]
fn rev_increments_on_delete() {
    let mut db = Db::open();
    db.put(1, "a", "1").unwrap();
    let r0 = db.rev(1).unwrap();
    assert!(db.delete(1, "a").unwrap());
    assert_eq!(db.rev(1), Some(r0 + 1));
    assert_eq!(db.get(1, "a"), None);
}

#[test]
fn commit_then_uncommitted_ops_discarded_on_recover() {
    let mut db = Db::open();
    db.put(1, "a", "durable").unwrap();
    db.commit();
    db.put(1, "b", "volatile").unwrap();
    // no commit
    db.recover_durable();
    assert_eq!(db.get(1, "a"), Some("durable"));
    assert_eq!(db.get(1, "b"), None, "unfenced write must vanish after recover_durable");
}

#[test]
fn cas_requires_matching_rev() {
    let mut db = Db::open();
    db.put(1, "a", "1").unwrap();
    let rev = db.rev(1).unwrap();
    // wrong rev must fail
    match db.put_cas(1, "a", "x", rev + 10) {
        Err(DbError::Conflict { .. }) => {}
        other => panic!("expected conflict, got {other:?}"),
    }
    // correct rev must succeed
    db.put_cas(1, "a", "2", rev).unwrap();
    assert_eq!(db.get(1, "a"), Some("2"));
}

#[test]
fn checkpoint_restore_roundtrip() {
    let mut db = Db::open();
    db.put(1, "a", "snap").unwrap();
    let seq_before = db.seq();
    db.checkpoint();
    db.put(1, "a", "changed").unwrap();
    db.restore_checkpoint().unwrap();
    assert_eq!(db.get(1, "a"), Some("snap"));
    // seq should resume at the checkpointed seq, not an inflated value
    assert_eq!(db.seq(), seq_before);
}

#[test]
fn multi_page_isolation() {
    let mut db = Db::open();
    db.put(1, "k", "p1").unwrap();
    db.put(2, "k", "p2").unwrap();
    assert_eq!(db.get(1, "k"), Some("p1"));
    assert_eq!(db.get(2, "k"), Some("p2"));
}
