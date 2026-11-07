//! Audit entries the database refuses are kept, in order, and written later (docs/security.md,
//! "Audit logging"): never lost while the action goes on.  (A test binary of its own: the queue
//! is the process's.)

mod common;

#[test]
fn a_refused_audit_entry_is_written_later_in_order() {
    let tmp = tempfile::tempdir().unwrap();
    let root = medorama::config::real(tmp.path());
    let mut cfg = common::config(&root.join("home"), &root.join("results"));
    cfg.audit_export = Some(root.join("audit.jsonl"));
    let db = medorama::db::Db::open(&cfg, true).unwrap();
    let c = db.get().unwrap();
    // a database that refuses the write (here: no audit table)
    let refusing = medorama::sql::Connection::open_in_memory().unwrap();
    medorama::db::audit(&refusing, None, "view", "first", "");
    let export = std::fs::read_to_string(root.join("audit.jsonl")).unwrap();
    assert!(export.contains(r#""pending":true"#) && export.contains("first"), "appended to audit_export at once");
    // the next entry the catalog takes: the waiting one first
    medorama::db::audit(&c, None, "view", "second", "");
    let mut st = c.prepare("SELECT target FROM audit WHERE event = 'view' ORDER BY id").unwrap();
    let targets: Vec<String> = st.query_map([], |r| r.get(0)).unwrap().map(Result::unwrap).collect();
    assert_eq!(targets, ["first", "second"]);
    assert_eq!(medorama::db::flush_audit(&c), 0, "nothing waits");
}
