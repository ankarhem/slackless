//! Lightweight, zero-dependency, theme-aware syntax highlighter for fenced code blocks and
//! snippet cards. Emits exact byte-length [`TokenKind`] spans summing to the input's byte length
//! so GPUI [`gpui_kit::gpui::TextRun`]s preserve full text selection and copy fidelity.

use gpui_kit::gpui::Hsla;

/// Supported syntax-highlighting languages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lang {
    Xml,
    Json,
    Rust,
    Js,
    Python,
    Shell,
    Sql,
    Diff,
    Plain,
}

/// Semantic token category mapped to a dark- or light-mode color.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenKind {
    Plain,
    Keyword,
    String,
    Number,
    Comment,
    Tag,
    Attr,
    Type,
    DiffAdd,
    DiffDel,
    DiffHunk,
}

impl Lang {
    /// Detect the language from an optional fence/filetype hint, filename, and content sample.
    /// Generic hints like `"text"` or `"Plain Text"` fall through to filename and content
    /// auto-detection so XML plists like `Chalk_Custom.terminal` still highlight cleanly.
    pub fn detect(hint: Option<&str>, filename: Option<&str>, sample: &str) -> Self {
        if let Some(lang) = hint.and_then(Self::from_hint) {
            return lang;
        }
        if let Some(ext) = filename.and_then(|name| name.rsplit_once('.').map(|(_, e)| e))
            && let Some(lang) = Self::from_hint(ext)
        {
            return lang;
        }
        Self::from_content(sample)
    }

    fn from_hint(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "xml" | "html" | "htm" | "svg" | "plist" | "xhtml" | "terminal" => Some(Self::Xml),
            "json" | "jsonc" | "jsonl" => Some(Self::Json),
            "rust" | "rs" => Some(Self::Rust),
            "js" | "javascript" | "jsx" | "ts" | "typescript" | "tsx" | "mjs" | "cjs" => {
                Some(Self::Js)
            }
            "py" | "python" | "py3" => Some(Self::Python),
            "sh" | "bash" | "zsh" | "shell" | "shellscript" | "console" => Some(Self::Shell),
            "sql" | "postgres" | "postgresql" | "mysql" | "sqlite" => Some(Self::Sql),
            "diff" | "patch" => Some(Self::Diff),
            _ => None,
        }
    }

    fn from_content(sample: &str) -> Self {
        let trimmed = sample.trim_start();
        if trimmed.is_empty() {
            return Self::Plain;
        }
        if trimmed.starts_with("<?xml")
            || trimmed.starts_with("<!DOCTYPE")
            || trimmed.starts_with("<!doctype")
            || trimmed.starts_with("<plist")
            || trimmed.starts_with("<html")
            || trimmed.starts_with("<svg")
        {
            return Self::Xml;
        }
        if trimmed.starts_with("diff --git ")
            || (trimmed.starts_with("--- ") && trimmed.contains("\n+++ "))
            || trimmed.starts_with("@@ ")
        {
            return Self::Diff;
        }
        if let Some(first_line) = trimmed.lines().next()
            && let Some(rest) = first_line.strip_prefix("#!")
        {
            if rest.contains("python") {
                return Self::Python;
            }
            if rest.contains("sh") || rest.contains("bash") || rest.contains("zsh") {
                return Self::Shell;
            }
        }
        if (trimmed.starts_with('{') || trimmed.starts_with('[')) && trimmed.contains("\":") {
            return Self::Json;
        }
        let first_lines: String = trimmed.lines().take(6).collect::<Vec<_>>().join("\n");
        if first_lines.contains("fn ")
            && (first_lines.contains("->")
                || first_lines.contains("let ")
                || first_lines.contains("pub "))
        {
            return Self::Rust;
        }
        let upper = first_lines.to_ascii_uppercase();
        if (upper.starts_with("SELECT ") && upper.contains(" FROM "))
            || upper.starts_with("CREATE TABLE ")
            || upper.starts_with("INSERT INTO ")
            || upper.starts_with("ALTER TABLE ")
        {
            return Self::Sql;
        }
        if trimmed.lines().any(|l| {
            let l = l.trim_start();
            l.starts_with("$ ")
                || l.starts_with("cargo ")
                || l.starts_with("git ")
                || l.starts_with("curl ")
                || l.starts_with("export ")
                || l.starts_with("docker ")
                || l.starts_with("npm ")
                || l.starts_with("kubectl ")
        }) {
            return Self::Shell;
        }
        if first_lines.contains("def ") && first_lines.contains(':') {
            return Self::Python;
        }
        if (first_lines.contains("const ")
            || first_lines.contains("function ")
            || first_lines.contains("interface "))
            && (first_lines.contains("=>") || first_lines.contains('{'))
        {
            return Self::Js;
        }
        Self::Plain
    }
}

/// Map `kind` to a legible foreground color in dark or light mode.
pub fn token_color(kind: TokenKind, dark: bool, fg: Hsla) -> Hsla {
    crate::theme::syntax_token_color(kind, dark, fg)
}

/// Tokenize `code` for `lang`, returning merged `(byte_len, TokenKind)` spans whose lengths sum
/// exactly to `code.len()`.
pub fn highlight(lang: Lang, code: &str) -> Vec<(usize, TokenKind)> {
    if code.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    match lang {
        Lang::Plain => push(&mut out, code.len(), TokenKind::Plain),
        Lang::Diff => highlight_diff(code, &mut out),
        Lang::Xml => highlight_xml(code, &mut out),
        Lang::Json => highlight_json(code, &mut out),
        Lang::Rust | Lang::Js | Lang::Python | Lang::Shell | Lang::Sql => {
            highlight_code(lang, code, &mut out);
        }
    }
    out
}

fn push(out: &mut Vec<(usize, TokenKind)>, len: usize, kind: TokenKind) {
    if len == 0 {
        return;
    }
    if let Some((last_len, last_kind)) = out.last_mut()
        && *last_kind == kind
    {
        *last_len += len;
        return;
    }
    out.push((len, kind));
}

fn highlight_diff(code: &str, out: &mut Vec<(usize, TokenKind)>) {
    for line in code.split_inclusive('\n') {
        let kind =
            if line.starts_with("+++") || line.starts_with("---") || line.starts_with("diff ") {
                TokenKind::Keyword
            } else if line.starts_with("@@") {
                TokenKind::DiffHunk
            } else if line.starts_with('+') {
                TokenKind::DiffAdd
            } else if line.starts_with('-') {
                TokenKind::DiffDel
            } else {
                TokenKind::Plain
            };
        push(out, line.len(), kind);
    }
}

fn highlight_xml(code: &str, out: &mut Vec<(usize, TokenKind)>) {
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if code[i..].starts_with("<!--") {
            let end = code[i + 4..]
                .find("-->")
                .map(|off| i + 4 + off + 3)
                .unwrap_or(bytes.len());
            push(out, end - i, TokenKind::Comment);
            i = end;
            continue;
        }
        if bytes[i] == b'<' {
            let tag_start = i;
            let is_decl = code[i..].starts_with("<?") || code[i..].starts_with("<!");
            let is_close = code[i..].starts_with("</");
            let prefix_len = if is_decl || is_close { 2 } else { 1 };
            push(out, prefix_len, TokenKind::Plain);
            i += prefix_len;

            // Tag or declaration name.
            let name_start = i;
            while i < bytes.len()
                && !bytes[i].is_ascii_whitespace()
                && !matches!(bytes[i], b'>' | b'/' | b'?')
            {
                i += next_char_len(code, i);
            }
            if i > name_start {
                push(
                    out,
                    i - name_start,
                    if is_decl {
                        TokenKind::Keyword
                    } else {
                        TokenKind::Tag
                    },
                );
            }

            // Inside tag: attributes, quoted values, closing bracket.
            while i < bytes.len() {
                if bytes[i] == b'>' {
                    push(out, 1, TokenKind::Plain);
                    i += 1;
                    break;
                }
                if (bytes[i] == b'/' || bytes[i] == b'?')
                    && i + 1 < bytes.len()
                    && bytes[i + 1] == b'>'
                {
                    push(out, 2, TokenKind::Plain);
                    i += 2;
                    break;
                }
                if bytes[i] == b'"' || bytes[i] == b'\'' {
                    let quote = bytes[i];
                    let start = i;
                    i += 1;
                    while i < bytes.len() && bytes[i] != quote {
                        i += next_char_len(code, i);
                    }
                    if i < bytes.len() {
                        i += 1;
                    }
                    push(out, i - start, TokenKind::String);
                } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
                    let start = i;
                    while i < bytes.len()
                        && (bytes[i].is_ascii_alphanumeric()
                            || matches!(bytes[i], b'_' | b'-' | b':'))
                    {
                        i += 1;
                    }
                    let word = &code[start..i];
                    let kind = if is_decl && matches!(word, "PUBLIC" | "SYSTEM") {
                        TokenKind::Keyword
                    } else {
                        TokenKind::Attr
                    };
                    push(out, i - start, kind);
                } else {
                    let step = next_char_len(code, i);
                    push(out, step, TokenKind::Plain);
                    i += step;
                }
            }
            debug_assert!(i > tag_start);
        } else {
            let start = i;
            while i < bytes.len() && bytes[i] != b'<' {
                i += next_char_len(code, i);
            }
            push(out, i - start, TokenKind::Plain);
        }
    }
}

fn highlight_json(code: &str, out: &mut Vec<(usize, TokenKind)>) {
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'"' => {
                let start = i;
                i = scan_quoted(code, i, b'"');
                let mut look = i;
                while look < bytes.len() && bytes[look].is_ascii_whitespace() {
                    look += 1;
                }
                let kind = if look < bytes.len() && bytes[look] == b':' {
                    TokenKind::Attr
                } else {
                    TokenKind::String
                };
                push(out, i - start, kind);
            }
            b'-' if i + 1 < bytes.len() && bytes[i + 1].is_ascii_digit() => {
                let start = i;
                i = scan_number(bytes, i + 1);
                push(out, i - start, TokenKind::Number);
            }
            b'0'..=b'9' => {
                let start = i;
                i = scan_number(bytes, i);
                push(out, i - start, TokenKind::Number);
            }
            b'a'..=b'z' | b'A'..=b'Z' => {
                let start = i;
                while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let kind = match &code[start..i] {
                    "true" | "false" | "null" => TokenKind::Keyword,
                    _ => TokenKind::Plain,
                };
                push(out, i - start, kind);
            }
            _ => {
                let step = next_char_len(code, i);
                push(out, step, TokenKind::Plain);
                i += step;
            }
        }
    }
}

fn highlight_code(lang: Lang, code: &str, out: &mut Vec<(usize, TokenKind)>) {
    let bytes = code.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let b = bytes[i];

        // Line & block comments.
        if matches!(lang, Lang::Rust | Lang::Js) && code[i..].starts_with("//") {
            let end = scan_to_eol(bytes, i);
            push(out, end - i, TokenKind::Comment);
            i = end;
            continue;
        }
        if matches!(lang, Lang::Rust | Lang::Js | Lang::Sql) && code[i..].starts_with("/*") {
            let end = code[i + 2..]
                .find("*/")
                .map(|off| i + 2 + off + 2)
                .unwrap_or(bytes.len());
            push(out, end - i, TokenKind::Comment);
            i = end;
            continue;
        }
        if lang == Lang::Sql && code[i..].starts_with("--") {
            let end = scan_to_eol(bytes, i);
            push(out, end - i, TokenKind::Comment);
            i = end;
            continue;
        }
        if matches!(lang, Lang::Python | Lang::Shell) && b == b'#' {
            let end = scan_to_eol(bytes, i);
            push(out, end - i, TokenKind::Comment);
            i = end;
            continue;
        }

        // Strings (note: Rust lifetimes like `'a` are not strings).
        if b == b'"' || (b == b'`' && lang == Lang::Js) {
            let start = i;
            i = scan_quoted(code, i, b);
            push(out, i - start, TokenKind::String);
            continue;
        }
        if b == b'\'' {
            if lang == Lang::Rust {
                // Character literal `'x'` / `'\n'` vs lifetime `'a` / `'static`.
                if let Some(end) = scan_rust_char_or_lifetime(code, i) {
                    let kind = if bytes.get(end - 1) == Some(&b'\'') && end - i >= 3 {
                        TokenKind::String
                    } else {
                        TokenKind::Type
                    };
                    push(out, end - i, kind);
                    i = end;
                    continue;
                }
            } else {
                let start = i;
                i = scan_quoted(code, i, b'\'');
                push(out, i - start, TokenKind::String);
                continue;
            }
        }

        // Shell variables ($VAR / ${VAR}) and CLI flags (--flag / -f).
        if lang == Lang::Shell && b == b'$' {
            let start = i;
            i += 1;
            if i < bytes.len() && bytes[i] == b'{' {
                i += 1;
                while i < bytes.len() && bytes[i] != b'}' && bytes[i] != b'\n' {
                    i += next_char_len(code, i);
                }
                if i < bytes.len() && bytes[i] == b'}' {
                    i += 1;
                }
            } else {
                while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                    i += 1;
                }
            }
            push(
                out,
                i - start,
                if i - start > 1 {
                    TokenKind::Attr
                } else {
                    TokenKind::Plain
                },
            );
            continue;
        }
        if lang == Lang::Shell
            && b == b'-'
            && (i == 0 || bytes[i - 1].is_ascii_whitespace())
            && i + 1 < bytes.len()
            && (bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'-')
        {
            let start = i;
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || matches!(bytes[i], b'-' | b'_'))
            {
                i += 1;
            }
            push(out, i - start, TokenKind::Type);
            continue;
        }

        // Numbers.
        if b.is_ascii_digit() && (i == 0 || !is_ident_char(bytes[i - 1])) {
            let start = i;
            i = scan_number(bytes, i);
            push(out, i - start, TokenKind::Number);
            continue;
        }

        // Identifiers, keywords, and types.
        if b.is_ascii_alphabetic() || b == b'_' {
            let start = i;
            while i < bytes.len() && is_ident_char(bytes[i]) {
                i += 1;
            }
            let word = &code[start..i];
            let kind = classify_word(lang, word);
            push(out, i - start, kind);
            continue;
        }

        let step = next_char_len(code, i);
        push(out, step, TokenKind::Plain);
        i += step;
    }
}

fn classify_word(lang: Lang, word: &str) -> TokenKind {
    match lang {
        Lang::Rust => {
            if matches!(
                word,
                "as" | "async"
                    | "await"
                    | "break"
                    | "const"
                    | "continue"
                    | "crate"
                    | "dyn"
                    | "else"
                    | "enum"
                    | "extern"
                    | "false"
                    | "fn"
                    | "for"
                    | "if"
                    | "impl"
                    | "in"
                    | "let"
                    | "loop"
                    | "match"
                    | "mod"
                    | "move"
                    | "mut"
                    | "pub"
                    | "ref"
                    | "return"
                    | "self"
                    | "static"
                    | "struct"
                    | "super"
                    | "trait"
                    | "true"
                    | "type"
                    | "unsafe"
                    | "use"
                    | "where"
                    | "while"
            ) {
                TokenKind::Keyword
            } else if matches!(
                word,
                "bool"
                    | "char"
                    | "str"
                    | "u8"
                    | "u16"
                    | "u32"
                    | "u64"
                    | "u128"
                    | "usize"
                    | "i8"
                    | "i16"
                    | "i32"
                    | "i64"
                    | "i128"
                    | "isize"
                    | "f32"
                    | "f64"
                    | "Self"
            ) || is_pascal_case(word)
            {
                TokenKind::Type
            } else {
                TokenKind::Plain
            }
        }
        Lang::Js => {
            if matches!(
                word,
                "as" | "async"
                    | "await"
                    | "break"
                    | "case"
                    | "catch"
                    | "class"
                    | "const"
                    | "continue"
                    | "default"
                    | "delete"
                    | "do"
                    | "else"
                    | "enum"
                    | "export"
                    | "extends"
                    | "false"
                    | "finally"
                    | "for"
                    | "from"
                    | "function"
                    | "if"
                    | "implements"
                    | "import"
                    | "in"
                    | "instanceof"
                    | "interface"
                    | "let"
                    | "new"
                    | "null"
                    | "of"
                    | "private"
                    | "public"
                    | "readonly"
                    | "return"
                    | "static"
                    | "super"
                    | "switch"
                    | "this"
                    | "throw"
                    | "true"
                    | "try"
                    | "type"
                    | "typeof"
                    | "undefined"
                    | "var"
                    | "void"
                    | "while"
                    | "yield"
            ) {
                TokenKind::Keyword
            } else if is_pascal_case(word) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            }
        }
        Lang::Python => {
            if matches!(
                word,
                "False"
                    | "None"
                    | "True"
                    | "and"
                    | "as"
                    | "assert"
                    | "async"
                    | "await"
                    | "break"
                    | "class"
                    | "continue"
                    | "def"
                    | "del"
                    | "elif"
                    | "else"
                    | "except"
                    | "finally"
                    | "for"
                    | "from"
                    | "global"
                    | "if"
                    | "import"
                    | "in"
                    | "is"
                    | "lambda"
                    | "nonlocal"
                    | "not"
                    | "or"
                    | "pass"
                    | "raise"
                    | "return"
                    | "try"
                    | "while"
                    | "with"
                    | "yield"
            ) {
                TokenKind::Keyword
            } else if is_pascal_case(word) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            }
        }
        Lang::Shell => {
            if matches!(
                word,
                "if" | "then"
                    | "else"
                    | "elif"
                    | "fi"
                    | "for"
                    | "in"
                    | "do"
                    | "done"
                    | "while"
                    | "until"
                    | "case"
                    | "esac"
                    | "function"
                    | "export"
                    | "local"
                    | "return"
                    | "exit"
                    | "set"
                    | "unset"
                    | "echo"
                    | "cd"
                    | "sudo"
                    | "true"
                    | "false"
            ) {
                TokenKind::Keyword
            } else {
                TokenKind::Plain
            }
        }
        Lang::Sql => {
            let upper = word.to_ascii_uppercase();
            if matches!(
                upper.as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "INSERT"
                    | "INTO"
                    | "VALUES"
                    | "UPDATE"
                    | "SET"
                    | "DELETE"
                    | "CREATE"
                    | "DROP"
                    | "ALTER"
                    | "TABLE"
                    | "INDEX"
                    | "VIEW"
                    | "JOIN"
                    | "LEFT"
                    | "RIGHT"
                    | "INNER"
                    | "OUTER"
                    | "ON"
                    | "AS"
                    | "AND"
                    | "OR"
                    | "NOT"
                    | "IN"
                    | "IS"
                    | "NULL"
                    | "TRUE"
                    | "FALSE"
                    | "ORDER"
                    | "BY"
                    | "GROUP"
                    | "HAVING"
                    | "LIMIT"
                    | "OFFSET"
                    | "DISTINCT"
                    | "UNION"
                    | "ALL"
                    | "CASE"
                    | "WHEN"
                    | "THEN"
                    | "ELSE"
                    | "END"
                    | "PRIMARY"
                    | "KEY"
                    | "REFERENCES"
                    | "DEFAULT"
                    | "BEGIN"
                    | "COMMIT"
                    | "ROLLBACK"
            ) {
                TokenKind::Keyword
            } else if matches!(
                upper.as_str(),
                "INT"
                    | "INTEGER"
                    | "BIGINT"
                    | "TEXT"
                    | "VARCHAR"
                    | "BOOLEAN"
                    | "BOOL"
                    | "TIMESTAMP"
                    | "JSON"
                    | "JSONB"
                    | "UUID"
                    | "BLOB"
            ) {
                TokenKind::Type
            } else {
                TokenKind::Plain
            }
        }
        _ => TokenKind::Plain,
    }
}

fn is_pascal_case(word: &str) -> bool {
    let mut chars = word.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_uppercase() && chars.any(|c| c.is_ascii_lowercase())
}

fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn next_char_len(s: &str, i: usize) -> usize {
    s[i..].chars().next().map_or(1, char::len_utf8)
}

fn scan_to_eol(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i] != b'\n' {
        i += 1;
    }
    i
}

fn scan_quoted(code: &str, mut i: usize, quote: u8) -> usize {
    let bytes = code.as_bytes();
    i += 1;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 1 < bytes.len() {
            i += 1;
            i += next_char_len(code, i);
            continue;
        }
        if bytes[i] == quote {
            return i + 1;
        }
        if bytes[i] == b'\n' && quote != b'`' {
            return i;
        }
        i += next_char_len(code, i);
    }
    i
}

fn scan_rust_char_or_lifetime(code: &str, i: usize) -> Option<usize> {
    let bytes = code.as_bytes();
    if i + 1 >= bytes.len() {
        return None;
    }
    if bytes[i + 1] == b'\\' {
        // Escaped char literal like `'\n'` or `'\u{1234}'`.
        let mut j = i + 2;
        while j < bytes.len() && bytes[j] != b'\'' && bytes[j] != b'\n' {
            j += next_char_len(code, j);
        }
        if j < bytes.len() && bytes[j] == b'\'' {
            return Some(j + 1);
        }
        return None;
    }
    let ch_len = next_char_len(code, i + 1);
    if i + 1 + ch_len < bytes.len() && bytes[i + 1 + ch_len] == b'\'' {
        return Some(i + 1 + ch_len + 1);
    }
    if bytes[i + 1].is_ascii_alphabetic() || bytes[i + 1] == b'_' {
        let mut j = i + 1;
        while j < bytes.len() && is_ident_char(bytes[j]) {
            j += 1;
        }
        return Some(j);
    }
    None
}

fn scan_number(bytes: &[u8], mut i: usize) -> usize {
    if i + 1 < bytes.len()
        && bytes[i] == b'0'
        && matches!(bytes[i + 1], b'x' | b'X' | b'b' | b'B' | b'o' | b'O')
    {
        i += 2;
        while i < bytes.len() && (bytes[i].is_ascii_hexdigit() || bytes[i] == b'_') {
            i += 1;
        }
        return i;
    }
    while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
        i += 1;
    }
    if i + 1 < bytes.len() && bytes[i] == b'.' && bytes[i + 1].is_ascii_digit() {
        i += 1;
        while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'_') {
            i += 1;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_covers(lang: Lang, code: &str) -> Vec<(usize, TokenKind)> {
        let spans = highlight(lang, code);
        let mut at = 0;
        for &(len, _) in &spans {
            assert!(len > 0, "spans must be non-empty");
            assert!(
                code.is_char_boundary(at + len),
                "spans must stay on UTF-8 boundaries"
            );
            at += len;
        }
        assert_eq!(at, code.len(), "spans must cover the entire input");
        spans
    }

    #[test]
    fn detects_languages_from_hints_filenames_and_content() {
        assert_eq!(
            Lang::detect(
                Some("text"),
                Some("Chalk_Custom.terminal"),
                "<?xml version=\"1.0\"?>\n<plist><dict></dict></plist>"
            ),
            Lang::Xml
        );
        assert_eq!(
            Lang::detect(Some("Plain Text"), None, "<?xml version=\"1.0\"?>"),
            Lang::Xml
        );
        assert_eq!(Lang::detect(Some("rust"), None, ""), Lang::Rust);
        assert_eq!(Lang::detect(Some("ts"), None, ""), Lang::Js);
        assert_eq!(Lang::detect(None, Some("deploy.sh"), ""), Lang::Shell);
        assert_eq!(Lang::detect(None, None, "{\n  \"ok\": true\n}"), Lang::Json);
        assert_eq!(
            Lang::detect(None, None, "SELECT id FROM users WHERE active = true;"),
            Lang::Sql
        );
        assert_eq!(
            Lang::detect(None, None, "diff --git a/foo b/foo\n@@ -1 +1 @@\n+hi"),
            Lang::Diff
        );
    }

    #[test]
    fn highlights_all_languages_and_utf8_safely() {
        let samples = [
            (
                Lang::Xml,
                "<!-- café -->\n<?xml version=\"1.0\"?>\n<key>ANSIBlackColor</key>",
            ),
            (
                Lang::Json,
                "{\"name\": \"日本語\", \"count\": 42, \"ok\": true}",
            ),
            (
                Lang::Rust,
                "pub fn greet<'a>(name: &'a str) -> String {\n    // héllo\n    let n = 0x2A;\n    format!(\"hi {name}\")\n}",
            ),
            (Lang::Js, "const fn = (x: MyType) => `hi ${x}` // ok"),
            (
                Lang::Python,
                "# note\ndef fib(n):\n    return None if n == 0 else n",
            ),
            (
                Lang::Shell,
                "export FOO=\"bar\" # set\ncargo test --workspace $FOO",
            ),
            (
                Lang::Sql,
                "-- query\nSELECT id FROM items WHERE count >= 10;",
            ),
            (Lang::Diff, "--- a/x\n+++ b/x\n@@ -1 +1 @@\n-old\n+new"),
            (Lang::Plain, "plain café text"),
        ];
        for (lang, code) in samples {
            let spans = assert_covers(lang, code);
            if lang != Lang::Plain {
                assert!(
                    spans.iter().any(|&(_, k)| k != TokenKind::Plain),
                    "expected non-plain tokens for {lang:?}"
                );
            }
        }
    }
}
