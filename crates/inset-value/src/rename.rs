//! Serde's `rename_all` rules, which turn a field's or a variant's Rust
//! name into its name in JSON.
//!
//! These copy serde's own rules (`serde_derive`'s `RenameRule`) exactly,
//! because the compiler states a JSON body's names in the API contract and
//! in requirement tests, while the program's own serde derive writes them.
//! The two must agree, and the requirement tests, which compare what the
//! built program replies with what these rules say, would show if they did not.

/// The rules `#[serde(rename_all = "...")]` accepts, by name.
pub const RULES: [&str; 8] = [
    "lowercase",
    "UPPERCASE",
    "PascalCase",
    "camelCase",
    "snake_case",
    "SCREAMING_SNAKE_CASE",
    "kebab-case",
    "SCREAMING-KEBAB-CASE",
];

/// Whether `rule` is one of [`RULES`].
pub fn is_rule(rule: &str) -> bool {
    RULES.contains(&rule)
}

/// A field's name under `rule`. Fields are written in `snake_case`.
pub fn field(rule: &str, name: &str) -> String {
    match rule {
        "UPPERCASE" | "SCREAMING_SNAKE_CASE" => name.to_ascii_uppercase(),
        "PascalCase" => pascal(name),
        "camelCase" => lower_first(&pascal(name)),
        "kebab-case" => name.replace('_', "-"),
        "SCREAMING-KEBAB-CASE" => name.to_ascii_uppercase().replace('_', "-"),
        _ => name.to_string(),
    }
}

/// A variant's name under `rule`. Variants are written in `PascalCase`.
pub fn variant(rule: &str, name: &str) -> String {
    match rule {
        "lowercase" => name.to_ascii_lowercase(),
        "UPPERCASE" => name.to_ascii_uppercase(),
        "camelCase" => lower_first(name),
        "snake_case" => snake(name),
        "SCREAMING_SNAKE_CASE" => snake(name).to_ascii_uppercase(),
        "kebab-case" => snake(name).replace('_', "-"),
        "SCREAMING-KEBAB-CASE" => snake(name).to_ascii_uppercase().replace('_', "-"),
        _ => name.to_string(),
    }
}

fn pascal(name: &str) -> String {
    let mut out = String::new();
    let mut capitalize = true;
    for c in name.chars() {
        if c == '_' {
            capitalize = true;
        } else if capitalize {
            out.push(c.to_ascii_uppercase());
            capitalize = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn lower_first(name: &str) -> String {
    let mut chars = name.chars();
    match chars.next() {
        Some(first) => first.to_ascii_lowercase().to_string() + chars.as_str(),
        None => String::new(),
    }
}

fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.char_indices() {
        if i > 0 && c.is_uppercase() {
            out.push('_');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fields_and_variants_follow_serdes_rules() {
        let fields: Vec<String> = RULES.iter().map(|r| field(r, "on_hand")).collect();
        assert_eq!(
            fields,
            [
                "on_hand", "ON_HAND", "OnHand", "onHand", "on_hand", "ON_HAND", "on-hand",
                "ON-HAND"
            ]
        );
        let variants: Vec<String> = RULES.iter().map(|r| variant(r, "OnHold")).collect();
        assert_eq!(
            variants,
            [
                "onhold", "ONHOLD", "OnHold", "onHold", "on_hold", "ON_HOLD", "on-hold", "ON-HOLD"
            ]
        );
    }
}
