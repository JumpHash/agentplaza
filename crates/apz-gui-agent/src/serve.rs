//! The line protocol of the agent: one request per line in, one reply per line out, until the peer closes.

use std::io::{self, BufRead, Write};

use serde_json::{Value, json};

use crate::agent::Agent;

/// Serves one connection: each line is a request, `{"tool": .., "input": ..}`, answered with a line of `{"ok": ..}`.
pub fn serve(
    agent: &mut Agent,
    mut reader: impl BufRead,
    mut writer: impl Write,
) -> io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(request) => {
                let tool = request["tool"].as_str().unwrap_or_default();
                match agent.handle(tool, &request["input"]) {
                    Ok(value) => json!({"ok": true, "value": value}),
                    Err(error) => json!({"ok": false, "error": error}),
                }
            }
            Err(why) => json!({"ok": false, "error": format!("a request is a JSON line: {why}")}),
        };
        writeln!(writer, "{reply}")?;
        writer.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::share;
    use crate::script::{ScriptDesk, shop};

    #[test]
    fn each_request_line_gets_one_reply_line() {
        let input = b"{\"tool\":\"station_lease\",\"input\":{}}\n{\"tool\":\"station_ui_tree\",\"input\":{}}\nnot json\n";
        let mut agent = Agent::new(share(Box::new(ScriptDesk::new(shop(false)))));
        let mut out = Vec::new();
        serve(&mut agent, &input[..], &mut out).unwrap();
        let replies: Vec<Value> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(replies.len(), 3);
        assert_eq!(replies[0]["ok"], true);
        assert_eq!(replies[1]["value"]["name"], "Cart");
        assert_eq!(replies[2]["ok"], false);
    }
}
