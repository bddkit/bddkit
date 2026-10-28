//! "Did you mean": the templates `steps list` prints, ranked by word-level
//! edit distance from an unknown step.

#[derive(PartialEq)]
enum Token<'a> {
    Word(&'a str),
    /// A quoted string on the step side, `"<name>"` on the template side.
    Quoted,
    /// A bare `<name>` in a template: matches any word of the step.
    Any,
}

fn step_tokens(text: &str) -> Vec<Token<'_>> {
    let mut tokens = Vec::new();
    let (mut start, mut quoted) = (None, false);
    for (i, c) in text.char_indices().chain([(text.len(), ' ')]) {
        if c == '"' {
            quoted = !quoted;
        }
        match (start, (c.is_whitespace() && !quoted) || i == text.len()) {
            (None, false) => start = Some(i),
            (Some(from), true) => {
                let word = &text[from..i];
                let is_quoted = word.len() > 1 && word.starts_with('"') && word.ends_with('"');
                tokens.push(if is_quoted {
                    Token::Quoted
                } else {
                    Token::Word(word)
                });
                start = None;
            }
            _ => {}
        }
    }
    tokens
}

fn template_tokens(template: &str) -> Vec<Token<'_>> {
    template
        .split_whitespace()
        .map(|word| {
            if word.starts_with('"') && word.contains('<') {
                Token::Quoted
            } else if word.starts_with('<') && word.ends_with('>') {
                Token::Any
            } else {
                Token::Word(word)
            }
        })
        .collect()
}

fn distance(step: &[Token], template: &[Token]) -> usize {
    let mut row: Vec<usize> = (0..=template.len()).collect();
    for (i, s) in step.iter().enumerate() {
        let mut diagonal = std::mem::replace(&mut row[0], i + 1);
        for (j, t) in template.iter().enumerate() {
            let cost = usize::from(!(matches!(t, Token::Any) || s == t));
            let next = (diagonal + cost).min(row[j] + 1).min(row[j + 1] + 1);
            diagonal = std::mem::replace(&mut row[j + 1], next);
        }
    }
    row[template.len()]
}

/// The nearest templates, at most three and only the tied best, or none when
/// even the best differs in more than about a third of the longer phrase.
pub fn closest<'a>(text: &str, templates: impl Iterator<Item = &'a str>) -> Vec<&'a str> {
    let step = step_tokens(text);
    let mut best = usize::MAX;
    let mut found: Vec<&str> = Vec::new();
    for template in templates {
        let tokens = template_tokens(template);
        let d = distance(&step, &tokens);
        if d * 3 > step.len().max(tokens.len()) || d > best || found.contains(&template) {
            continue;
        }
        if d < best {
            best = d;
            found.clear();
        }
        found.push(template);
    }
    found.truncate(3);
    found
}
