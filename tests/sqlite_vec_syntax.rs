use vectorcode::store::db::Database;

#[test]
fn test_vec_syntax() {
    let mut db = Database::open_in_memory().unwrap();
    // try different syntaxes
    let syntaxes = vec![
        "embedding float distance_metric=cosine",
        "+embedding float distance_metric=cosine",
        "embedding float[] distance_metric=cosine",
        "embedding float[+] distance_metric=cosine",
        "+embedding float[768] distance_metric=cosine"
    ];
    for syn in syntaxes {
        let sql = format!("CREATE VIRTUAL TABLE test_vec USING vec0({});", syn);
        match db.conn().execute_batch(&sql) {
            Ok(_) => println!("Syntax OK: {}", syn),
            Err(e) => println!("Syntax ERR for {}: {:?}", syn, e),
        }
        // cleanup
        let _ = db.conn().execute_batch("DROP TABLE IF EXISTS test_vec;");
    }
}
