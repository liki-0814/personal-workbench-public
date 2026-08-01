const PREFIX: &str = "\u{001e}pwcli-added-tools:";
const SUFFIX: char = '\u{001e}';

pub fn attach(content: String, names: &[String]) -> String {
    if names.is_empty() {
        return content;
    }
    let encoded = serde_json::to_string(names).unwrap_or_else(|_| "[]".to_string());
    format!("{PREFIX}{encoded}{SUFFIX}{content}")
}

pub fn split(content: &str) -> (Vec<String>, &str) {
    let Some(rest) = content.strip_prefix(PREFIX) else {
        return (Vec::new(), content);
    };
    let Some((encoded, body)) = rest.split_once(SUFFIX) else {
        return (Vec::new(), content);
    };
    match serde_json::from_str(encoded) {
        Ok(names) => (names, body),
        Err(_) => (Vec::new(), content),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn metadata_round_trips_without_changing_plain_content() {
        let names = vec!["late_tool".to_string()];
        let encoded = attach("result".to_string(), &names);
        assert_eq!(split(&encoded), (names, "result"));
        assert_eq!(split("plain"), (Vec::new(), "plain"));
    }
}
