//! MSSQL BoM lookup through the same ODBC settings and query file as the sidecar.
//! JSON and the job-card API stay in `bom.rs`.

use odbc_api::{ConnectionOptions, Cursor, IntoParameter, ResultSetMetadata, environment};
use serde_json::{Map, Value, json};

use crate::local_config;

const DEFAULT_SQL_DRIVER: &str = "ODBC Driver 18 for SQL Server";
const QUERY_FILE: &str = "bom_queries.sql";
const QUERY_EXAMPLE: &str = "bom_queries.example.sql";

pub fn inspect(barcode: &str, config: &Value) -> Value {
    let source = config
        .get("source")
        .and_then(|value| value.as_str())
        .unwrap_or("json");
    if source != "mssql" {
        return fail(barcode, "SQL BoM lookup is not the selected source.");
    }
    let mssql = match config.get("mssql").filter(|value| value.is_object()) {
        Some(value) => value,
        None => {
            return fail(
                barcode,
                "MSSQL server and database are required. Save them on the Settings page or in vision/config.local.json.",
            );
        }
    };
    let password = text(mssql, "password");
    match inspect_mssql(barcode, mssql) {
        Ok(value) => value,
        Err(err) => fail(barcode, &scrub(&err, &password)),
    }
}

fn inspect_mssql(barcode: &str, mssql: &Value) -> Result<Value, String> {
    let (header_sql, parts_sql) = load_queries()?;
    let connection_string = connection_string(mssql)?;
    let env = environment().map_err(|err| format!("Could not query MSSQL: {err}"))?;
    let connection = env
        .connect_with_connection_string(
            &connection_string,
            ConnectionOptions {
                login_timeout_sec: Some(5),
                ..ConnectionOptions::default()
            },
        )
        .map_err(|err| format!("Could not query MSSQL: {err}"))?;
    let _ = connection.set_autocommit(true);
    let header_rows = query_rows(&connection, &header_sql, barcode)?;
    if header_rows.is_empty() {
        return Ok(fail(barcode, "Barcode not found in BoM"));
    }
    let part_rows = query_rows(&connection, &parts_sql, barcode)?;
    Ok(pass(barcode, &job_from_rows(&header_rows[0], &part_rows)))
}

fn query_rows(
    connection: &odbc_api::Connection<'_>,
    sql: &str,
    kit: &str,
) -> Result<Vec<Map<String, Value>>, String> {
    let bound = bind_kit_ic(sql);
    let marks = bound.matches('?').count();
    let cursor = match marks {
        0 => connection.execute(&bound, (), Some(5)),
        1 => connection.execute(&bound, &kit.into_parameter(), Some(5)),
        2 => connection.execute(
            &bound,
            (&kit.into_parameter(), &kit.into_parameter()),
            Some(5),
        ),
        3 => connection.execute(
            &bound,
            (
                &kit.into_parameter(),
                &kit.into_parameter(),
                &kit.into_parameter(),
            ),
            Some(5),
        ),
        4 => connection.execute(
            &bound,
            (
                &kit.into_parameter(),
                &kit.into_parameter(),
                &kit.into_parameter(),
                &kit.into_parameter(),
            ),
            Some(5),
        ),
        _ => {
            return Err(
                "BoM query has more than four @kitIC placeholders. Use four or fewer.".to_string(),
            );
        }
    }
    .map_err(|err| format!("Could not query MSSQL: {err}"))?;
    let Some(mut cursor) = cursor else {
        return Ok(Vec::new());
    };
    let column_count = cursor
        .num_result_cols()
        .map_err(|err| format!("Could not query MSSQL: {err}"))? as u16;
    let mut names = Vec::with_capacity(column_count as usize);
    for index in 1..=column_count {
        names.push(
            cursor
                .col_name(index)
                .map_err(|err| format!("Could not query MSSQL: {err}"))?,
        );
    }
    use odbc_api::buffers::TextRowSet;
    let mut buffers = TextRowSet::for_cursor(64, &mut cursor, Some(4096))
        .map_err(|err| format!("Could not query MSSQL: {err}"))?;
    let mut rows = Vec::new();
    let mut bound = cursor
        .bind_buffer(&mut buffers)
        .map_err(|err| format!("Could not query MSSQL: {err}"))?;
    while let Some(batch) = bound
        .fetch()
        .map_err(|err| format!("Could not query MSSQL: {err}"))?
    {
        for row_index in 0..batch.num_rows() {
            let mut row = Map::new();
            for (column, name) in names.iter().enumerate() {
                let text = batch
                    .at(column, row_index)
                    .map(|bytes| String::from_utf8_lossy(bytes).trim().to_string());
                if let Some(text) = text {
                    if !text.is_empty() {
                        row.insert(name.clone(), Value::String(text));
                    }
                }
            }
            rows.push(row);
        }
    }
    Ok(rows)
}

fn job_from_rows(header: &Map<String, Value>, parts: &[Map<String, Value>]) -> Value {
    let mut part_list = Vec::new();
    for row in parts {
        let ic = row_text(row, &["ic", "cc"]);
        if ic.is_empty() {
            continue;
        }
        let qty = row_number(row, &["qty", "quantity"]).unwrap_or(0.0);
        part_list.push(json!({
            "ic": ic,
            "des": row_text(row, &["des", "description"]),
            "qty": qty,
        }));
    }
    json!({
        "des": row_text(header, &["des", "kitname", "kit_name", "description"]),
        "expectedKitCount": row_number(header, &["expectedkitcount", "expected_kit_count", "labelcount"]),
        "parts": part_list,
    })
}

fn row_text(row: &Map<String, Value>, names: &[&str]) -> String {
    let Some(value) = row_value(row, names) else {
        return String::new();
    };
    match value {
        Value::String(text) => text.trim().to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn row_number(row: &Map<String, Value>, names: &[&str]) -> Option<f64> {
    let value = row_value(row, names)?;
    match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse::<f64>().ok(),
        Value::Bool(flag) => Some(if *flag { 1.0 } else { 0.0 }),
        _ => None,
    }
}

fn row_value<'a>(row: &'a Map<String, Value>, names: &[&str]) -> Option<&'a Value> {
    for name in names {
        for (key, value) in row {
            if key.eq_ignore_ascii_case(name) {
                return Some(value);
            }
        }
    }
    None
}

fn pass(barcode: &str, job: &Value) -> Value {
    json!({
        "barcode": barcode,
        "status": "pass",
        "message": "BoM loaded – 2 kit(s) on bench",
        "kitCode": barcode,
        "des": job.get("des").cloned().unwrap_or(Value::Null),
        "expectedKitCount": job.get("expectedKitCount").cloned().unwrap_or(Value::Null),
        "parts": job.get("parts").cloned().unwrap_or_else(|| json!([])),
        "kitsFound": 2,
    })
}

fn fail(barcode: &str, message: &str) -> Value {
    json!({
        "barcode": barcode,
        "status": "fail",
        "message": message,
        "kits": [],
    })
}

fn text(value: &Value, name: &str) -> String {
    value
        .get(name)
        .and_then(|item| item.as_str())
        .unwrap_or("")
        .trim()
        .to_string()
}

fn connection_string(mssql: &Value) -> Result<String, String> {
    let server = text(mssql, "server");
    let database = text(mssql, "database");
    if server.is_empty() || database.is_empty() {
        return Err(
            "MSSQL server and database are required. Save them on the Settings page or in vision/config.local.json."
                .to_string(),
        );
    }
    let auth = text(mssql, "auth");
    let auth = if auth.is_empty() { "windows" } else { &auth };
    let user = text(mssql, "user");
    let password = mssql
        .get("password")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    if auth == "sql" && (user.is_empty() || password.trim().is_empty()) {
        return Err(
            "Username and password are required for SQL Server Authentication.".to_string(),
        );
    }
    let mut driver = text(mssql, "driver");
    if driver.is_empty() {
        driver = DEFAULT_SQL_DRIVER.to_string();
    }
    let mut parts = vec![
        format!("DRIVER={}", odbc_brace(&driver)),
        odbc_pair("SERVER", &server),
        odbc_pair("DATABASE", &database),
        "Encrypt=optional".to_string(),
        "TrustServerCertificate=yes".to_string(),
    ];
    if auth == "sql" {
        parts.push(odbc_pair("UID", &user));
        parts.push(odbc_pair("PWD", password));
    } else {
        parts.push("Trusted_Connection=yes".to_string());
    }
    Ok(format!("{};", parts.join(";")))
}

fn odbc_brace(value: &str) -> String {
    format!("{{{}}}", value.replace('}', "}}"))
}

fn odbc_pair(key: &str, value: &str) -> String {
    if value
        .chars()
        .any(|character| matches!(character, ';' | '{' | '}' | '='))
    {
        format!("{key}={}", odbc_brace(value))
    } else {
        format!("{key}={value}")
    }
}

fn scrub(text: &str, secret: &str) -> String {
    let secret = secret.trim();
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "********")
    }
}

fn bind_kit_ic(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let bytes = sql.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let rest = &sql[index..];
        if rest.len() >= 6 && rest[..6].eq_ignore_ascii_case("@kitIC") {
            let after = index + 6;
            let boundary = after >= bytes.len()
                || !(bytes[after].is_ascii_alphanumeric() || bytes[after] == b'_');
            if boundary {
                out.push('?');
                index = after;
                continue;
            }
        }
        let ch = rest.chars().next().expect("index is on a char boundary");
        out.push(ch);
        index += ch.len_utf8();
    }
    out
}

fn load_queries() -> Result<(String, String), String> {
    let root = local_config::project_root()?;
    let dir = root.join("vision");
    let primary = dir.join(QUERY_FILE);
    let example = dir.join(QUERY_EXAMPLE);
    let path = if primary.is_file() {
        primary
    } else if example.is_file() {
        example
    } else {
        return Err(
            "BoM queries were not found. Add vision/bom_queries.sql or vision/bom_queries.example.sql."
                .to_string(),
        );
    };
    let text = std::fs::read_to_string(&path).map_err(|_| {
        format!(
            "Could not read {}.",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(QUERY_FILE)
        )
    })?;
    split_queries(&text).ok_or_else(|| {
        "BoM query file must contain a kit header query and a parts query, separated by a blank line."
            .to_string()
    })
}

fn split_queries(text: &str) -> Option<(String, String)> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let lines: Vec<&str> = normalized.split('\n').collect();
    let split_at = find_split(&lines, true).or_else(|| find_split(&lines, false))?;
    let header = lines[..split_at].join("\n").trim().to_string();
    let parts = lines[split_at + 1..].join("\n").trim().to_string();
    Some((header, parts))
}

fn find_split(lines: &[&str], marker: bool) -> Option<usize> {
    for (index, line) in lines.iter().enumerate() {
        let stripped = line.trim();
        let is_split = if marker {
            is_split_marker(stripped)
        } else {
            stripped.is_empty()
        };
        if !is_split {
            continue;
        }
        let header = lines[..index].join("\n");
        let parts = lines[index + 1..].join("\n");
        if has_select(&header) && starts_with_select(&parts) {
            return Some(index);
        }
    }
    None
}

fn is_split_marker(line: &str) -> bool {
    let line = line.trim();
    if line.eq_ignore_ascii_case("go") {
        return true;
    }
    let Some(rest) = line.strip_prefix("--") else {
        return false;
    };
    let rest = rest.trim();
    if rest.eq_ignore_ascii_case("split") {
        return true;
    }
    (rest.len() >= 3 && rest.chars().all(|ch| ch == '-'))
        || (rest.len() >= 3 && rest.chars().all(|ch| ch == '='))
}

fn has_select(sql: &str) -> bool {
    sql.split_whitespace()
        .any(|word| word.trim_start_matches('(').eq_ignore_ascii_case("select"))
}

fn starts_with_select(sql: &str) -> bool {
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with("--") {
            continue;
        }
        return trimmed
            .trim_start_matches('(')
            .to_ascii_lowercase()
            .starts_with("select");
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn example_queries_split_on_the_blank_line() {
        let root = local_config::project_root().unwrap();
        let text = std::fs::read_to_string(root.join("vision").join(QUERY_EXAMPLE)).unwrap();
        let (header, parts) = split_queries(&text).unwrap();
        assert!(header.to_ascii_lowercase().contains("select"));
        assert!(parts.to_ascii_lowercase().starts_with("select") || parts.contains("SELECT"));
        assert!(header.contains("@kitIC"));
        assert!(bind_kit_ic(&header).contains('?'));
        assert!(!bind_kit_ic(&header).to_ascii_lowercase().contains("@kitic"));
    }

    #[test]
    fn connection_string_uses_optional_encrypt_and_windows_auth() {
        let mssql = json!({
            "driver": "ODBC Driver 18 for SQL Server",
            "server": "sql.example",
            "database": "erp",
            "auth": "windows",
            "user": "",
            "password": ""
        });
        let text = connection_string(&mssql).unwrap();
        assert!(text.contains("Encrypt=optional"));
        assert!(text.contains("TrustServerCertificate=yes"));
        assert!(text.contains("Trusted_Connection=yes"));
        assert!(!text.contains("PWD="));
    }

    #[test]
    fn sql_auth_rejects_a_missing_password_and_scrubs_a_stored_one() {
        let mssql = json!({
            "server": "sql.example",
            "database": "erp",
            "auth": "sql",
            "user": "reader",
            "password": ""
        });
        assert!(connection_string(&mssql).is_err());
        let message = scrub("login failed for secret-value", "secret-value");
        assert_eq!(message, "login failed for ********");
        assert!(!message.contains("secret-value"));
    }
}
