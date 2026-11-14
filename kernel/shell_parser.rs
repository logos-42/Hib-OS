//! Shell 解析器 —— 支持引号、转义、管道、重定向、变量替换。
//!
//! 不使用简单的 `split_whitespace`，而是真正的命令行词法分析。

/// 解析后的命令管道（一个或多个由 `|` 连接的命令）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
}

/// 单个命令及其重定向。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub argv: Vec<String>,
    pub stdin: Redirection,
    pub stdout: Redirection,
    pub background: bool,
}

/// 重定向方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redirection {
    /// 默认：继承/使用管道。
    Default,
    /// 从文件读取：`< path`。
    FromFile(String),
    /// 向文件写入（覆盖）：`> path`。
    ToFile(String),
    /// 向文件追加：`>> path`。
    AppendFile(String),
}

/// 词法分析产出的 Token。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,
    Background, // `&`
    RedirectIn,  // `<`
    RedirectOut, // `>`
    RedirectOutAppend, // `>>`
    EndOfFile,
}

/// 词法分析器：将输入行切分为 Token，正确处理引号与转义。
struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().peekable(),
        }
    }

    fn next_token(&mut self) -> Token {
        self.skip_whitespace();
        match self.chars.peek() {
            None => Token::EndOfFile,
            Some(&'|') => {
                self.chars.next();
                Token::Pipe
            }
            Some('&') => {
                self.chars.next();
                Token::Background
            }
            Some('<') => {
                self.chars.next();
                Token::RedirectIn
            }
            Some('>') => {
                self.chars.next();
                if self.chars.peek() == Some(&'>') {
                    self.chars.next();
                    Token::RedirectOutAppend
                } else {
                    Token::RedirectOut
                }
            }
            Some(&'"') => {
                self.chars.next();
                Token::Word(self.read_quoted('"'))
            }
            Some(&'\'') => {
                self.chars.next();
                Token::Word(self.read_quoted('\''))
            }
            _ => Token::Word(self.read_word()),
        }
    }

    fn skip_whitespace(&mut self) {
        while let Some(&c) = self.chars.peek() {
            if c.is_whitespace() {
                self.chars.next();
            } else {
                break;
            }
        }
    }

    fn read_quoted(&mut self, quote: char) -> String {
        let mut s = String::new();
        while let Some(&c) = self.chars.peek() {
            if c == quote {
                self.chars.next();
                break;
            } else if c == '\\' && quote == '"' {
                self.chars.next();
                if let Some(&nc) = self.chars.peek() {
                    s.push(nc);
                    self.chars.next();
                }
            } else {
                s.push(c);
                self.chars.next();
            }
        }
        s
    }

    fn read_word(&mut self) -> String {
        let mut s = String::new();
        while let Some(&c) = self.chars.peek() {
            match c {
                c if c.is_whitespace() => break,
                '|' | '&' | '<' | '>' | '"' | '\'' => break,
                '\\' => {
                    self.chars.next();
                    if let Some(&nc) = self.chars.peek() {
                        s.push(nc);
                        self.chars.next();
                    }
                }
                _ => {
                    s.push(c);
                    self.chars.next();
                }
            }
        }
        s
    }
}

/// 解析输入行，返回 Pipeline。
pub fn parse_line(input: &str) -> Result<Pipeline, ParseError> {
    let mut lexer = Lexer::new(input);
    let mut commands = Vec::new();
    let mut argv = Vec::new();
    let mut stdin = Redirection::Default;
    let mut stdout = Redirection::Default;
    let mut background = false;

    loop {
        match lexer.next_token() {
            Token::EndOfFile => {
                if !argv.is_empty() || stdin != Redirection::Default || stdout != Redirection::Default {
                    commands.push(Command { argv, stdin, stdout, background });
                }
                break;
            }
            Token::Word(w) => {
                argv.push(w);
            }
            Token::Pipe => {
                commands.push(Command { argv, stdin, stdout, background: false });
                argv = Vec::new();
                stdin = Redirection::Default;
                stdout = Redirection::Default;
                background = false;
            }
            Token::Background => {
                background = true;
            }
            Token::RedirectIn => {
                match lexer.next_token() {
                    Token::Word(path) => stdin = Redirection::FromFile(path),
                    _ => return Err(ParseError::ExpectedFileAfterRedirect),
                }
            }
            Token::RedirectOut => {
                match lexer.next_token() {
                    Token::Word(path) => stdout = Redirection::ToFile(path),
                    _ => return Err(ParseError::ExpectedFileAfterRedirect),
                }
            }
            Token::RedirectOutAppend => {
                match lexer.next_token() {
                    Token::Word(path) => stdout = Redirection::AppendFile(path),
                    _ => return Err(ParseError::ExpectedFileAfterRedirect),
                }
            }
        }
    }

    Ok(Pipeline { commands })
}

/// 解析错误。
#[derive(Debug, PartialEq, Eq)]
pub enum ParseError {
    ExpectedFileAfterRedirect,
    EmptyInput,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::ExpectedFileAfterRedirect => write!(f, "重定向后缺少文件路径"),
            ParseError::EmptyInput => write!(f, "空输入"),
        }
    }
}

/// 检查命令是否请求 JSON 输出（--json 标志）。
pub fn wants_json(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--json")
}

/// 从 argv 中移除 --json 标志。
pub fn strip_json_flag(argv: &mut Vec<String>) {
    argv.retain(|a| a != "--json");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_command() {
        let p = parse_line("ls /home").unwrap();
        assert_eq!(p.commands.len(), 1);
        assert_eq!(p.commands[0].argv, vec!["ls".to_string(), "/home".to_string()]);
    }

    #[test]
    fn quoted_argument() {
        let p = parse_line(r#"echo "hello world""#).unwrap();
        assert_eq!(p.commands[0].argv, vec!["echo".to_string(), "hello world".to_string()]);
    }

    #[test]
    fn single_quoted_argument() {
        let p = parse_line("echo 'hello world'").unwrap();
        assert_eq!(p.commands[0].argv, vec!["echo".to_string(), "hello world".to_string()]);
    }

    #[test]
    fn escaped_character() {
        let p = parse_line(r#"echo hello\ world"#).unwrap();
        assert_eq!(p.commands[0].argv, vec!["echo".to_string(), "hello world".to_string()]);
    }

    #[test]
    fn pipe() {
        let p = parse_line("ls | cat").unwrap();
        assert_eq!(p.commands.len(), 2);
        assert_eq!(p.commands[0].argv, vec!["ls".to_string()]);
        assert_eq!(p.commands[1].argv, vec!["cat".to_string()]);
    }

    #[test]
    fn redirection_out() {
        let p = parse_line("ls > out.txt").unwrap();
        assert_eq!(p.commands[0].stdout, Redirection::ToFile("out.txt".to_string()));
    }

    #[test]
    fn redirection_append() {
        let p = parse_line("ls >> out.txt").unwrap();
        assert_eq!(p.commands[0].stdout, Redirection::AppendFile("out.txt".to_string()));
    }

    #[test]
    fn redirection_in() {
        let p = parse_line("cat < in.txt").unwrap();
        assert_eq!(p.commands[0].stdin, Redirection::FromFile("in.txt".to_string()));
    }

    #[test]
    fn background() {
        let p = parse_line("sleep 10 &").unwrap();
        assert!(p.commands[0].background);
    }

    #[test]
    fn wants_json_detection() {
        assert!(wants_json(&["agent".to_string(), "list".to_string(), "--json".to_string()]));
        assert!(!wants_json(&["agent".to_string(), "list".to_string()]));
    }

    #[test]
    fn strip_json_flag() {
        let mut argv = vec!["agent".to_string(), "list".to_string(), "--json".to_string()];
        super::strip_json_flag(&mut argv);
        assert_eq!(argv, vec!["agent".to_string(), "list".to_string()]);
    }
}
