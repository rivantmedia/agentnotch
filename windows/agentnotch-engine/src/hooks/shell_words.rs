//! Just enough word splitting to read back the hook and status line commands
//! in a settings.json on Windows (ShellWords.swift, HS§3.3).
//!
//! Claude Code runs a string command through Git Bash or PowerShell, so a
//! command found in the file may be written for either. This splitter reads
//! both well enough to answer one question, "which program does the last
//! command run, and with which arguments":
//!
//! - single and double quotes group a word;
//! - a backslash is an ordinary character (`C:\Users\me` is a path here, as
//!   PowerShell reads it; bash would eat the backslashes), except `\"`
//!   inside double quotes, which is a quote (no program path ends in a
//!   backslash, so nothing is lost);
//! - `;`, `&`, `&&`, `|`, `||` and a newline end a simple command
//!   (PowerShell's call operator `&` is one of them, which drops it).
//!
//! Nothing is ever expanded: `$HOME` stays `$HOME`.

/// One token of a command line: a word, or a control operator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Word(String),
    Operator(String),
}

/// Splits `command` into words and operators. An unterminated quote ends the
/// command (what is left is kept as the last word), so this never fails.
pub fn tokens(command: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = command.chars().peekable();

    fn end_word(tokens: &mut Vec<Token>, word: &mut String, in_word: &mut bool) {
        if *in_word {
            tokens.push(Token::Word(std::mem::take(word)));
        }
        word.clear();
        *in_word = false;
    }

    while let Some(ch) = chars.next() {
        match ch {
            '\'' => {
                in_word = true;
                for quoted in chars.by_ref() {
                    if quoted == '\'' {
                        break;
                    }
                    word.push(quoted);
                }
            }
            '"' => {
                in_word = true;
                while let Some(quoted) = chars.next() {
                    match quoted {
                        '"' => break,
                        '\\' if chars.peek() == Some(&'"') => {
                            chars.next();
                            word.push('"');
                        }
                        other => word.push(other),
                    }
                }
            }
            ';' | '&' | '|' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                let mut operator = String::from(ch);
                if ch != ';' && chars.peek() == Some(&ch) {
                    chars.next();
                    operator.push(ch);
                }
                tokens.push(Token::Operator(operator));
            }
            '\n' => {
                end_word(&mut tokens, &mut word, &mut in_word);
                tokens.push(Token::Operator(";".to_owned()));
            }
            other if other.is_whitespace() => end_word(&mut tokens, &mut word, &mut in_word),
            other => {
                in_word = true;
                word.push(other);
            }
        }
    }
    end_word(&mut tokens, &mut word, &mut in_word);
    tokens
}

/// The words of `command`, operators dropped.
pub fn words(command: &str) -> Vec<String> {
    tokens(command)
        .into_iter()
        .filter_map(|token| match token {
            Token::Word(word) => Some(word),
            Token::Operator(_) => None,
        })
        .collect()
}

/// The words of the last simple command (after the last `;`, `&&`, `||`,
/// `|`, `&` or newline), or `None` when there are none.
pub fn last_simple_command(command: &str) -> Option<Vec<String>> {
    let mut current = Vec::new();
    let mut last = None;
    for token in tokens(command) {
        match token {
            Token::Word(word) => current.push(word),
            Token::Operator(_) => {
                if !current.is_empty() {
                    last = Some(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.is_empty() {
        last = Some(current);
    }
    last
}

/// Where the program of a simple command starts: past `exec`, `env` (with
/// its options and `NAME=value` pairs) and leading assignments. `None` when
/// nothing is left to run.
pub fn program_index(words: &[String]) -> Option<usize> {
    fn is_assignment(word: &str) -> bool {
        match word.find('=') {
            Some(0) | None => false,
            Some(equals) => word[..equals]
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_'),
        }
    }
    let mut index = 0;
    while index < words.len() {
        let word = words[index].as_str();
        if word == "exec" || is_assignment(word) {
            index += 1;
        } else if matches!(file_name(word), "env" | "env.exe") {
            index += 1;
            while index < words.len()
                && (words[index].starts_with('-') || is_assignment(&words[index]))
            {
                index += 1;
            }
        } else {
            return Some(index);
        }
    }
    None
}

/// The last component of a path written with either separator.
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(text: &str) -> Token {
        Token::Word(text.to_owned())
    }

    fn operator(text: &str) -> Token {
        Token::Operator(text.to_owned())
    }

    /// `ShellWordsTests.splitsLikeTheShell`, with Windows' backslashes.
    #[test]
    fn splits_words_quotes_and_operators() {
        assert_eq!(
            tokens(r#"a 'b c' "d \"e\" $f" g\h; i && j || k | l &"#),
            [
                word("a"),
                word("b c"),
                word(r#"d "e" $f"#),
                word(r"g\h"),
                operator(";"),
                word("i"),
                operator("&&"),
                word("j"),
                operator("||"),
                word("k"),
                operator("|"),
                word("l"),
                operator("&"),
            ]
        );
        assert_eq!(
            words(r#""C:\Program Files\Tool\tool.exe" run"#),
            [r"C:\Program Files\Tool\tool.exe", "run"]
        );
        assert_eq!(words("unterminated 'quote"), ["unterminated", "quote"]);
        assert_eq!(tokens("a\nb"), [word("a"), operator(";"), word("b")]);
        assert_eq!(tokens(""), []);
        // Quotes glued to a word are one word.
        assert_eq!(words(r#"a'b c'"d""#), ["ab cd"]);
    }

    /// `HookCommandsTests.lastSimpleCommandAndInterpreter`
    #[test]
    fn last_simple_command_and_program() {
        let last = |command: &str| last_simple_command(command);
        assert_eq!(last("a; b c && d e"), Some(vec!["d".into(), "e".into()]));
        assert_eq!(last("a;"), Some(vec!["a".into()]));
        assert_eq!(last(" ; "), None);
        // PowerShell's call operator is an operator: what follows is the command.
        assert_eq!(
            last(r#"& "C:\x\tool.exe" run"#),
            Some(vec![r"C:\x\tool.exe".into(), "run".into()])
        );

        let strings = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let words = strings(&["exec", "X=1", "/usr/bin/env", "-i", "Y=2", "tool", "-S"]);
        assert_eq!(program_index(&words), Some(5));
        assert_eq!(program_index(&strings(&["exec"])), None);
        assert_eq!(program_index(&strings(&["=x", "y"])), Some(0));
    }

    #[test]
    fn file_names_with_either_separator() {
        assert_eq!(file_name(r"C:\a\b.exe"), "b.exe");
        assert_eq!(file_name("C:/a/b.exe"), "b.exe");
        assert_eq!(file_name(r"C:\a/b\\c.exe"), "c.exe");
        assert_eq!(file_name("b.exe"), "b.exe");
        assert_eq!(file_name(r"C:\a\"), "a");
        assert_eq!(file_name(""), "");
    }
}
