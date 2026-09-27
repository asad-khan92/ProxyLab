//! Splitting a pasted cURL command into its arguments.
//!
//! ```text
//! curl 'https://example.com/api' \     ──▶  ["curl",
//!   -H 'Accept: application/json' \          "https://example.com/api",
//!   -d '{"name":"ada"}'                      "-H", "Accept: application/json",
//!                                            "-d", "{\"name\":\"ada\"}"]
//! ```
//!
//! This is the quoting a shell does and **none of the expansion**. A command
//! substitution, a backtick, a variable, a pipe, a redirection — every one of
//! them is refused with an error that says so. Nothing here runs a process,
//! opens a file, or reads an environment variable, and there is no code path
//! from this module to anything that could: what it produces is a list of
//! byte strings.
//!
//! That is the whole security posture of cURL import. A pasted command is
//! somebody else's text, quite possibly from a bug report or a support
//! ticket, and the one thing that must never happen is for pasting it to do
//! what it would do in a terminal.
//!
//! ```text
//! curl 'https://example.com/$(touch /tmp/pwned)'
//!       └──────────────────────────────────────┘
//!       one argument, containing those characters, and nothing else happens
//! ```
//!
//! # Quoting that is understood
//!
//! | | |
//! | --- | --- |
//! | `'…'` | every byte literal, backslash included |
//! | `"…"` | `\"`, `\\`, `` \` ``, `\$` and a line continuation are escapes |
//! | `$'…'` | ANSI-C quoting: `\n`, `\t`, `\xNN`, `\NNN`, and the rest |
//! | `\x` | the character `x`, outside quotes |
//! | `\` + newline | a line continuation, which is how a cURL command is pasted |
//! | `#` | a comment, to the end of the line |
//!
//! `$'…'` is here because it is *quoting* and not substitution — it is how a
//! byte string that is not text is written down, and so it is how this crate
//! writes a request body that is not text. Reading it back is the same
//! decision: bytes in, bytes out.

use crate::error::CurlError;

/// Splits `command` into the arguments a shell would pass to cURL.
///
/// Every byte of every argument is data. See the module documentation for
/// what is refused rather than expanded.
pub fn split(command: &str) -> Result<Vec<Vec<u8>>, CurlError> {
    Lexer {
        input: command.as_bytes(),
        at: 0,
    }
    .run()
}

/// Where the split has got to.
struct Lexer<'a> {
    input: &'a [u8],
    at: usize,
}

impl Lexer<'_> {
    /// Splits the whole command.
    fn run(mut self) -> Result<Vec<Vec<u8>>, CurlError> {
        let mut words = Vec::new();

        loop {
            self.skip_separators();
            if self.at >= self.input.len() {
                return Ok(words);
            }

            // A comment runs to the end of the line, which is what makes a
            // command this crate wrote — one that may carry a note above it
            // — something it can also read back.
            if self.peek() == Some(b'#') {
                self.skip_line();
                continue;
            }

            words.push(self.word()?);
        }
    }

    /// Reads one argument.
    fn word(&mut self) -> Result<Vec<u8>, CurlError> {
        let mut word = Vec::new();

        while let Some(byte) = self.peek() {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' => break,

                b'\'' => {
                    self.at += 1;
                    self.single_quoted(&mut word)?;
                }

                b'"' => {
                    self.at += 1;
                    self.double_quoted(&mut word)?;
                }

                b'\\' => {
                    self.at += 1;
                    match self.peek() {
                        // A line continuation: the backslash and the newline
                        // both go, and the word carries on across the break.
                        Some(b'\n') => self.at += 1,
                        Some(b'\r') => {
                            self.at += 1;
                            if self.peek() == Some(b'\n') {
                                self.at += 1;
                            }
                        }
                        Some(escaped) => {
                            word.push(escaped);
                            self.at += 1;
                        }
                        // A trailing backslash is a command that was cut off.
                        None => return Err(CurlError::UnterminatedQuote),
                    }
                }

                b'$' => match self.input.get(self.at + 1) {
                    Some(b'\'') => {
                        self.at += 2;
                        self.ansi_c_quoted(&mut word)?;
                    }
                    // `$"…"` is a double-quoted string a shell would offer to
                    // translate, and is otherwise a double-quoted string.
                    Some(b'"') => {
                        self.at += 2;
                        self.double_quoted(&mut word)?;
                    }
                    Some(next) if starts_expansion(*next) => return Err(CurlError::ShellExpansion),
                    // A lone `$` is a dollar sign, in a shell and here.
                    _ => {
                        word.push(b'$');
                        self.at += 1;
                    }
                },

                b'`' => return Err(CurlError::ShellExpansion),

                b'|' | b';' | b'&' | b'<' | b'>' | b'(' | b')' => {
                    return Err(CurlError::ShellMetacharacter)
                }

                _ => {
                    word.push(byte);
                    self.at += 1;
                }
            }
        }

        Ok(word)
    }

    /// Reads the inside of `'…'`, where every byte is itself.
    fn single_quoted(&mut self, word: &mut Vec<u8>) -> Result<(), CurlError> {
        while let Some(byte) = self.peek() {
            self.at += 1;
            if byte == b'\'' {
                return Ok(());
            }

            word.push(byte);
        }

        Err(CurlError::UnterminatedQuote)
    }

    /// Reads the inside of `"…"`.
    ///
    /// Four escapes, as a shell has; everything else — including a backslash
    /// before anything else — is literal. What a shell would *expand* is
    /// refused rather than expanded or quietly kept.
    fn double_quoted(&mut self, word: &mut Vec<u8>) -> Result<(), CurlError> {
        while let Some(byte) = self.peek() {
            match byte {
                b'"' => {
                    self.at += 1;
                    return Ok(());
                }

                b'\\' => {
                    self.at += 1;
                    match self.peek() {
                        Some(escaped @ (b'"' | b'\\' | b'`' | b'$')) => {
                            word.push(escaped);
                            self.at += 1;
                        }
                        Some(b'\n') => self.at += 1,
                        Some(other) => {
                            word.push(b'\\');
                            word.push(other);
                            self.at += 1;
                        }
                        None => return Err(CurlError::UnterminatedQuote),
                    }
                }

                b'`' => return Err(CurlError::ShellExpansion),

                b'$' => match self.input.get(self.at + 1) {
                    Some(next) if starts_expansion(*next) => return Err(CurlError::ShellExpansion),
                    _ => {
                        word.push(b'$');
                        self.at += 1;
                    }
                },

                _ => {
                    word.push(byte);
                    self.at += 1;
                }
            }
        }

        Err(CurlError::UnterminatedQuote)
    }

    /// Reads the inside of `$'…'`, which is how a byte string is written.
    fn ansi_c_quoted(&mut self, word: &mut Vec<u8>) -> Result<(), CurlError> {
        while let Some(byte) = self.peek() {
            match byte {
                b'\'' => {
                    self.at += 1;
                    return Ok(());
                }

                b'\\' => {
                    self.at += 1;
                    self.ansi_c_escape(word)?;
                }

                _ => {
                    word.push(byte);
                    self.at += 1;
                }
            }
        }

        Err(CurlError::UnterminatedQuote)
    }

    /// Reads one escape inside `$'…'`.
    fn ansi_c_escape(&mut self, word: &mut Vec<u8>) -> Result<(), CurlError> {
        let Some(byte) = self.peek() else {
            return Err(CurlError::UnterminatedQuote);
        };
        self.at += 1;

        let literal = match byte {
            b'a' => 0x07,
            b'b' => 0x08,
            b'e' | b'E' => 0x1b,
            b'f' => 0x0c,
            b'n' => b'\n',
            b'r' => b'\r',
            b't' => b'\t',
            b'v' => 0x0b,
            b'\\' => b'\\',
            b'\'' => b'\'',
            b'"' => b'"',
            b'?' => b'?',

            // At most two hex digits, as a shell reads them, so a byte
            // written `\x41` followed by a literal `B` is not read as three
            // hex digits.
            b'x' => {
                let mut value: u32 = 0;
                let mut digits = 0;
                while digits < 2 {
                    match self.peek().and_then(hex_digit) {
                        Some(digit) => {
                            value = value * 16 + digit;
                            digits += 1;
                            self.at += 1;
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    // `\x` with no digits is the two characters it is.
                    word.push(b'\\');
                    word.push(b'x');
                    return Ok(());
                }

                value as u8
            }

            // At most three octal digits, the first of which has already
            // been read.
            b'0'..=b'7' => {
                let mut value: u32 = u32::from(byte - b'0');
                let mut digits = 1;
                while digits < 3 {
                    match self.peek() {
                        Some(next @ b'0'..=b'7') => {
                            value = value * 8 + u32::from(next - b'0');
                            digits += 1;
                            self.at += 1;
                        }
                        _ => break,
                    }
                }

                (value & 0xff) as u8
            }

            // A code point, written as UTF-8: this is a byte string, and the
            // bytes a shell would produce are the encoded ones.
            b'u' | b'U' => {
                let width = if byte == b'u' { 4 } else { 8 };
                let mut value: u32 = 0;
                let mut digits = 0;
                while digits < width {
                    match self.peek().and_then(hex_digit) {
                        Some(digit) => {
                            value = value * 16 + digit;
                            digits += 1;
                            self.at += 1;
                        }
                        None => break,
                    }
                }
                if digits == 0 {
                    word.push(b'\\');
                    word.push(byte);
                    return Ok(());
                }

                match char::from_u32(value) {
                    Some(character) => {
                        let mut buffer = [0u8; 4];
                        word.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                    }
                    // Not a code point. Refused rather than replaced, so a
                    // body is never quietly something else.
                    None => return Err(CurlError::InvalidEscape),
                }

                return Ok(());
            }

            // An escape a shell does not know is the backslash and the
            // character it was in front of.
            other => {
                word.push(b'\\');
                word.push(other);
                return Ok(());
            }
        };

        word.push(literal);

        Ok(())
    }

    /// The byte at the cursor.
    fn peek(&self) -> Option<u8> {
        self.input.get(self.at).copied()
    }

    /// Steps over whitespace and the line continuations between arguments.
    fn skip_separators(&mut self) {
        while let Some(byte) = self.peek() {
            match byte {
                b' ' | b'\t' | b'\n' | b'\r' => self.at += 1,
                b'\\' if matches!(self.input.get(self.at + 1), Some(b'\n' | b'\r')) => self.at += 1,
                _ => return,
            }
        }
    }

    /// Steps over the rest of a line, which is what a comment is.
    fn skip_line(&mut self) {
        while let Some(byte) = self.peek() {
            self.at += 1;
            if byte == b'\n' {
                return;
            }
        }
    }
}

/// Whether `$` followed by this byte is something a shell would expand.
///
/// `$(`, `${`, and `$name` all are. A `$` in front of anything else — a
/// digit-free punctuation mark, a space, the end of the string — is the
/// character itself, which is why a URL with a `$` in its query is not
/// refused.
fn starts_expansion(byte: u8) -> bool {
    byte == b'(' || byte == b'{' || byte == b'_' || byte.is_ascii_alphabetic()
}

/// The value of one hexadecimal digit.
fn hex_digit(byte: u8) -> Option<u32> {
    char::from(byte).to_digit(16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(command: &str) -> Vec<String> {
        split(command)
            .expect("splits")
            .into_iter()
            .map(|word| String::from_utf8(word).expect("text"))
            .collect()
    }

    #[test]
    fn a_plain_command_splits_on_whitespace() {
        assert_eq!(
            words("curl -X GET https://example.com"),
            ["curl", "-X", "GET", "https://example.com"]
        );
    }

    #[test]
    fn runs_of_whitespace_are_one_separator() {
        assert_eq!(
            words("curl   \t  https://example.com"),
            ["curl", "https://example.com"]
        );
    }

    #[test]
    fn single_quotes_hold_a_word_together() {
        assert_eq!(
            words("curl -H 'Accept: application/json'"),
            ["curl", "-H", "Accept: application/json"]
        );
    }

    #[test]
    fn a_backslash_inside_single_quotes_is_a_backslash() {
        assert_eq!(words(r"curl 'a\b'"), ["curl", r"a\b"]);
    }

    #[test]
    fn double_quotes_hold_a_word_together_and_have_escapes() {
        assert_eq!(
            words(r#"curl -d "{\"name\":\"ada\"}""#),
            ["curl", "-d", r#"{"name":"ada"}"#]
        );
    }

    #[test]
    fn a_backslash_before_anything_else_in_double_quotes_is_literal() {
        assert_eq!(words(r#"curl "a\db""#), ["curl", r"a\db"]);
    }

    #[test]
    fn quotes_join_onto_the_word_around_them() {
        assert_eq!(words("curl pre'fix'post"), ["curl", "prefixpost"]);
        assert_eq!(words(r#"curl a"b"c'd'e"#), ["curl", "abcde"]);
    }

    #[test]
    fn a_line_continuation_joins_the_lines() {
        let command = "curl 'https://example.com' \\\n  -H 'Accept: */*' \\\n  -X POST";

        assert_eq!(
            words(command),
            [
                "curl",
                "https://example.com",
                "-H",
                "Accept: */*",
                "-X",
                "POST"
            ]
        );
    }

    #[test]
    fn a_windows_line_continuation_joins_the_lines_too() {
        assert_eq!(
            words("curl 'https://example.com' \\\r\n  -X POST"),
            ["curl", "https://example.com", "-X", "POST"]
        );
    }

    #[test]
    fn a_command_split_over_lines_without_backslashes_still_splits() {
        assert_eq!(
            words("curl\n  -X POST\n  https://example.com"),
            ["curl", "-X", "POST", "https://example.com"]
        );
    }

    #[test]
    fn a_comment_line_is_skipped() {
        assert_eq!(
            words("# ProxyLab captured only part of this body\ncurl 'https://example.com'"),
            ["curl", "https://example.com"]
        );
    }

    #[test]
    fn an_escaped_space_stays_in_the_word() {
        assert_eq!(words(r"curl a\ b"), ["curl", "a b"]);
    }

    #[test]
    fn an_escaped_quote_outside_quotes_is_a_quote() {
        assert_eq!(words(r"curl \'"), ["curl", "'"]);
    }

    // MARK: - What is never expanded

    #[test]
    fn command_substitution_in_single_quotes_is_text() {
        assert_eq!(
            words("curl 'https://example.com/$(touch /tmp/pwned)'"),
            ["curl", "https://example.com/$(touch /tmp/pwned)"]
        );
    }

    #[test]
    fn command_substitution_outside_quotes_is_refused() {
        assert_eq!(
            split("curl https://example.com/$(whoami)"),
            Err(CurlError::ShellExpansion)
        );
    }

    #[test]
    fn command_substitution_in_double_quotes_is_refused() {
        assert_eq!(
            split(r#"curl "https://example.com/$(whoami)""#),
            Err(CurlError::ShellExpansion)
        );
    }

    #[test]
    fn a_backtick_is_refused_wherever_it_could_expand() {
        assert_eq!(split("curl `whoami`"), Err(CurlError::ShellExpansion));
        assert_eq!(split(r#"curl "`whoami`""#), Err(CurlError::ShellExpansion));
    }

    #[test]
    fn a_backtick_in_single_quotes_is_text() {
        assert_eq!(words("curl 'a`b`c'"), ["curl", "a`b`c"]);
    }

    #[test]
    fn a_variable_is_refused_rather_than_read() {
        assert_eq!(
            split("curl -H \"Authorization: Bearer $TOKEN\""),
            Err(CurlError::ShellExpansion)
        );
        assert_eq!(split("curl ${URL}"), Err(CurlError::ShellExpansion));
    }

    #[test]
    fn a_variable_in_single_quotes_is_text() {
        assert_eq!(
            words("curl -H 'Authorization: Bearer $TOKEN'"),
            ["curl", "-H", "Authorization: Bearer $TOKEN"]
        );
    }

    #[test]
    fn a_lone_dollar_sign_is_a_dollar_sign() {
        assert_eq!(
            words("curl https://example.com/?cost=$"),
            ["curl", "https://example.com/?cost=$"]
        );
        assert_eq!(words(r#"curl "cost: $""#), ["curl", "cost: $"]);
    }

    #[test]
    fn a_pipeline_is_refused() {
        assert_eq!(
            split("curl https://example.com | sh"),
            Err(CurlError::ShellMetacharacter)
        );
    }

    #[test]
    fn a_redirection_is_refused() {
        assert_eq!(
            split("curl https://example.com > /tmp/out"),
            Err(CurlError::ShellMetacharacter)
        );
        assert_eq!(
            split("curl https://example.com < /etc/passwd"),
            Err(CurlError::ShellMetacharacter)
        );
    }

    #[test]
    fn a_second_command_is_refused() {
        assert_eq!(
            split("curl https://example.com; rm -rf /"),
            Err(CurlError::ShellMetacharacter)
        );
        assert_eq!(
            split("curl https://example.com && rm -rf /"),
            Err(CurlError::ShellMetacharacter)
        );
    }

    #[test]
    fn a_subshell_is_refused() {
        assert_eq!(split("curl (whoami)"), Err(CurlError::ShellMetacharacter));
    }

    #[test]
    fn metacharacters_inside_quotes_are_text() {
        assert_eq!(
            words("curl 'https://example.com/?a=1&b=2;c=3|d'"),
            ["curl", "https://example.com/?a=1&b=2;c=3|d"]
        );
    }

    // MARK: - Unterminated

    #[test]
    fn an_unterminated_single_quote_is_refused() {
        assert_eq!(
            split("curl 'https://example.com"),
            Err(CurlError::UnterminatedQuote)
        );
    }

    #[test]
    fn an_unterminated_double_quote_is_refused() {
        assert_eq!(
            split(r#"curl "https://example.com"#),
            Err(CurlError::UnterminatedQuote)
        );
    }

    #[test]
    fn a_trailing_backslash_is_refused() {
        assert_eq!(
            split(r"curl https://example.com \"),
            Err(CurlError::UnterminatedQuote)
        );
    }

    // MARK: - ANSI-C quoting

    #[test]
    fn ansi_c_quoting_reads_the_usual_escapes() {
        let word = &split(r"curl $'a\nb\tc\\d\'e'").expect("splits")[1];

        assert_eq!(word, b"a\nb\tc\\d'e");
    }

    #[test]
    fn ansi_c_quoting_reads_hex_bytes() {
        let word = &split(r"curl $'\x00\xff\x41'").expect("splits")[1];

        assert_eq!(word, &[0x00, 0xff, 0x41]);
    }

    #[test]
    fn a_hex_escape_reads_at_most_two_digits() {
        let word = &split(r"curl $'\x41BC'").expect("splits")[1];

        assert_eq!(word, b"ABC");
    }

    #[test]
    fn ansi_c_quoting_reads_octal_bytes() {
        let word = &split(r"curl $'\101\0\377'").expect("splits")[1];

        assert_eq!(word, &[b'A', 0x00, 0xff]);
    }

    #[test]
    fn ansi_c_quoting_reads_a_code_point_as_its_utf8_bytes() {
        let word = &split(r"curl $'é'").expect("splits")[1];

        assert_eq!(word, "é".as_bytes());
    }

    #[test]
    fn an_unknown_escape_keeps_its_backslash() {
        let word = &split(r"curl $'a\qb'").expect("splits")[1];

        assert_eq!(word, br"a\qb");
    }

    #[test]
    fn an_unterminated_ansi_c_quote_is_refused() {
        assert_eq!(split(r"curl $'abc"), Err(CurlError::UnterminatedQuote));
    }

    #[test]
    fn every_byte_value_survives_ansi_c_quoting() {
        let escaped: String = (0u16..=255)
            .map(|byte| format!(r"\x{byte:02x}"))
            .collect::<Vec<_>>()
            .concat();
        let word = &split(&format!("curl $'{escaped}'")).expect("splits")[1];

        assert_eq!(*word, (0..=255u8).collect::<Vec<u8>>());
    }

    // MARK: - Nothing at all

    #[test]
    fn an_empty_command_splits_into_nothing() {
        assert_eq!(split("").expect("splits"), Vec::<Vec<u8>>::new());
        assert_eq!(split("   \n  ").expect("splits"), Vec::<Vec<u8>>::new());
    }

    #[test]
    fn an_empty_quoted_argument_is_an_argument() {
        assert_eq!(words("curl -d ''"), ["curl", "-d", ""]);
    }
}
