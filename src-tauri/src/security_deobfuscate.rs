//! Bounded lexical transformations of an explicitly recorded command. These
//! outputs are search aids, never evaluated programs or proof of execution.
use crate::entities::Decoded;
use regex::Regex;
use std::sync::LazyLock;
static CONCAT: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?:'[^'\r\n]{1,256}'|"[^"$`\r\n]{1,256}")(?:\s*\+\s*(?:'[^'\r\n]{1,256}'|"[^"$`\r\n]{1,256}")){1,16}"#).unwrap()
});
static LITERAL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"'([^'\r\n]*)'|"([^"\r\n]*)""#).unwrap());
pub fn command(text: &str, process: Option<&str>, source: &str) -> Vec<Decoded> {
    if crate::security_normalize::literal_output(text) || text.len() > 64000 {
        return vec![];
    }
    let executable = process
        .unwrap_or("")
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let prefix = text
        .split_whitespace()
        .next()
        .unwrap_or("")
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .trim_matches('"')
        .to_ascii_lowercase();
    let powershell = ["powershell", "powershell.exe", "pwsh", "pwsh.exe"]
        .contains(&executable.as_str())
        || ["powershell", "powershell.exe", "pwsh", "pwsh.exe"].contains(&prefix.as_str());
    let cmd = ["cmd", "cmd.exe"].contains(&executable.as_str())
        || ["cmd", "cmd.exe"].contains(&prefix.as_str());
    let shell = ["bash", "sh", "zsh"].contains(&executable.as_str())
        || ["bash", "sh", "zsh"].contains(&prefix.as_str());
    let mut out = Vec::new();
    if powershell || cmd {
        let marker = if powershell { '`' } else { '^' };
        let mut chars = text.chars().peekable();
        let mut decoded = String::new();
        let mut changed = false;
        while let Some(c) = chars.next() {
            if c == marker {
                if let Some(&next) = chars.peek() {
                    if !powershell
                        || !matches!(next, '0' | 'a' | 'b' | 'e' | 'f' | 'n' | 'r' | 't' | 'v')
                    {
                        decoded.push(chars.next().unwrap());
                        changed = true;
                        continue;
                    }
                }
            }
            decoded.push(c);
        }
        if changed {
            out.push(Decoded {
                kind: if powershell {
                    "PowerShell: escapes lexicais"
                } else {
                    "cmd: escapes lexicais"
                }
                .into(),
                source: source.into(),
                text: decoded.chars().take(20000).collect(),
            });
        }
    }
    if powershell {
        let decoded = CONCAT.replace_all(text, |captures: &regex::Captures<'_>| {
            LITERAL
                .captures_iter(&captures[0])
                .filter_map(|c| c.get(1).or_else(|| c.get(2)))
                .map(|m| m.as_str())
                .collect::<String>()
        });
        if decoded != text {
            out.push(Decoded {
                kind: "PowerShell: concatenação de literais sem interpolação".into(),
                source: source.into(),
                text: decoded.chars().take(20000).collect(),
            });
        }
    }
    if shell {
        static ANSI: LazyLock<Regex> = LazyLock::new(|| {
            Regex::new(r"\$'((?:\\x[0-9a-fA-F]{2}|[^'\\\r\n]){1,4096})'").unwrap()
        });
        let decoded = ANSI.replace_all(text, |captures: &regex::Captures<'_>| {
            let bytes = captures[1].as_bytes();
            let mut output = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'\\' && i + 3 < bytes.len() && bytes[i + 1] == b'x' {
                    output.push(u8::from_str_radix(&captures[1][i + 2..i + 4], 16).unwrap());
                    i += 4;
                } else {
                    output.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(output).unwrap_or_else(|_| captures[0].to_string())
        });
        if decoded != text {
            out.push(Decoded {
                kind: "Shell: literais ANSI com escapes hexadecimais".into(),
                source: source.into(),
                text: decoded.chars().take(20000).collect(),
            });
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "Validação específica opt-in da 0.13; execute explicitamente com --ignored"]
    fn v013_static_command_transformations_are_bounded_contextual_and_never_execute() {
        assert!(command("cmd /c who^ami", None, "CommandLine")[0]
            .text
            .contains("whoami"));
        assert!(command(
            "powershell -c & ('Invoke'+'-Expression')",
            None,
            "CommandLine"
        )[0]
        .text
        .contains("Invoke-Expression"));
        assert!(command("bash -c $'\\x77hoami'", None, "CommandLine")[0]
            .text
            .contains("whoami"));
        assert!(command("echo 'cmd /c who^ami'", Some("cmd.exe"), "CommandLine").is_empty());
        assert!(command("powershell -c ${secret}+variable", None, "CommandLine").is_empty());
        assert!(command("powershell -c ('Invoke'+\"$secret\")", None, "CommandLine").is_empty());
        assert!(command("quoted administrator tutorial who^ami", None, "message").is_empty());
        assert!(command(&"x".repeat(64001), Some("cmd.exe"), "CommandLine").is_empty());
    }
}
