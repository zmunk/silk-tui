//! Shell exit/output contract. Parses --query, prints final command to stdout,
//! exits with the correct code (0 / 10 / 130).

pub fn parse_args() -> anyhow::Result<String> {
    // TODO: parse --query "$BUFFER"
    Ok(String::new())
}
