use std::io::{self, BufRead};

pub const MAX_LINE_BYTES: usize = 4096;

pub fn parse_target(text: &str) -> anyhow::Result<(Option<String>, nbmcbot_core::Command)> {
    let text = text.trim();
    let Some(targeted) = text.strip_prefix('@') else {
        return Ok((None, nbmcbot_core::parse_command(text)?));
    };
    let (username, command) = targeted
        .split_once(char::is_whitespace)
        .ok_or_else(|| anyhow::anyhow!("expected @Username command"))?;
    anyhow::ensure!(!username.is_empty(), "command target is empty");
    let command = nbmcbot_core::parse_command(command)?;
    if username == "all" {
        return Ok((None, command));
    }
    anyhow::ensure!(
        command != nbmcbot_core::Command::Quit,
        "quit exits the whole process; use @Username disconnect for one bot"
    );
    Ok((Some(username.into()), command))
}

#[derive(Debug, PartialEq)]
pub enum InputLine {
    Text(String),
    TooLong,
    InvalidUtf8,
    Eof,
}

pub fn read_line(reader: &mut impl BufRead) -> io::Result<InputLine> {
    let mut bytes = Vec::new();
    let mut overlong = false;
    loop {
        let chunk = reader.fill_buf()?;
        if chunk.is_empty() {
            if overlong {
                return Ok(InputLine::TooLong);
            }
            if bytes.is_empty() {
                return Ok(InputLine::Eof);
            }
            break;
        }
        let newline = chunk.iter().position(|&b| b == b'\n');
        let content_len = newline.unwrap_or(chunk.len());
        if !overlong {
            if bytes.len() + content_len > MAX_LINE_BYTES {
                overlong = true;
                bytes.clear();
            } else {
                bytes.extend_from_slice(&chunk[..content_len]);
            }
        }
        reader.consume(content_len + usize::from(newline.is_some()));
        if newline.is_some() {
            break;
        }
    }
    if overlong {
        return Ok(InputLine::TooLong);
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    Ok(match String::from_utf8(bytes) {
        Ok(text) => InputLine::Text(text),
        Err(_) => InputLine::InvalidUtf8,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn routes_commands_to_only_the_requested_account() {
        use nbmcbot_core::Command;
        assert_eq!(parse_target("status").unwrap(), (None, Command::Status));
        assert_eq!(parse_target("@all stop").unwrap(), (None, Command::Stop));
        assert_eq!(
            parse_target("  @Bot02 status  ").unwrap(),
            (Some("Bot02".into()), Command::Status)
        );
        assert_eq!(
            parse_target("@Bot01 say @Bot02 hello").unwrap(),
            (Some("Bot01".into()), Command::Chat("@Bot02 hello".into()))
        );
        for text in ["@ status", "@Bot01", "@Bot01 invalid", "@Bot01 quit"] {
            assert!(parse_target(text).is_err(), "accepted {text:?}");
        }
        assert_eq!(parse_target("@all quit").unwrap(), (None, Command::Quit));
    }
    #[test]
    fn max_length_and_overflow_recovery() {
        let mut source = io::Cursor::new(format!(
            "{}\n{}\nquit\n",
            "x".repeat(MAX_LINE_BYTES),
            "x".repeat(MAX_LINE_BYTES + 1)
        ));
        assert_eq!(
            read_line(&mut source).unwrap(),
            InputLine::Text("x".repeat(MAX_LINE_BYTES))
        );
        assert_eq!(read_line(&mut source).unwrap(), InputLine::TooLong);
        assert_eq!(
            read_line(&mut source).unwrap(),
            InputLine::Text("quit".into())
        );
        assert_eq!(read_line(&mut source).unwrap(), InputLine::Eof);
    }
    #[test]
    fn eof_utf8_and_crlf() {
        let mut source = io::Cursor::new(b"\xff\nstatus\r\nquit");
        assert_eq!(read_line(&mut source).unwrap(), InputLine::InvalidUtf8);
        assert_eq!(
            read_line(&mut source).unwrap(),
            InputLine::Text("status".into())
        );
        assert_eq!(
            read_line(&mut source).unwrap(),
            InputLine::Text("quit".into())
        );
        assert_eq!(read_line(&mut source).unwrap(), InputLine::Eof);
    }
}
