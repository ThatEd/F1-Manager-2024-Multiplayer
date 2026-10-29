//! Helpers shared by the snapshot tools: parsing the snapshot JSON and quoting values for SQL.

use std::collections::BTreeMap;

pub type Table = BTreeMap<String, Vec<Vec<String>>>;

/// Minimal JSON reader for the snapshot shape: `{ "table": [ ["v","v"], ... ], ... }`.
///
/// Hand-rolled rather than pulling in a dependency: the format is ours, written by `snapshot`, and it
/// is small enough that a parser is less risk than a new crate.
pub fn parse_snapshot(text: &str) -> Result<Table, String> {
    let b: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    skip_ws(&b, &mut i);
    expect(&b, &mut i, '{')?;
    let mut out: Table = BTreeMap::new();
    loop {
        skip_ws(&b, &mut i);
        if peek(&b, i) == Some('}') {
            i += 1;
            break;
        }
        let key = parse_string(&b, &mut i)?;
        skip_ws(&b, &mut i);
        expect(&b, &mut i, ':')?;
        let rows = parse_rows(&b, &mut i)?;
        out.insert(key, rows);
        skip_ws(&b, &mut i);
        match peek(&b, i) {
            Some(',') => i += 1,
            Some('}') => {
                i += 1;
                break;
            }
            other => return Err(format!("expected , or }} but found {other:?}")),
        }
    }
    Ok(out)
}

fn parse_rows(b: &[char], i: &mut usize) -> Result<Vec<Vec<String>>, String> {
    let mut rows = Vec::new();
    skip_ws(b, i);
    expect(b, i, '[')?;
    loop {
        skip_ws(b, i);
        if peek(b, *i) == Some(']') {
            *i += 1;
            break;
        }
        let mut row = Vec::new();
        expect(b, i, '[')?;
        loop {
            skip_ws(b, i);
            if peek(b, *i) == Some(']') {
                *i += 1;
                break;
            }
            row.push(parse_string(b, i)?);
            skip_ws(b, i);
            if peek(b, *i) == Some(',') {
                *i += 1;
            }
        }
        rows.push(row);
        skip_ws(b, i);
        if peek(b, *i) == Some(',') {
            *i += 1;
        }
    }
    Ok(rows)
}

fn parse_string(b: &[char], i: &mut usize) -> Result<String, String> {
    skip_ws(b, i);
    expect(b, i, '"')?;
    let mut s = String::new();
    while let Some(c) = peek(b, *i) {
        *i += 1;
        match c {
            '"' => return Ok(s),
            '\\' => {
                let e = peek(b, *i).ok_or("truncated escape")?;
                *i += 1;
                match e {
                    '"' => s.push('"'),
                    '\\' => s.push('\\'),
                    '/' => s.push('/'),
                    'n' => s.push('\n'),
                    'r' => s.push('\r'),
                    't' => s.push('\t'),
                    'b' => s.push('\u{8}'),
                    'f' => s.push('\u{c}'),
                    'u' => {
                        let mut code = 0u32;
                        for _ in 0..4 {
                            let h = peek(b, *i).ok_or("truncated \\u escape")?;
                            *i += 1;
                            code = code * 16
                                + h.to_digit(16).ok_or_else(|| format!("bad hex {h:?}"))?;
                        }
                        if let Some(ch) = char::from_u32(code) {
                            s.push(ch);
                        }
                    }
                    other => return Err(format!("unknown escape \\{other}")),
                }
            }
            other => s.push(other),
        }
    }
    Err("unterminated string".into())
}

fn skip_ws(b: &[char], i: &mut usize) {
    while let Some(c) = peek(b, *i) {
        if c.is_whitespace() {
            *i += 1;
        } else {
            break;
        }
    }
}

fn peek(b: &[char], i: usize) -> Option<char> {
    b.get(i).copied()
}

fn expect(b: &[char], i: &mut usize, want: char) -> Result<(), String> {
    match peek(b, *i) {
        Some(c) if c == want => {
            *i += 1;
            Ok(())
        }
        other => Err(format!("expected {want:?} but found {other:?}")),
    }
}


/// Quotes a value for SQL. Every value is text because that is all the row callback produces, so an
/// empty string stays an empty string and is NOT mapped to NULL.
pub fn quote(v: &str) -> String {
    format!("'{}'", v.replace('\'', "''"))
}
