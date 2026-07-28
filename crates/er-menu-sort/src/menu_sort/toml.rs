pub(super) struct TomlString;

impl TomlString {
    pub(super) fn parse(value: &str) -> Result<String, &'static str> {
        let value = value.trim();
        if value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'') {
            return Ok(value[1..value.len() - 1].to_owned());
        }
        if value.len() < 2 || !value.starts_with('"') || !value.ends_with('"') {
            return Err("expected a quoted TOML string");
        }

        let mut parsed = String::with_capacity(value.len());
        let mut chars = value[1..value.len() - 1].chars();
        while let Some(ch) = chars.next() {
            if ch == '\\' {
                match chars.next() {
                    Some('\\') => parsed.push('\\'),
                    Some('"') => parsed.push('"'),
                    Some('n') => parsed.push('\n'),
                    Some('r') => parsed.push('\r'),
                    Some('t') => parsed.push('\t'),
                    _ => return Err("unsupported escape in string"),
                }
            } else {
                parsed.push(ch);
            }
        }
        Ok(parsed)
    }
}
