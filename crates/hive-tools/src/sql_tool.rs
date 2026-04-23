use crate::{BoxFuture, Tool, ToolError, ToolResult};
use hive_classification::{ChannelClass, DataClass};
use hive_contracts::{ToolAnnotations, ToolApproval, ToolDefinition};
use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Mutex;

const MAX_RESULT_ROWS: usize = 500;

/// A per-session SQLite data store that agents can use for tabular data
/// analysis, aggregation, tracking, and any structured data workflow.
pub struct DataStoreTool {
    definition: ToolDefinition,
    _db_path: PathBuf,
    conn: Mutex<Connection>,
}

impl DataStoreTool {
    /// Returns the tool definition without opening a database connection.
    /// Used by the tools listing API for UI discovery.
    pub fn tool_definition() -> ToolDefinition {
        ToolDefinition {
            id: "core.data_store".to_string(),
            name: "Data Store".to_string(),
            description: concat!(
                "Execute SQL against a per-session SQLite database for structured data ",
                "analysis. Use this to create tables, insert/load data, run aggregation ",
                "queries, filter and join datasets, track todo lists, or any tabular data ",
                "workflow. The database persists for the lifetime of the session. ",
                "Supports full SQL: CREATE TABLE, INSERT, SELECT, UPDATE, DELETE, ",
                "WITH (CTEs), window functions, etc."
            )
            .to_string(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "SQL query to execute. Supports all SQLite SQL including DDL, DML, and queries."
                    },
                    "description": {
                        "type": "string",
                        "description": "Brief description of what this query does (for logging/debugging)."
                    },
                    "params": {
                        "type": "array",
                        "description": "Optional positional parameters for the query (bound as ?1, ?2, etc.).",
                        "items": {}
                    }
                },
                "required": ["query"]
            }),
            output_schema: Some(json!({
                "type": "object",
                "properties": {
                    "columns": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Column names (for SELECT queries)"
                    },
                    "rows": {
                        "type": "array",
                        "description": "Result rows (for SELECT queries)"
                    },
                    "row_count": {
                        "type": "number",
                        "description": "Number of rows returned or affected"
                    },
                    "truncated": {
                        "type": "boolean",
                        "description": "Whether results were truncated due to row limit"
                    }
                }
            })),
            channel_class: ChannelClass::Internal,
            side_effects: true,
            approval: ToolApproval::Auto,
            annotations: ToolAnnotations {
                title: "Data Store".to_string(),
                read_only_hint: Some(false),
                destructive_hint: Some(false),
                idempotent_hint: Some(false),
                open_world_hint: Some(false),
            },
        }
    }

    pub fn new(db_path: PathBuf) -> Result<Self, String> {
        // Ensure parent directory exists
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create data store directory: {e}"))?;
        }

        let conn =
            Connection::open(&db_path).map_err(|e| format!("failed to open data store: {e}"))?;

        // Enable WAL mode for better concurrency
        conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON;")
            .map_err(|e| format!("failed to configure data store: {e}"))?;

        Ok(Self {
            definition: Self::tool_definition(),
            _db_path: db_path,
            conn: Mutex::new(conn),
        })
    }
}

fn is_returning_query(query: &str) -> bool {
    let mut s = query.trim();
    loop {
        s = s.trim_start();
        if s.starts_with("--") {
            if let Some(pos) = s.find('\n') {
                s = &s[pos + 1..];
            } else {
                return false;
            }
        } else if s.starts_with("/*") {
            if let Some(pos) = s.find("*/") {
                s = &s[pos + 2..];
            } else {
                return false;
            }
        } else {
            break;
        }
    }
    let first_keyword = s.split_ascii_whitespace().next().unwrap_or("").to_ascii_uppercase();
    matches!(first_keyword.as_str(), "SELECT" | "PRAGMA" | "EXPLAIN" | "WITH")
}

fn sqlite_value_to_json(val: SqlValue) -> Value {
    match val {
        SqlValue::Null => Value::Null,
        SqlValue::Integer(i) => json!(i),
        SqlValue::Real(f) => json!(f),
        SqlValue::Text(s) => json!(s),
        SqlValue::Blob(b) => {
            let hex: String = b.iter().map(|byte| format!("{byte:02x}")).collect();
            json!(format!("hex:{hex}"))
        }
    }
}

fn json_to_rusqlite_value(val: &Value) -> SqlValue {
    match val {
        Value::Null => SqlValue::Null,
        Value::Bool(b) => SqlValue::Integer(if *b { 1 } else { 0 }),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                SqlValue::Integer(i)
            } else if let Some(f) = n.as_f64() {
                SqlValue::Real(f)
            } else {
                SqlValue::Text(n.to_string())
            }
        }
        Value::String(s) => SqlValue::Text(s.clone()),
        _ => SqlValue::Text(val.to_string()),
    }
}

impl Tool for DataStoreTool {
    fn definition(&self) -> &ToolDefinition {
        &self.definition
    }

    fn execute(&self, input: Value) -> BoxFuture<'_, Result<ToolResult, ToolError>> {
        Box::pin(async move {
            let query = input.get("query").and_then(|v| v.as_str()).ok_or_else(|| {
                ToolError::InvalidInput("missing required field `query`".to_string())
            })?;

            let params: Vec<Value> =
                input.get("params").and_then(|v| v.as_array()).cloned().unwrap_or_default();

            let conn = self.conn.lock().map_err(|e| {
                ToolError::ExecutionFailed(format!("failed to acquire database lock: {e}"))
            })?;

            let rusqlite_params: Vec<SqlValue> =
                params.iter().map(json_to_rusqlite_value).collect();
            let param_refs: Vec<&dyn rusqlite::types::ToSql> =
                rusqlite_params.iter().map(|v| v as &dyn rusqlite::types::ToSql).collect();

            if is_returning_query(query) {
                // SELECT / PRAGMA / EXPLAIN / WITH — return rows
                let mut stmt = conn.prepare(query).map_err(|e| {
                    ToolError::ExecutionFailed(format!("failed to prepare query: {e}"))
                })?;

                let columns: Vec<String> =
                    stmt.column_names().iter().map(|s| s.to_string()).collect();

                let mut rows = stmt.query(param_refs.as_slice()).map_err(|e| {
                    ToolError::ExecutionFailed(format!("query execution failed: {e}"))
                })?;

                let mut rows_out: Vec<Value> = Vec::new();
                let mut truncated = false;

                while let Some(row) = rows
                    .next()
                    .map_err(|e| ToolError::ExecutionFailed(format!("error reading row: {e}")))?
                {
                    if rows_out.len() >= MAX_RESULT_ROWS {
                        truncated = true;
                        break;
                    }
                    let mut row_values: Vec<Value> = Vec::with_capacity(columns.len());
                    for i in 0..columns.len() {
                        let val: SqlValue = row.get(i).map_err(|e| {
                            ToolError::ExecutionFailed(format!("error reading column {i}: {e}"))
                        })?;
                        row_values.push(sqlite_value_to_json(val));
                    }
                    rows_out.push(Value::Array(row_values));
                }

                let row_count = rows_out.len();

                Ok(ToolResult {
                    output: json!({
                        "columns": columns,
                        "rows": rows_out,
                        "row_count": row_count,
                        "truncated": truncated,
                    }),
                    data_class: DataClass::Internal,
                })
            } else {
                // DDL / DML — execute and return rows affected
                let changes = conn.execute(query, param_refs.as_slice()).map_err(|e| {
                    ToolError::ExecutionFailed(format!("query execution failed: {e}"))
                })?;

                Ok(ToolResult {
                    output: json!({
                        "row_count": changes,
                        "message": format!("{changes} row(s) affected"),
                    }),
                    data_class: DataClass::Internal,
                })
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_tool() -> (tempfile::TempDir, DataStoreTool) {
        let dir = tempfile::tempdir().expect("failed to create temp dir");
        let db_path = dir.path().join("test_store.db");
        let tool = DataStoreTool::new(db_path).expect("failed to create DataStoreTool");
        (dir, tool)
    }

    #[tokio::test]
    async fn create_table_and_insert() {
        let (_dir, tool) = setup_tool();

        let result = tool
            .execute(json!({
                "query": "CREATE TABLE items (id INTEGER PRIMARY KEY, name TEXT, price REAL)"
            }))
            .await
            .expect("CREATE TABLE failed");
        assert_eq!(result.output["row_count"], 0);

        let result = tool
            .execute(json!({
                "query": "INSERT INTO items (name, price) VALUES (?1, ?2)",
                "params": ["Widget", 9.99]
            }))
            .await
            .expect("INSERT failed");
        assert_eq!(result.output["row_count"], 1);
    }

    #[tokio::test]
    async fn select_returns_columns_and_rows() {
        let (_dir, tool) = setup_tool();

        tool.execute(json!({
            "query": "CREATE TABLE users (id INTEGER PRIMARY KEY, name TEXT, score REAL)"
        }))
        .await
        .unwrap();

        tool.execute(json!({
            "query": "INSERT INTO users VALUES (1, 'Alice', 95.5), (2, 'Bob', 82.0), (3, 'Charlie', NULL)"
        }))
        .await
        .unwrap();

        let result = tool
            .execute(json!({
                "query": "SELECT id, name, score FROM users ORDER BY id",
                "description": "Get all users"
            }))
            .await
            .expect("SELECT failed");

        assert_eq!(result.output["columns"], json!(["id", "name", "score"]));
        assert_eq!(result.output["row_count"], 3);
        assert_eq!(result.output["truncated"], false);
        let rows = result.output["rows"].as_array().unwrap();
        assert_eq!(rows[0], json!([1, "Alice", 95.5]));
        assert_eq!(rows[2], json!([3, "Charlie", null]));
    }

    #[tokio::test]
    async fn aggregation_queries() {
        let (_dir, tool) = setup_tool();

        tool.execute(json!({
            "query": "CREATE TABLE sales (product TEXT, amount REAL, region TEXT)"
        }))
        .await
        .unwrap();

        tool.execute(json!({
            "query": "INSERT INTO sales VALUES ('A', 100, 'US'), ('B', 200, 'US'), ('A', 150, 'EU'), ('B', 50, 'EU')"
        }))
        .await
        .unwrap();

        let result = tool
            .execute(json!({
                "query": "SELECT product, SUM(amount) as total, COUNT(*) as cnt FROM sales GROUP BY product ORDER BY total DESC"
            }))
            .await
            .unwrap();

        assert_eq!(result.output["columns"], json!(["product", "total", "cnt"]));
        let rows = result.output["rows"].as_array().unwrap();
        assert_eq!(rows[0], json!(["B", 250.0, 2]));
        assert_eq!(rows[1], json!(["A", 250.0, 2]));
    }

    #[tokio::test]
    async fn update_and_delete() {
        let (_dir, tool) = setup_tool();

        tool.execute(json!({ "query": "CREATE TABLE t (id INTEGER PRIMARY KEY, v TEXT)" }))
            .await
            .unwrap();
        tool.execute(json!({ "query": "INSERT INTO t VALUES (1, 'a'), (2, 'b'), (3, 'c')" }))
            .await
            .unwrap();

        let result =
            tool.execute(json!({ "query": "UPDATE t SET v = 'x' WHERE id > 1" })).await.unwrap();
        assert_eq!(result.output["row_count"], 2);

        let result = tool.execute(json!({ "query": "DELETE FROM t WHERE v = 'x'" })).await.unwrap();
        assert_eq!(result.output["row_count"], 2);

        let result =
            tool.execute(json!({ "query": "SELECT COUNT(*) as cnt FROM t" })).await.unwrap();
        assert_eq!(result.output["rows"], json!([[1]]));
    }

    #[tokio::test]
    async fn cte_and_window_functions() {
        let (_dir, tool) = setup_tool();

        tool.execute(json!({ "query": "CREATE TABLE scores (name TEXT, score INT)" }))
            .await
            .unwrap();
        tool.execute(json!({
            "query": "INSERT INTO scores VALUES ('A', 10), ('B', 20), ('C', 15)"
        }))
        .await
        .unwrap();

        let result = tool
            .execute(json!({
                "query": "WITH ranked AS (SELECT name, score, ROW_NUMBER() OVER (ORDER BY score DESC) as rank FROM scores) SELECT * FROM ranked"
            }))
            .await
            .unwrap();

        let rows = result.output["rows"].as_array().unwrap();
        assert_eq!(rows[0], json!(["B", 20, 1]));
    }

    #[tokio::test]
    async fn params_binding() {
        let (_dir, tool) = setup_tool();

        tool.execute(json!({ "query": "CREATE TABLE t (id INT, name TEXT)" })).await.unwrap();
        tool.execute(json!({
            "query": "INSERT INTO t VALUES (?1, ?2)",
            "params": [42, "test"]
        }))
        .await
        .unwrap();

        let result = tool
            .execute(json!({
                "query": "SELECT * FROM t WHERE id = ?1",
                "params": [42]
            }))
            .await
            .unwrap();
        assert_eq!(result.output["rows"], json!([[42, "test"]]));
    }

    #[tokio::test]
    async fn invalid_sql_returns_error() {
        let (_dir, tool) = setup_tool();
        let err = tool.execute(json!({ "query": "INVALID SQL HERE" })).await.unwrap_err();
        assert!(err.to_string().contains("query execution failed"));
    }

    #[tokio::test]
    async fn pragma_works() {
        let (_dir, tool) = setup_tool();
        tool.execute(json!({ "query": "CREATE TABLE t (a INT, b TEXT)" })).await.unwrap();

        let result = tool.execute(json!({ "query": "PRAGMA table_info('t')" })).await.unwrap();
        assert!(result.output["row_count"].as_u64().unwrap() >= 2);
    }

    #[test]
    fn is_returning_query_classification() {
        assert!(is_returning_query("SELECT * FROM t"));
        assert!(is_returning_query("  select 1  "));
        assert!(is_returning_query("WITH cte AS (SELECT 1) SELECT * FROM cte"));
        assert!(is_returning_query("EXPLAIN SELECT 1"));
        assert!(is_returning_query("PRAGMA table_info('users')"));

        assert!(!is_returning_query("INSERT INTO t VALUES (1)"));
        assert!(!is_returning_query("UPDATE t SET x=1"));
        assert!(!is_returning_query("DELETE FROM t"));
        assert!(!is_returning_query("DROP TABLE t"));
        assert!(!is_returning_query("CREATE TABLE t (id INT)"));
    }
}
