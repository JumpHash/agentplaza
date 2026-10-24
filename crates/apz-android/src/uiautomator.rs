//! The XML of a `uiautomator dump` (PLAN 19.3), read into nodes. A dump is a `hierarchy` of `node` elements, each with its
//! class, text, resource id, content description, flags and `bounds` (`[left,top][right,bottom]`). The reader is small on
//! purpose: it reads the start and end tags of `node`, their attributes and the five predefined entities and numeric
//! character references, and skips anything else (the line adb prints before the XML, the declaration, comments).

/// A rectangle on the screen, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bounds {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Bounds {
    /// The point a tap lands on: the centre of the rectangle.
    pub fn center(self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }

    /// Reads `[l,t][r,b]`. None when the text is not that shape.
    pub fn parse(text: &str) -> Option<Bounds> {
        if !(text.starts_with('[') && text.ends_with(']') && text.contains("][")) {
            return None;
        }
        let numbers: Vec<i32> = text
            .split(['[', ']', ','])
            .filter(|s| !s.is_empty())
            .map(|s| s.trim().parse::<i32>().ok())
            .collect::<Option<Vec<i32>>>()?;
        match numbers[..] {
            [left, top, right, bottom] => Some(Bounds {
                left,
                top,
                right,
                bottom,
            }),
            _ => None,
        }
    }
}

/// One `node` of the dump: its position in the tree (`0`, `0.2`, `0.2.1`: the index of each ancestor's child), and what the
/// node says about itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Node {
    pub path: String,
    pub class: String,
    pub text: String,
    pub resource_id: String,
    pub content_desc: String,
    pub clickable: bool,
    pub enabled: bool,
    pub checkable: bool,
    pub password: bool,
    pub bounds: Bounds,
}

/// A start or end tag, with its attributes decoded.
struct Tag {
    name: String,
    closing: bool,
    selfclosing: bool,
    attrs: Vec<(String, String)>,
}

impl Tag {
    fn attr(&self, key: &str) -> &str {
        self.attrs
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }
}

/// Reads a dump. The text before its `hierarchy` (adb's own line) is skipped. An answer with no `hierarchy` is refused: the
/// screen was not read. A `node` with no bounds is refused.
pub fn parse_dump(text: &str) -> Result<Vec<Node>, String> {
    let start = text
        .find("<hierarchy")
        .ok_or("the dump has no hierarchy: the screen was not read")?;
    let mut stack: Vec<(String, usize)> = Vec::new();
    let mut roots = 0usize;
    let mut nodes = Vec::new();
    for tag in tags(&text[start..]) {
        if tag.name != "node" {
            continue;
        }
        if tag.closing {
            stack.pop();
            continue;
        }
        let path = match stack.last_mut() {
            Some((parent, children)) => {
                let path = format!("{parent}.{children}");
                *children += 1;
                path
            }
            None => {
                let path = roots.to_string();
                roots += 1;
                path
            }
        };
        let bounds = Bounds::parse(tag.attr("bounds"))
            .ok_or_else(|| format!("node {path} has no bounds"))?;
        nodes.push(Node {
            path: path.clone(),
            class: tag.attr("class").to_string(),
            text: tag.attr("text").to_string(),
            resource_id: tag.attr("resource-id").to_string(),
            content_desc: tag.attr("content-desc").to_string(),
            clickable: tag.attr("clickable") == "true",
            enabled: tag.attr("enabled") == "true",
            checkable: tag.attr("checkable") == "true",
            password: tag.attr("password") == "true",
            bounds,
        });
        if !tag.selfclosing {
            stack.push((path, 0));
        }
    }
    Ok(nodes)
}

/// The tags of an XML text, in order. Declarations, comments and text are skipped.
fn tags(xml: &str) -> Vec<Tag> {
    let bytes = xml.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(lt) = xml[i..].find('<').map(|k| i + k) {
        let mut j = lt + 1;
        if matches!(bytes.get(j), Some(b'?') | Some(b'!')) {
            i = xml[j..].find('>').map(|k| j + k + 1).unwrap_or(bytes.len());
            continue;
        }
        let closing = bytes.get(j) == Some(&b'/');
        if closing {
            j += 1;
        }
        let name_end = (j..bytes.len())
            .find(|&k| matches!(bytes[k], b' ' | b'\t' | b'\r' | b'\n' | b'/' | b'>'))
            .unwrap_or(bytes.len());
        let name = xml[j..name_end].to_string();
        let mut attrs = Vec::new();
        let mut selfclosing = false;
        let mut k = name_end;
        loop {
            while k < bytes.len() && bytes[k].is_ascii_whitespace() {
                k += 1;
            }
            match bytes.get(k) {
                None => break,
                Some(b'>') => {
                    k += 1;
                    break;
                }
                Some(b'/') => {
                    selfclosing = true;
                    k += 1;
                }
                Some(_) => {
                    let eq = match xml[k..].find('=') {
                        Some(e) => k + e,
                        None => bytes.len(),
                    };
                    let key = xml[k..eq].trim().to_string();
                    let quote = bytes.get(eq + 1).copied().unwrap_or(b'"');
                    let value_start = eq + 2;
                    let value_end = (value_start..bytes.len())
                        .find(|&m| bytes[m] == quote)
                        .unwrap_or(bytes.len());
                    let raw = &xml[value_start.min(bytes.len())..value_end];
                    if !key.is_empty() {
                        attrs.push((key, decode(raw)));
                    }
                    k = (value_end + 1).min(bytes.len());
                }
            }
        }
        out.push(Tag {
            name,
            closing,
            selfclosing,
            attrs,
        });
        i = k;
    }
    out
}

/// The five predefined entities and numeric character references of an attribute value.
fn decode(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        rest = &rest[amp..];
        let Some(semi) = rest.find(';') else { break };
        let entity = &rest[1..semi];
        let decoded = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix('#')
                .and_then(|n| match n.strip_prefix('x') {
                    Some(hex) => u32::from_str_radix(hex, 16).ok(),
                    None => n.parse::<u32>().ok(),
                })
                .and_then(char::from_u32),
        };
        match decoded {
            Some(c) => {
                out.push(c);
                rest = &rest[semi + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DUMP: &str = "UI hierchary dumped to: /dev/tty\n<?xml version='1.0' encoding='UTF-8' standalone='yes' ?>\n\
<hierarchy index=\"0\" class=\"hierarchy\" rotation=\"0\">\n\
  <node index=\"0\" text=\"\" resource-id=\"\" class=\"android.widget.FrameLayout\" content-desc=\"\" clickable=\"false\" enabled=\"true\" bounds=\"[0,0][1080,2400]\">\n\
    <node index=\"0\" text=\"Cart &amp; tax\" resource-id=\"com.shop:id/title\" class=\"android.widget.TextView\" content-desc=\"\" clickable=\"false\" enabled=\"true\" bounds=\"[40,100][1040,200]\" />\n\
    <node index=\"1\" text=\"Pay\" resource-id=\"com.shop:id/pay\" class=\"android.widget.Button\" content-desc=\"\" clickable=\"true\" enabled=\"true\" bounds=\"[40,1600][1040,1760]\" />\n\
    <node index=\"2\" text=\"\" resource-id=\"\" class=\"android.widget.EditText\" content-desc=\"Note &quot;for&quot; the cook\" checkable=\"false\" clickable=\"true\" password=\"true\" enabled=\"true\" bounds=\"[40,300][1040,400]\" >\n\
      <node index=\"0\" text=\"\" class=\"android.view.View\" clickable=\"false\" enabled=\"true\" bounds=\"[0,0][0,0]\" />\n\
    </node>\n\
  </node>\n\
</hierarchy>\n";

    #[test]
    fn a_dump_reads_into_nodes_with_their_paths_bounds_and_flags() {
        let nodes = parse_dump(DUMP).unwrap();
        let paths: Vec<&str> = nodes.iter().map(|n| n.path.as_str()).collect();
        assert_eq!(paths, ["0", "0.0", "0.1", "0.2", "0.2.0"]);
        let pay = &nodes[2];
        assert_eq!(pay.text, "Pay");
        assert_eq!(pay.resource_id, "com.shop:id/pay");
        assert!(pay.clickable && pay.enabled);
        assert_eq!(pay.bounds.center(), (540, 1680));
        assert_eq!(nodes[1].text, "Cart & tax", "entities are decoded");
        let note = &nodes[3];
        assert_eq!(note.content_desc, "Note \"for\" the cook");
        assert!(note.password && note.clickable && !note.checkable);
    }

    #[test]
    fn a_dump_without_a_hierarchy_or_a_node_without_bounds_is_refused() {
        assert!(
            parse_dump("<html></html>")
                .unwrap_err()
                .contains("hierarchy")
        );
        let no_bounds = "<hierarchy><node class=\"x\" /></hierarchy>";
        assert!(parse_dump(no_bounds).unwrap_err().contains("no bounds"));
    }

    #[test]
    fn bounds_read_the_bracketed_form_only() {
        assert_eq!(
            Bounds::parse("[10,20][30,60]"),
            Some(Bounds {
                left: 10,
                top: 20,
                right: 30,
                bottom: 60
            })
        );
        assert_eq!(Bounds::parse("10,20,30,60"), None);
        assert_eq!(Bounds::parse("[10,20]"), None);
        assert_eq!(Bounds::parse("[a,b][c,d]"), None);
    }

    #[test]
    fn an_entity_that_is_not_one_stays_as_it_is() {
        assert_eq!(decode("a &bogus; b &#65;&#x42;"), "a &bogus; b AB");
        assert_eq!(decode("tail &"), "tail &");
    }
}
