//! Shell 解析器 —— 全项目唯一的命令行解析器。
//!
//! 不使用 `split_whitespace`，而是真正的词法分析 + 语法分析：
//!
//! * 单引号 / 双引号 / 反斜杠转义（含引号内的转义），中文等 UTF-8 参数不乱码；
//! * 管道 `|`、stdin 重定向 `<`、stdout 覆盖 `>` / 追加 `>>`；
//! * 命令链 `&&`、`||`、`;`，后台作业 `&`；
//! * `--json` 标志的检测与剥离。
//!
//! 解析结果是一到多个 [`Pipeline`]：每段管道自带一个「与下一段之间的连接符」
//! [`ChainSeparator`]，执行器据此决定下一段是否执行。

/// 解析后的命令管道（一个或多个由 `|` 连接的命令）。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pipeline {
    pub commands: Vec<Command>,
    /// 本段与下一段之间的连接符；最后一段恒为 `Semicolon`。
    pub separator: ChainSeparator,
}

/// 命令之间的连接符。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChainSeparator {
    /// `;` / 行尾 / `&`：下一段无条件执行。
    Semicolon,
    /// `&&`：前一段成功才执行下一段。
    And,
    /// `||`：前一段失败才执行下一段。
    Or,
}

/// 单个命令及其重定向。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Command {
    pub argv: Vec<String>,
    pub stdin: Redirection,
    pub stdout: Redirection,
    /// 末尾的 `&`：该命令放到后台作业线程执行。
    pub background: bool,
}

/// 重定向方式。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Redirection {
    /// 默认：继承终端 / 使用管道输入。
    Default,
    /// 从文件读取：`< path`。
    FromFile(String),
    /// 向文件写入（覆盖）：`> path`。
    ToFile(String),
    /// 向文件追加：`>> path`。
    AppendFile(String),
}

/// 解析错误。
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ParseError {
    ExpectedFileAfterRedirect,
    EmptyInput,
    EmptyCommand,
    EmptyCommandAfterPipe,
    UnclosedQuote,
    TrailingEscape,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = match self {
            ParseError::ExpectedFileAfterRedirect => "重定向后缺少文件路径",
            ParseError::EmptyInput => "空输入",
            ParseError::EmptyCommand => "空命令",
            ParseError::EmptyCommandAfterPipe => "管道后缺少命令",
            ParseError::UnclosedQuote => "未闭合的引号",
            ParseError::TrailingEscape => "未闭合的转义",
        };
        write!(f, "{}", msg)
    }
}

impl std::error::Error for ParseError {}

/// 词法分析产出的 Token。
#[derive(Clone, Debug, PartialEq, Eq)]
enum Token {
    Word(String),
    Pipe,              // `|`
    OrIf,              // `||`
    Background,        // `&`
    AndIf,             // `&&`
    Semicolon,         // `;`
    RedirectIn,        // `<`
    RedirectOut,       // `>`
    RedirectOutAppend, // `>>`
    EndOfFile,
}

/// 词法分析器：按 `char` 切分，天然支持中文等非 ASCII 字符。
struct Lexer<'a> {
    chars: std::iter::Peekable<std::str::Chars<'a>>,
    /// 单 Token 前瞻缓存。
    pending: Option<Token>,
    /// 读词过程中遇到的错误（在语法层统一抛出，避免 Token 里塞 Result）。
    error: Option<ParseError>,
}

impl<'a> Lexer<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            chars: input.chars().peekable(),
            pending: None,
            error: None,
        }
    }

    fn peek(&mut self) -> Token {
        if self.pending.is_none() {
            let token = match self.next_token() {
                Ok(t) => t,
                Err(e) => {
                    self.error = Some(e);
                    Token::EndOfFile
                }
            };
            self.pending = Some(token);
        }
        self.pending.clone().unwrap_or(Token::EndOfFile)
    }

    fn next(&mut self) -> Token {
        if let Some(t) = self.pending.take() {
            return t;
        }
        match self.next_token() {
            Ok(t) => t,
            Err(e) => {
                self.error = Some(e);
                Token::EndOfFile
            }
        }
    }

    fn check_error(&self) -> Result<(), ParseError> {
        match &self.error {
            Some(e) => Err(e.clone()),
            None => Ok(()),
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

    fn next_token(&mut self) -> Result<Token, ParseError> {
        self.skip_whitespace();
        let Some(&c) = self.chars.peek() else {
            return Ok(Token::EndOfFile);
        };
        Ok(match c {
            '|' => {
                self.chars.next();
                if self.chars.peek() == Some(&'|') {
                    self.chars.next();
                    Token::OrIf
                } else {
                    Token::Pipe
                }
            }
            '&' => {
                self.chars.next();
                if self.chars.peek() == Some(&'&') {
                    self.chars.next();
                    Token::AndIf
                } else {
                    Token::Background
                }
            }
            ';' => {
                self.chars.next();
                Token::Semicolon
            }
            '<' => {
                self.chars.next();
                Token::RedirectIn
            }
            '>' => {
                self.chars.next();
                if self.chars.peek() == Some(&'>') {
                    self.chars.next();
                    Token::RedirectOutAppend
                } else {
                    Token::RedirectOut
                }
            }
            _ => Token::Word(self.read_word()?),
        })
    }

    /// 读一个「词」：引号可以出现在词的任意位置，反斜杠在非单引号内都转义。
    fn read_word(&mut self) -> Result<String, ParseError> {
        let mut out = String::new();
        let mut quote: Option<char> = None;
        let mut started = false;

        while let Some(&c) = self.chars.peek() {
            if let Some(q) = quote {
                self.chars.next();
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    match self.chars.next() {
                        Some(nc) => out.push(nc),
                        None => return Err(ParseError::UnclosedQuote),
                    }
                } else {
                    out.push(c);
                }
                continue;
            }
            match c {
                '\'' | '"' => {
                    started = true;
                    quote = Some(c);
                    self.chars.next();
                }
                '\\' => {
                    started = true;
                    self.chars.next();
                    match self.chars.next() {
                        Some(nc) => out.push(nc),
                        None => return Err(ParseError::TrailingEscape),
                    }
                }
                c if c.is_whitespace() => break,
                '|' | '&' | ';' | '<' | '>' => break,
                _ => {
                    started = true;
                    out.push(c);
                    self.chars.next();
                }
            }
        }

        if quote.is_some() {
            return Err(ParseError::UnclosedQuote);
        }
        let _ = started; // 至少消费了一个字符，空词（如 `""`）也是合法参数。
        Ok(out)
    }
}

/// 一条命令的结束原因。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CommandEnd {
    Pipe,
    Background,
    Chain(ChainSeparator),
    Eof,
}

struct Parser<'a> {
    lexer: Lexer<'a>,
}

impl<'a> Parser<'a> {
    fn new(input: &'a str) -> Self {
        Self {
            lexer: Lexer::new(input),
        }
    }

    /// 解析整行，返回一段或多段管道。
    fn parse_lines(&mut self) -> Result<Vec<Pipeline>, ParseError> {
        let mut pipelines = Vec::new();
        let mut commands: Vec<Command> = Vec::new();

        loop {
            if matches!(self.lexer.peek(), Token::EndOfFile) {
                self.lexer.check_error()?;
                if commands.is_empty() {
                    break;
                }
                return Err(ParseError::EmptyCommandAfterPipe);
            }

            let (cmd, end) = self.parse_command()?;
            self.lexer.check_error()?;
            if cmd.argv.is_empty()
                && cmd.stdin == Redirection::Default
                && cmd.stdout == Redirection::Default
            {
                return Err(ParseError::EmptyCommand);
            }
            commands.push(cmd);

            let separator = match end {
                // `|`：本段还没结束，继续收集下一条命令。
                CommandEnd::Pipe => continue,
                CommandEnd::Background => ChainSeparator::Semicolon,
                CommandEnd::Chain(s) => s,
                CommandEnd::Eof => ChainSeparator::Semicolon,
            };
            pipelines.push(Pipeline {
                commands: std::mem::take(&mut commands),
                separator,
            });
            if end == CommandEnd::Eof {
                break;
            }
        }

        if pipelines.is_empty() {
            return Err(ParseError::EmptyInput);
        }
        Ok(pipelines)
    }

    fn parse_command(&mut self) -> Result<(Command, CommandEnd), ParseError> {
        let mut cmd = Command {
            argv: Vec::new(),
            stdin: Redirection::Default,
            stdout: Redirection::Default,
            background: false,
        };

        loop {
            match self.lexer.peek() {
                Token::Word(w) => {
                    self.lexer.next();
                    cmd.argv.push(w);
                }
                Token::RedirectIn => {
                    self.lexer.next();
                    cmd.stdin = Redirection::FromFile(self.expect_path()?);
                }
                Token::RedirectOut => {
                    self.lexer.next();
                    cmd.stdout = Redirection::ToFile(self.expect_path()?);
                }
                Token::RedirectOutAppend => {
                    self.lexer.next();
                    cmd.stdout = Redirection::AppendFile(self.expect_path()?);
                }
                Token::Pipe => {
                    self.lexer.next();
                    return Ok((cmd, CommandEnd::Pipe));
                }
                Token::OrIf => {
                    self.lexer.next();
                    return Ok((cmd, CommandEnd::Chain(ChainSeparator::Or)));
                }
                Token::AndIf => {
                    self.lexer.next();
                    return Ok((cmd, CommandEnd::Chain(ChainSeparator::And)));
                }
                Token::Semicolon => {
                    self.lexer.next();
                    return Ok((cmd, CommandEnd::Chain(ChainSeparator::Semicolon)));
                }
                Token::Background => {
                    self.lexer.next();
                    cmd.background = true;
                    return Ok((cmd, CommandEnd::Background));
                }
                Token::EndOfFile => {
                    self.lexer.check_error()?;
                    return Ok((cmd, CommandEnd::Eof));
                }
            }
        }
    }

    fn expect_path(&mut self) -> Result<String, ParseError> {
        match self.lexer.next() {
            Token::Word(p) if !p.is_empty() => Ok(p),
            _ => Err(ParseError::ExpectedFileAfterRedirect),
        }
    }
}

/// 解析整行（支持命令链），返回所有管道段。Shell 执行器使用这个入口。
pub fn parse_lines(input: &str) -> Result<Vec<Pipeline>, ParseError> {
    Parser::new(input).parse_lines()
}

/// 兼容旧接口：只返回第一段管道。用于只需要单条管道的外部调用与测试。
pub fn parse_line(input: &str) -> Result<Pipeline, ParseError> {
    let mut pipelines = parse_lines(input)?;
    Ok(pipelines.remove(0))
}

/// 检查命令是否请求 JSON 输出（`--json` 标志）。
pub fn wants_json(argv: &[String]) -> bool {
    argv.iter().any(|a| a == "--json")
}

/// 从 argv 中移除 `--json` 标志。
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
    fn utf8_arguments_are_not_garbled() {
        let p = parse_line("write /tmp/a.txt 你好 Hib OS").unwrap();
        assert_eq!(
            p.commands[0].argv,
            vec!["write", "/tmp/a.txt", "你好", "Hib", "OS"]
        );
        let q = parse_line("echo \"中文 参数\"").unwrap();
        assert_eq!(q.commands[0].argv[1], "中文 参数");
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
    fn background_then_foreground() {
        let ps = parse_lines("sleep 1 & pwd").unwrap();
        assert_eq!(ps.len(), 2);
        assert!(ps[0].commands[0].background);
        assert_eq!(ps[1].commands[0].argv, vec!["pwd"]);
    }

    #[test]
    fn command_chains() {
        let and = parse_lines("a && b").unwrap();
        assert_eq!(and.len(), 2);
        assert_eq!(and[0].separator, ChainSeparator::And);
        assert_eq!(and[1].separator, ChainSeparator::Semicolon);

        let or = parse_lines("a || b").unwrap();
        assert_eq!(or.len(), 2);
        assert_eq!(or[0].separator, ChainSeparator::Or);

        let semi = parse_lines("a; b; c").unwrap();
        assert_eq!(semi.len(), 3);
        assert_eq!(semi[0].separator, ChainSeparator::Semicolon);
        assert_eq!(semi[1].separator, ChainSeparator::Semicolon);
    }

    #[test]
    fn mixed_chain_and_pipe() {
        let ps = parse_lines("echo hi | grep h && pwd > out.txt || echo fail").unwrap();
        assert_eq!(ps.len(), 3);
        assert_eq!(ps[0].commands.len(), 2);
        assert_eq!(ps[0].separator, ChainSeparator::And);
        assert_eq!(ps[1].commands[0].stdout, Redirection::ToFile("out.txt".to_string()));
        assert_eq!(ps[1].separator, ChainSeparator::Or);
        assert_eq!(ps[2].commands[0].argv, vec!["echo", "fail"]);
    }

    #[test]
    fn trailing_separator_is_allowed() {
        assert_eq!(parse_lines("pwd;").unwrap().len(), 1);
    }

    #[test]
    fn malformed_input_errors() {
        assert_eq!(parse_lines(""), Err(ParseError::EmptyInput));
        assert_eq!(parse_lines("cat |"), Err(ParseError::EmptyCommandAfterPipe));
        assert_eq!(parse_lines("cat 'abc"), Err(ParseError::UnclosedQuote));
        assert_eq!(parse_lines("cat >"), Err(ParseError::ExpectedFileAfterRedirect));
        assert_eq!(parse_lines("cat \\"), Err(ParseError::TrailingEscape));
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
